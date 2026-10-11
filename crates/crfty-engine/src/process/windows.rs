//! Windows Job ownership with the maintained Tokio wrappers and blocking pipes.

use std::{
    future::Future,
    io,
    os::windows::io::{AsHandle, BorrowedHandle},
    pin::Pin,
    process::{Child, ChildStderr, ChildStdout, Command, ExitStatus},
    sync::{Arc, Mutex},
};

use process_wrap::tokio::{ChildWrapper, CommandWrap, CreationFlags, JobObject, KillOnDrop};
use tokio::{process::Command as TokioCommand, runtime::Runtime};

pub(super) struct WindowsChild {
    child: Box<dyn ChildWrapper>,
    runtime: Runtime,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl WindowsChild {
    pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let placeholder = Command::new(command.get_program());
        let owned_command = TokioCommand::from(std::mem::replace(command, placeholder));
        let mut wrapped = CommandWrap::from(owned_command);
        wrapped
            .wrap(KillOnDrop)
            .wrap(CreationFlags(
                ::windows::Win32::System::Threading::CREATE_NO_WINDOW,
            ))
            .wrap(JobObject);
        let mut stdout = None;
        let mut stderr = None;
        let cleanup = CleanupNotes(Arc::new(Mutex::new(Some(Vec::new()))));
        // Retain native blocking pipes for the supervisor's reader threads.
        // Tokio's Job wrapper only needs the child's process capability.
        let result = wrapped.spawn_with_child(|command| {
            let mut child = command.as_std_mut().spawn()?;
            stdout = child.stdout.take();
            stderr = child.stderr.take();
            Ok(Box::new(NativeChild::new(child, cleanup.clone())))
        });
        *command = wrapped.into_command().into_std();
        let notes = cleanup.finish();
        let result = result.map_err(|primary| {
            if notes.is_empty() {
                primary
            } else {
                io::Error::new(primary.kind(), SpawnFailure { primary, notes })
            }
        });
        result.map(|child| Self {
            child,
            runtime,
            stdout,
            stderr,
        })
    }

    pub(super) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    pub(super) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.stderr.take()
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.inner_mut().try_wait()
    }

    pub(super) fn start_kill(&mut self) -> io::Result<()> {
        self.child.start_kill()
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.runtime.block_on(self.child.wait())
    }
}

/// Native child ownership exists before Job setup can fail. The pipes stay with
/// the synchronous supervisor, and no asynchronous pipe methods are exposed.
#[derive(Debug)]
struct NativeChild {
    child: Child,
    cleanup: CleanupNotes,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
}

impl NativeChild {
    fn new(child: Child, cleanup: CleanupNotes) -> Self {
        Self {
            child,
            cleanup,
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }
}

impl ChildWrapper for NativeChild {
    fn inner(&self) -> &dyn ChildWrapper {
        self
    }

    fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
        self
    }

    fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
        self
    }

    fn process_handle(&self) -> Option<BorrowedHandle<'_>> {
        Some(self.child.as_handle())
    }

    fn id(&self) -> Option<u32> {
        Some(self.child.id())
    }

    fn stdin(&mut self) -> &mut Option<tokio::process::ChildStdin> {
        &mut self.stdin
    }

    fn stdout(&mut self) -> &mut Option<tokio::process::ChildStdout> {
        &mut self.stdout
    }

    fn stderr(&mut self) -> &mut Option<tokio::process::ChildStderr> {
        &mut self.stderr
    }

    fn start_kill(&mut self) -> io::Result<()> {
        let result = self.child.kill();
        if let Err(error) = &result {
            self.cleanup.record("terminate child", error);
        }
        result
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn wait(&mut self) -> Pin<Box<dyn Future<Output = io::Result<ExitStatus>> + Send + '_>> {
        // This adapter is driven only by the synchronous containment boundary.
        Box::pin(async { self.child.wait() })
    }
}

impl Drop for NativeChild {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(error) => self.cleanup.record("observe child", &error),
        }
        if let Err(error) = self.child.kill() {
            self.cleanup.record("terminate child", &error);
            // A failed request does not justify an unbounded wait for a child
            // that might still be suspended or unable to exit.
            if let Err(error) = self.child.try_wait() {
                self.cleanup
                    .record("observe child after termination failure", &error);
            }
            return;
        }
        if let Err(error) = self.child.wait() {
            self.cleanup.record("reap child", &error);
        }
    }
}

#[derive(Clone, Debug)]
struct CleanupNotes(Arc<Mutex<Option<Vec<String>>>>);

impl CleanupNotes {
    fn record(&self, step: &str, error: &io::Error) {
        tracing::error!("Windows child cleanup failed at {step}: {error}");
        let mut notes = self.0.lock().unwrap_or_else(|error| {
            tracing::error!("Windows spawn cleanup notes were poisoned: {error}");
            error.into_inner()
        });
        if let Some(notes) = notes.as_mut() {
            notes.push(format!("{step}: {error}"));
        }
    }

    fn finish(&self) -> Vec<String> {
        let mut notes = self.0.lock().unwrap_or_else(|error| {
            tracing::error!("Windows spawn cleanup notes were poisoned: {error}");
            error.into_inner()
        });
        notes.take().unwrap_or_default()
    }
}

#[derive(Debug)]
struct SpawnFailure {
    primary: io::Error,
    notes: Vec<String>,
}

impl std::fmt::Display for SpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}; cleanup: {}",
            self.primary,
            self.notes.join("; ")
        )
    }
}

impl std::error::Error for SpawnFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

#[cfg(test)]
mod tests;
