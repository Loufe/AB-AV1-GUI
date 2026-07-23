//! Bounded, cancellable supervision for short-lived external tools.
//!
//! The caller owns concurrency: hold a permit while calling [`run`]. The call
//! returns only after the complete process group/job has exited and both pipe
//! readers have joined, so returning also marks the safe point at which that
//! permit may be released.

use std::{
    collections::VecDeque,
    io::Read,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::process::ContainedChild;

const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(20);
const READ_BUFFER_BYTES: usize = 8 * 1024;

/// A cooperative cancellation signal shared between a caller and a running
/// supervisor.
#[derive(Clone, Debug, Default)]
pub struct ProcessCancellation {
    cancelled: Arc<AtomicBool>,
}

impl ProcessCancellation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Resource limits applied while supervising one process invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessLimits {
    /// `None` disables the deadline; cancellation remains available.
    pub timeout: Option<Duration>,
    /// Retain this many bytes from the beginning of stdout while continuing
    /// to drain all later bytes.
    pub stdout_head_bytes: usize,
    /// Retain this many bytes from the end of stderr while continuing to drain
    /// the complete stream.
    pub stderr_tail_bytes: usize,
}

impl ProcessLimits {
    #[must_use]
    pub const fn new(
        timeout: Option<Duration>,
        stdout_head_bytes: usize,
        stderr_tail_bytes: usize,
    ) -> Self {
        Self {
            timeout,
            stdout_head_bytes,
            stderr_tail_bytes,
        }
    }
}

/// A bounded capture with the total number of bytes observed on its pipe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundedOutput {
    bytes: Vec<u8>,
    total_bytes: usize,
}

