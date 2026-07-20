use std::{
    collections::VecDeque,
    fmt,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use crate::process::ContainedChild;

const PROCESS_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_STDERR_TAIL_BYTES: usize = 16 * 1024;
const STDERR_READ_BUFFER_BYTES: usize = 4 * 1024;
const MICROSECONDS_PER_MILLISECOND: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemuxRequest {
    pub ffmpeg: PathBuf,
    pub input: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemuxTelemetry {
    pub position_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemuxOutcome {
    pub output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemuxFailure {
    pub message: String,
    pub stderr_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemuxTerminal {
    Completed(RemuxOutcome),
    Failed(RemuxFailure),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemuxReport {
    pub terminal: RemuxTerminal,
    pub final_telemetry: Option<RemuxTelemetry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRemuxError(String);

impl fmt::Display for StartRemuxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StartRemuxError {}

#[derive(Clone)]
pub struct RemuxCancellationHandle {
    sender: Sender<()>,
}

impl RemuxCancellationHandle {
    pub fn cancel(&self) {
        let _result = self.sender.send(());
    }
}

pub struct RemuxHandle {
    cancellation: RemuxCancellationHandle,
    result: Receiver<RemuxReport>,
    telemetry: Arc<Mutex<Option<RemuxTelemetry>>>,
    cancel_on_drop: bool,
}

impl RemuxHandle {
    #[must_use]
    pub fn cancellation_handle(&self) -> RemuxCancellationHandle {
        self.cancellation.clone()
    }

    #[must_use]
    pub fn latest_telemetry(&self) -> Option<RemuxTelemetry> {
        match self.telemetry.lock() {
            Ok(telemetry) => *telemetry,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Blocks up to `timeout` for the report; `Ok(None)` means the remux is
    /// still running when the timeout elapses.
    pub fn recv_report(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<RemuxReport>, StartRemuxError> {
        match self.result.recv_timeout(timeout) {
            Ok(report) => {
                self.cancel_on_drop = false;
                Ok(Some(report))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(StartRemuxError(
                "remux worker dropped the result channel".to_owned(),
            )),
        }
    }

    pub fn wait(mut self) -> Result<RemuxReport, StartRemuxError> {
        let report = self.result.recv().map_err(|error| {
            StartRemuxError(format!("remux worker dropped the result: {error}"))
        })?;
        self.cancel_on_drop = false;
        Ok(report)
    }
}

impl Drop for RemuxHandle {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.cancellation.cancel();
        }
    }
}

pub fn start(request: RemuxRequest) -> Result<RemuxHandle, StartRemuxError> {
    validate_request(&request)?;
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let telemetry = Arc::new(Mutex::new(None));
    let worker_telemetry = Arc::clone(&telemetry);
    thread::Builder::new()
        .name("crfty-ffmpeg-remux".to_owned())
        .spawn(move || {
            let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(request, cancel_rx, worker_telemetry)
            }))
            .unwrap_or_else(|_| RemuxReport {
                terminal: RemuxTerminal::Failed(RemuxFailure {
                    message: "remux worker panicked".to_owned(),
                    stderr_tail: String::new(),
                }),
                final_telemetry: None,
            });
            let _result = result_tx.send(report);
        })
        .map_err(|error| StartRemuxError(format!("failed to start remux worker: {error}")))?;
    Ok(RemuxHandle {
        cancellation: RemuxCancellationHandle { sender: cancel_tx },
        result: result_rx,
        telemetry,
        cancel_on_drop: true,
    })
}

fn validate_request(request: &RemuxRequest) -> Result<(), StartRemuxError> {
    if !request.ffmpeg.is_absolute() || !request.ffmpeg.is_file() {
        return Err(StartRemuxError(format!(
            "ffmpeg is not an absolute executable file: {}",
            request.ffmpeg.display()
        )));
    }
    if request.input == request.output {
        return Err(StartRemuxError(
            "remux input and staging output must differ".to_owned(),
        ));
    }
    Ok(())
}

fn run(
    request: RemuxRequest,
    cancellation: Receiver<()>,
    telemetry: Arc<Mutex<Option<RemuxTelemetry>>>,
) -> RemuxReport {
    let mut command = remux_command(&request);
    let mut child = match ContainedChild::spawn(&mut command) {
        Ok(child) => child,
        Err(error) => {
            return failed(
                format!("failed to spawn FFmpeg remux: {error}"),
                String::new(),
                None,
            );
        }
    };
    let Some(stdout) = child.take_stdout() else {
        return failed_after_termination(
            &mut child,
            "FFmpeg progress pipe is missing",
            String::new(),
            None,
        );
    };
    let Some(stderr) = child.take_stderr() else {
        return failed_after_termination(
            &mut child,
            "FFmpeg stderr pipe is missing",
            String::new(),
            None,
        );
    };
    let progress = Arc::new(Mutex::new(ProgressState::default()));
    let progress_reader =
        spawn_progress_reader(stdout, Arc::clone(&progress), Arc::clone(&telemetry));
    let stderr_reader = spawn_stderr_reader(stderr);

    let status = loop {
        match cancellation.recv_timeout(PROCESS_STATUS_POLL_INTERVAL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _status = child.terminate_and_wait();
                let _progress_result = progress_reader.join();
                let _stderr_result = stderr_reader.join();
                return RemuxReport {
                    terminal: RemuxTerminal::Cancelled,
                    final_telemetry: latest_telemetry(&telemetry),
                };
            }
            Err(mpsc::RecvTimeoutError::Timeout) => match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    let _status = child.terminate_and_wait();
                    let stderr_tail = join_stderr(stderr_reader);
                    let _progress_result = progress_reader.join();
                    return failed(
                        format!("failed to wait for FFmpeg remux: {error}"),
                        stderr_tail,
                        latest_telemetry(&telemetry),
                    );
                }
            },
        }
    };

    let progress_joined = progress_reader.join().is_ok();
    let stderr_tail = join_stderr(stderr_reader);
    let final_telemetry = latest_telemetry(&telemetry);
    if !status.success() {
        return failed(
            format!("FFmpeg remux exited with {status}"),
            stderr_tail,
            final_telemetry,
        );
    }
    if !progress_joined {
        return failed(
            "FFmpeg progress reader panicked".to_owned(),
            stderr_tail,
            final_telemetry,
        );
    }
    let progress = match progress.lock() {
        Ok(progress) => progress.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    if let Some(error) = progress.error {
        return failed(error, stderr_tail, final_telemetry);
    }
    if !progress.saw_end {
        return failed(
            "FFmpeg remux ended without progress=end".to_owned(),
            stderr_tail,
            final_telemetry,
        );
    }
    RemuxReport {
        terminal: RemuxTerminal::Completed(RemuxOutcome {
            output: request.output,
        }),
        final_telemetry,
    }
}

fn remux_command(request: &RemuxRequest) -> Command {
    let mut command = Command::new(&request.ffmpeg);
    command
        .args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-loglevel",
            "error",
            "-progress",
            "pipe:1",
            "-y",
            "-i",
        ])
        .arg(&request.input)
        .args([
            "-map",
            "0",
            "-map_metadata",
            "0",
            "-map_chapters",
            "0",
            "-c",
            "copy",
            "-f",
            "matroska",
        ])
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[derive(Clone, Default)]
struct ProgressState {
    saw_end: bool,
    error: Option<String>,
}

fn spawn_progress_reader(
    stdout: std::process::ChildStdout,
    progress: Arc<Mutex<ProgressState>>,
    telemetry: Arc<Mutex<Option<RemuxTelemetry>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => parse_progress_line(&line, &progress, &telemetry),
                Err(error) => {
                    set_progress_error(
                        &progress,
                        format!("failed to read FFmpeg progress: {error}"),
                    );
                    break;
                }
            }
        }
    })
}

