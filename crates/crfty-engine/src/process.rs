use std::{
    io,
    process::{ChildStderr, ChildStdout, Command, ExitStatus, Output, Stdio},
};

use command_group::{CommandGroup, GroupChild};

#[cfg(windows)]
const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(unix)]
const UNIX_ESRCH: i32 = 3;

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
        // Leader exit does not prove that the complete process group/job is
        // empty. Only `terminate_group_and_wait` settles containment.
        self.child.try_wait()
    }

    pub(crate) fn terminate_and_wait(&mut self) -> io::Result<ExitStatus> {
        self.terminate_group_and_wait()
    }

    /// Terminates the process group even when its leader's exit status was
    /// already observed. On Unix, an orphaned descendant can outlive that
    /// status while still holding the group's output pipes open.
    pub(crate) fn terminate_group_and_wait(&mut self) -> io::Result<ExitStatus> {
        let kill_result = self.child.kill();
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
            (Err(kill_error), Err(_)) => Err(kill_error),
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
        // command-group forwards killpg(2)'s ESRCH without mapping it to
        // ErrorKind::NotFound. ESRCH means the process group no longer exists.
        error.raw_os_error() == Some(UNIX_ESRCH)
    }
    #[cfg(not(unix))]
    {
        false
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
        if let Err(error) = self.terminate_group_and_wait() {
            tracing::error!("failed to terminate contained child process group: {error}");
        }
    }
}