impl BoundedOutput {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    #[must_use]
    pub fn was_truncated(&self) -> bool {
        self.total_bytes > self.bytes.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessFailureStage {
    Spawn,
    CaptureSetup,
    Wait,
    Terminate,
    ReadStdout,
    ReadStderr,
    JoinStdout,
    JoinStderr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessFailure {
    pub stage: ProcessFailureStage,
    pub message: String,
}

/// The terminal reason for a supervised invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessTerminal {
    Success(ExitStatus),
    ToolFailed(ExitStatus),
    Cancelled,
    TimedOut,
    SpawnFailed(ProcessFailure),
    SupervisionFailed(ProcessFailure),
    CleanupFailed(ProcessFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessReport {
    pub terminal: ProcessTerminal,
    pub stdout: BoundedOutput,
    pub stderr_tail: BoundedOutput,
}

/// Runs one command in an operating-system process group/job.
///
/// Standard input is closed. Standard output and standard error are replaced
/// with pipes and drained concurrently. Stdout retains a bounded head suitable
/// for machine-readable output; stderr retains a bounded diagnostic tail.
#[must_use]
pub fn run(
    command: &mut Command,
    cancellation: &ProcessCancellation,
    limits: ProcessLimits,
) -> ProcessReport {
    if cancellation.is_cancelled() {
        return report(ProcessTerminal::Cancelled);
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match ContainedChild::spawn(command) {
        Ok(child) => child,
        Err(error) => {
            return report(ProcessTerminal::SpawnFailed(failure(
                ProcessFailureStage::Spawn,
                error,
            )));
        }
    };

    let stdout = match child.take_stdout() {
        Some(stdout) => stdout,
        None => {
            return terminate_without_readers(
                &mut child,
                ProcessFailureStage::CaptureSetup,
                "supervised process has no stdout pipe".to_owned(),
            );
        }
    };
    let stderr = match child.take_stderr() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            return terminate_without_readers(
                &mut child,
                ProcessFailureStage::CaptureSetup,
                "supervised process has no stderr pipe".to_owned(),
            );
        }
    };

    let stdout_reader = thread::Builder::new()
        .name("crfty-process-stdout".to_owned())
        .spawn(move || read_head(stdout, limits.stdout_head_bytes));
    let stdout_reader = match stdout_reader {
        Ok(reader) => reader,
        Err(error) => {
            drop(stderr);
            return terminate_without_readers(
                &mut child,
                ProcessFailureStage::CaptureSetup,
                format!("failed to start stdout reader: {error}"),
            );
        }
    };
    let stderr_reader = thread::Builder::new()
        .name("crfty-process-stderr".to_owned())
        .spawn(move || read_tail(stderr, limits.stderr_tail_bytes));
    let stderr_reader = match stderr_reader {
        Ok(reader) => reader,
        Err(error) => {
            let cleanup = child.terminate_group_and_wait().err().map(|terminate| {
                failure(
                    ProcessFailureStage::Terminate,
                    format!(
                        "failed to start stderr reader ({error}); process cleanup also failed: {terminate}"
                    ),
                )
            });
            let stdout = join_reader(stdout_reader, ProcessFailureStage::JoinStdout);
            return ProcessReport {
                terminal: cleanup.map_or_else(
                    || {
                        ProcessTerminal::SupervisionFailed(ProcessFailure {
                            stage: ProcessFailureStage::CaptureSetup,
                            message: format!("failed to start stderr reader: {error}"),
                        })
                    },
                    ProcessTerminal::CleanupFailed,
                ),
                stdout: stdout.output,
                stderr_tail: BoundedOutput::default(),
            };
        }
    };

    let started = Instant::now();
    let mut exit_status = None;
    loop {
        if cancellation.is_cancelled() {
            return terminate_and_finish(
                &mut child,
                ProcessTerminal::Cancelled,
                stdout_reader,
                stderr_reader,
            );
        }

        if exit_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exit_status = Some(status),
                Ok(None) => {}
                Err(error) => {
                    let terminal = ProcessTerminal::SupervisionFailed(failure(
                        ProcessFailureStage::Wait,
                        error,
                    ));
                    return terminate_and_finish(
                        &mut child,
                        terminal,
                        stdout_reader,
                        stderr_reader,
                    );
                }
            }
        }

        if let Some(status) = exit_status {
            let terminal = if status.success() {
                ProcessTerminal::Success(status)
            } else {
                ProcessTerminal::ToolFailed(status)
            };
            // The directly spawned leader defines the invocation's terminal
            // status. Any descendants still in its containment unit are
            // cleanup, regardless of whether they retained or replaced the
            // leader's pipe handles. This keeps pipe inheritance from changing
            // success into a timeout and prevents silent descendants from
            // outliving the caller-owned concurrency permit.
            return terminate_and_finish(&mut child, terminal, stdout_reader, stderr_reader);
        }

        if limits
            .timeout
            .is_some_and(|timeout| started.elapsed() >= timeout)
        {
            return terminate_and_finish(
                &mut child,
                ProcessTerminal::TimedOut,
                stdout_reader,
                stderr_reader,
            );
        }
        thread::sleep(STATUS_POLL_INTERVAL);
    }
}

fn terminate_without_readers(
    child: &mut ContainedChild,
    stage: ProcessFailureStage,
    message: String,
) -> ProcessReport {
    let terminal = match child.terminate_group_and_wait() {
        Ok(_status) => ProcessTerminal::SupervisionFailed(ProcessFailure { stage, message }),
        Err(error) => ProcessTerminal::CleanupFailed(ProcessFailure {
            stage: ProcessFailureStage::Terminate,
            message: format!("{message}; process cleanup also failed: {error}"),
        }),
    };
    report(terminal)
}

fn terminate_and_finish(
    child: &mut ContainedChild,
    intended: ProcessTerminal,
    stdout_reader: thread::JoinHandle<ReaderResult>,
    stderr_reader: thread::JoinHandle<ReaderResult>,
) -> ProcessReport {
    let terminal = match child.terminate_group_and_wait() {
        Ok(_status) => intended,
        Err(error) => {
            ProcessTerminal::CleanupFailed(failure(ProcessFailureStage::Terminate, error))
        }
    };
    finish(terminal, stdout_reader, stderr_reader)
}

fn finish(
    terminal: ProcessTerminal,
    stdout_reader: thread::JoinHandle<ReaderResult>,
    stderr_reader: thread::JoinHandle<ReaderResult>,
) -> ProcessReport {
    let stdout = join_reader(stdout_reader, ProcessFailureStage::JoinStdout);
    let stderr = join_reader(stderr_reader, ProcessFailureStage::JoinStderr);
    let terminal = if matches!(terminal, ProcessTerminal::CleanupFailed(_)) {
        terminal
    } else if let Some(failure) = stdout.failure.or(stderr.failure) {
        ProcessTerminal::SupervisionFailed(failure)
    } else {
        terminal
    };
    ProcessReport {
        terminal,
        stdout: stdout.output,
        stderr_tail: stderr.output,
    }
}

struct JoinedReader {
    output: BoundedOutput,
    failure: Option<ProcessFailure>,
}

fn join_reader(
    reader: thread::JoinHandle<ReaderResult>,
    panic_stage: ProcessFailureStage,
) -> JoinedReader {
    match reader.join() {
        Ok(result) => JoinedReader {
            output: result.output,
            failure: result.failure,
        },
        Err(_panic) => JoinedReader {
            output: BoundedOutput::default(),
            failure: Some(ProcessFailure {
                stage: panic_stage,
                message: "process output reader panicked".to_owned(),
            }),
        },
    }
}

struct ReaderResult {
    output: BoundedOutput,
    failure: Option<ProcessFailure>,
}

fn read_head(mut reader: impl Read, limit: usize) -> ReaderResult {
    let mut output = Vec::with_capacity(limit.min(READ_BUFFER_BYTES));
    let mut total_bytes = 0_usize;
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                total_bytes = total_bytes.saturating_add(count);
                let remaining = limit.saturating_sub(output.len());
                let retained = count.min(remaining);
                if let Some(chunk) = buffer.get(..retained) {
                    output.extend_from_slice(chunk);
                }
            }
            Err(error) => {
                return ReaderResult {
                    output: BoundedOutput {
                        bytes: output,
                        total_bytes,
                    },
                    failure: Some(failure(ProcessFailureStage::ReadStdout, error)),
                };
            }
        }
    }
    ReaderResult {
        output: BoundedOutput {
            bytes: output,
            total_bytes,
        },
        failure: None,
    }
}