fn parse_progress_line(
    line: &str,
    progress: &Mutex<ProgressState>,
    telemetry: &Mutex<Option<RemuxTelemetry>>,
) {
    let Some((key, value)) = line.split_once('=') else {
        return;
    };
    match key {
        "out_time_us" if value != "N/A" => match value.parse::<i64>() {
            Ok(microseconds) => {
                let nonnegative = u64::try_from(microseconds).unwrap_or_default();
                let update = RemuxTelemetry {
                    position_ms: nonnegative / MICROSECONDS_PER_MILLISECOND,
                };
                match telemetry.lock() {
                    Ok(mut telemetry) => *telemetry = Some(update),
                    Err(poisoned) => *poisoned.into_inner() = Some(update),
                }
            }
            Err(error) => set_progress_error(
                progress,
                format!("invalid FFmpeg out_time_us value: {error}"),
            ),
        },
        "progress" if value == "end" => match progress.lock() {
            Ok(mut progress) => progress.saw_end = true,
            Err(poisoned) => poisoned.into_inner().saw_end = true,
        },
        _ => {}
    }
}

fn set_progress_error(progress: &Mutex<ProgressState>, error: String) {
    match progress.lock() {
        Ok(mut progress) => progress.error = Some(error),
        Err(poisoned) => poisoned.into_inner().error = Some(error),
    }
}

