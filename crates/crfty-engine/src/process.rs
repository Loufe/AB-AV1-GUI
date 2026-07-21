use std::{
    io,
    process::{ChildStderr, ChildStdout, Command, ExitStatus, Output, Stdio},
};

use command_group::{CommandGroup, GroupChild};

#[cfg(windows)]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) struct ContainedChild {
    child: GroupChild,
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
        self.child.inner().stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.inner().stderr.take()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.settled = true;
        }
        Ok(status)
    }

    pub(crate) fn terminate_and_wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.try_wait()? {
            return Ok(status);
        }
        let kill_result = self.child.kill();
        let wait_result = self.child.wait();
        self.settled = wait_result.is_ok();
        match (kill_result, wait_result) {
            (_, Ok(status)) => Ok(status),
            (Err(kill_error), Err(_)) => Err(kill_error),
            (Ok(()), Err(wait_error)) => Err(wait_error),
        }
    }
}

pub(crate) fn output(command: &mut Command) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn_group(command)?.wait_with_output()
}

pub(crate) fn status(command: &mut Command) -> io::Result<ExitStatus> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_group(command)?.wait()
}

fn spawn_group(command: &mut Command) -> io::Result<GroupChild> {
    let mut group = command.group();
    #[cfg(windows)]
    {
        group
            .kill_on_drop(true)
            .creation_flags(WINDOWS_CREATE_NO_WINDOW);
    }
    group.spawn()
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Err(error) = self.terminate_and_wait() {
            tracing::error!("failed to terminate contained child process group: {error}");
        }
    }
}
