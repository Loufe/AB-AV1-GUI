use std::{
    io,
    process::{ChildStderr, ChildStdout, Command, ExitStatus},
};

#[cfg(not(windows))]
use process_wrap::std::{ChildWrapper, CommandWrap};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::WindowsChild;

#[cfg(unix)]
const UNIX_ESRCH: i32 = 3;

pub(crate) struct ContainedChild {
    #[cfg(not(windows))]
    child: Box<dyn ChildWrapper>,
    #[cfg(windows)]
    child: WindowsChild,
    settled: bool,
}

impl ContainedChild {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        spawn_group(command).map(|child| Self {
            child,
            settled: false,
        })
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        #[cfg(not(windows))]
        {
            self.child.stdout().take()
        }
        #[cfg(windows)]
        {
            self.child.take_stdout()
        }
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        #[cfg(not(windows))]
        {
            self.child.stderr().take()
        }
        #[cfg(windows)]
        {
            self.child.take_stderr()
        }
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // Leader exit does not prove that the complete process group/job is
        // empty. Only `terminate_group_and_wait` settles containment.
        #[cfg(not(windows))]
        {
            self.child.inner_mut().try_wait()
        }
        #[cfg(windows)]
        {
            self.child.try_wait()
        }
    }

    pub(crate) fn terminate_and_wait(&mut self) -> io::Result<ExitStatus> {
        self.terminate_group_and_wait()
    }

    /// Terminates the process group even when its leader's exit status was
    /// already observed. On Unix, an orphaned descendant can outlive that
    /// status while still holding the group's output pipes open.
    pub(crate) fn terminate_group_and_wait(&mut self) -> io::Result<ExitStatus> {
        let kill_result = self.child.start_kill();
        let wait_result = self.child.wait();
        let result = match (kill_result, wait_result) {
            (Ok(()), Ok(status)) => Ok(status),
            (Err(kill_error), Ok(status)) if containment_already_empty(&kill_error) => {
                // The containment unit had already emptied between the last
                // observation and termination. The cached leader status is
                // still authoritative.
                Ok(status)
            }
            (Err(kill_error), Ok(_status)) => Err(kill_error),
            (Err(kill_error), Err(wait_error)) => Err(io::Error::new(
                kill_error.kind(),
                format!("termination: {kill_error}; wait: {wait_error}"),
            )),
            (Ok(()), Err(wait_error)) => Err(wait_error),
        };
        self.settled = result.is_ok();
        result
    }
}

fn containment_already_empty(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::InvalidInput | io::ErrorKind::NotFound
    ) {
        return true;
    }
    #[cfg(unix)]
    {
        // process-wrap forwards killpg(2)'s ESRCH without mapping it to
        // ErrorKind::NotFound. ESRCH means the process group no longer exists.
        error.raw_os_error() == Some(UNIX_ESRCH)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(not(windows))]
fn spawn_group(command: &mut Command) -> io::Result<Box<dyn ChildWrapper>> {
    // CommandWrap owns the command during wrapper hooks. Return it to the caller
    // after spawning so callers retain their configured arguments and streams.
    let placeholder = Command::new(command.get_program());
    let mut wrapped = CommandWrap::from(std::mem::replace(command, placeholder));
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    let result = wrapped.spawn();
    *command = wrapped.into_command();
    result
}

#[cfg(windows)]
fn spawn_group(command: &mut Command) -> io::Result<WindowsChild> {
    WindowsChild::spawn(command)
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Err(error) = self.terminate_group_and_wait() {
            tracing::error!("failed to terminate contained child process group: {error}");
        }
    }
}