fn read_tail(mut reader: impl Read, limit: usize) -> ReaderResult {
    let mut output = VecDeque::with_capacity(limit.min(READ_BUFFER_BYTES));
    let mut total_bytes = 0_usize;
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                total_bytes = total_bytes.saturating_add(count);
                if let Some(chunk) = buffer.get(..count) {
                    output.extend(chunk);
                    let excess = output.len().saturating_sub(limit);
                    output.drain(..excess);
                }
            }
            Err(error) => {
                return ReaderResult {
                    output: BoundedOutput {
                        bytes: output.into(),
                        total_bytes,
                    },
                    failure: Some(failure(ProcessFailureStage::ReadStderr, error)),
                };
            }
        }
    }
    ReaderResult {
        output: BoundedOutput {
            bytes: output.into(),
            total_bytes,
        },
        failure: None,
    }
}

fn failure(stage: ProcessFailureStage, error: impl std::fmt::Display) -> ProcessFailure {
    ProcessFailure {
        stage,
        message: error.to_string(),
    }
}

fn report(terminal: ProcessTerminal) -> ProcessReport {
    ProcessReport {
        terminal,
        stdout: BoundedOutput::default(),
        stderr_tail: BoundedOutput::default(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read};

    use super::{ProcessFailureStage, read_head, read_tail};

    #[test]
    fn head_retains_prefix_and_counts_discarded_bytes() {
        let result = read_head(Cursor::new(b"abcdefghij"), 4);
        assert_eq!(result.output.as_bytes(), b"abcd");
        assert_eq!(result.output.total_bytes(), 10);
        assert!(result.output.was_truncated());
        assert_eq!(result.failure, None);
    }

    #[test]
    fn tail_retains_suffix_and_counts_discarded_bytes() {
        let result = read_tail(Cursor::new(b"abcdefghij"), 4);
        assert_eq!(result.output.as_bytes(), b"ghij");
        assert_eq!(result.output.total_bytes(), 10);
        assert!(result.output.was_truncated());
        assert_eq!(result.failure, None);
    }

    #[test]
    fn zero_limits_still_drain_the_streams() {
        let head = read_head(Cursor::new(b"stdout"), 0);
        let tail = read_tail(Cursor::new(b"stderr"), 0);
        assert!(head.output.as_bytes().is_empty());
        assert_eq!(head.output.total_bytes(), 6);
        assert!(tail.output.as_bytes().is_empty());
        assert_eq!(tail.output.total_bytes(), 6);
    }

    #[test]
    fn read_failure_preserves_retained_bytes_and_identifies_the_pipe() {
        let result = read_head(FailingReader::default(), 8);
        assert_eq!(result.output.as_bytes(), b"abc");
        assert_eq!(result.output.total_bytes(), 3);
        assert!(matches!(
            result.failure,
            Some(failure) if failure.stage == ProcessFailureStage::ReadStdout
        ));
    }

    #[derive(Default)]
    struct FailingReader {
        emitted: bool,
    }

    impl Read for FailingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.emitted {
                return Err(io::Error::other("fixture read failure"));
            }
            self.emitted = true;
            buffer
                .get_mut(..3)
                .ok_or_else(|| io::Error::other("fixture buffer is too small"))?
                .copy_from_slice(b"abc");
            Ok(3)
        }
    }
}