fn spawn_stderr_reader(stderr: std::process::ChildStderr) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buffer = vec![0_u8; STDERR_READ_BUFFER_BYTES];
        let mut tail: VecDeque<u8> = VecDeque::with_capacity(MAX_STDERR_TAIL_BYTES);
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if let Some(chunk) = buffer.get(..count) {
                        tail.extend(chunk);
                        while tail.len() > MAX_STDERR_TAIL_BYTES {
                            let _discarded = tail.pop_front();
                        }
                    }
                }
                Err(error) => {
                    let context = format!("failed to read FFmpeg stderr: {error}");
                    tail.extend(context.as_bytes());
                    break;
                }
            }
        }
        String::from_utf8_lossy(&tail.into_iter().collect::<Vec<_>>()).into_owned()
    })
}

fn join_stderr(reader: thread::JoinHandle<String>) -> String {
    reader
        .join()
        .unwrap_or_else(|_| "FFmpeg stderr reader panicked".to_owned())
}

fn latest_telemetry(telemetry: &Mutex<Option<RemuxTelemetry>>) -> Option<RemuxTelemetry> {
    match telemetry.lock() {
        Ok(telemetry) => *telemetry,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

fn failed(
    message: String,
    stderr_tail: String,
    final_telemetry: Option<RemuxTelemetry>,
) -> RemuxReport {
    RemuxReport {
        terminal: RemuxTerminal::Failed(RemuxFailure {
            message,
            stderr_tail,
        }),
        final_telemetry,
    }
}

fn failed_after_termination(
    child: &mut ContainedChild,
    message: &str,
    stderr_tail: String,
    final_telemetry: Option<RemuxTelemetry>,
) -> RemuxReport {
    let _status = child.terminate_and_wait();
    failed(message.to_owned(), stderr_tail, final_telemetry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_progress_uses_microseconds_and_requires_end() {
        let progress = Mutex::new(ProgressState::default());
        let telemetry = Mutex::new(None);
        parse_progress_line("out_time_us=1234567", &progress, &telemetry);
        parse_progress_line("unknown=value", &progress, &telemetry);
        parse_progress_line("progress=end", &progress, &telemetry);

        assert_eq!(
            latest_telemetry(&telemetry),
            Some(RemuxTelemetry { position_ms: 1234 })
        );
        let progress = progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(progress.saw_end);
        assert!(progress.error.is_none());
    }

    #[test]
    fn invalid_required_progress_value_is_recorded() {
        let progress = Mutex::new(ProgressState::default());
        let telemetry = Mutex::new(None);
        parse_progress_line("out_time_us=invalid", &progress, &telemetry);
        let progress = progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(progress.error.is_some());
    }

    #[test]
    fn command_preserves_every_stream_in_matroska() {
        let request = RemuxRequest {
            ffmpeg: PathBuf::from("ffmpeg"),
            input: PathBuf::from("input.mp4"),
            output: PathBuf::from(".output.mkv.crfty-1.part"),
        };
        let command = remux_command(&request);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            vec![
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-loglevel",
                "error",
                "-progress",
                "pipe:1",
                "-y",
                "-i",
                "input.mp4",
                "-map",
                "0",
                "-map_metadata",
                "0",
                "-map_chapters",
                "0",
                "-c",
                "copy",
                "-f",
                "matroska",
                ".output.mkv.crfty-1.part",
            ]
        );
    }
}
