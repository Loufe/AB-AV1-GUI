#[cfg(feature = "contract-test-fixture")]
use super::FaultInjection;
use super::{
    EncodeRequest, EncodeTelemetry, JobFailure, JobTerminal, SearchOutcome, SearchRequest,
    SearchTelemetry, Telemetry, protocol::SearchProtocol, tool_path,
};
use super::{JobFailureKind, ProcessFailureKind};
use crate::{
    failure::scrub_tail,
    process_supervisor::{
        self, ProcessCancellation, ProcessLimits, ProcessReport, ProcessTerminal,
    },
    tools::MediaTools,
};
use crfty_core::DecodeMode;
use std::{
    process::Command,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

const STDERR_BYTES: usize = 64 * 1024;
const PROGRESS_LINE_BYTES: usize = 4096;
const KEYFRAME_INTERVAL: &str = "240";
const PIXEL_FORMAT: &str = "yuv420p10le";

pub(crate) fn run_search(
    tools: MediaTools,
    request: SearchRequest,
    cancellation: ProcessCancellation,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
) -> JobTerminal<SearchOutcome> {
    run_search_limited(tools, request, cancellation, telemetry, None, None)
}

pub(crate) fn run_search_limited(
    tools: MediaTools,
    request: SearchRequest,
    cancellation: ProcessCancellation,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    timeout: Option<Duration>,
    fixed_crf: Option<f32>,
) -> JobTerminal<SearchOutcome> {
    let temporary = match tempfile::Builder::new().prefix("crfty-search-").tempdir() {
        Ok(directory) => directory,
        Err(error) => {
            return JobTerminal::Failed(JobFailure::new(format!(
                "cannot create search workspace: {error}"
            )));
        }
    };
    let mut command = search_command(&tools, &request, temporary.path());
    if let Err(error) = tool_path::configure(&mut command, &tools, temporary.path()) {
        return JobTerminal::Failed(JobFailure::new(error));
    }
    if let Some(crf) = fixed_crf {
        command
            .arg("--min-crf")
            .arg(crf.to_string())
            .arg("--max-crf")
            .arg((crf + 1.0).to_string());
    }
    let protocol = Arc::new(Mutex::new(SearchProtocol::default()));
    let reader_protocol = Arc::clone(&protocol);
    let report = process_supervisor::run_streaming(
        &mut command,
        &cancellation,
        ProcessLimits::new(timeout, 0, STDERR_BYTES),
        redactions(&request.input, &[temporary.path()]),
        move |bytes| {
            let mut protocol = lock(&reader_protocol);
            protocol.push(bytes)?;
            if let Some(last) = &protocol.last {
                *lock(&telemetry) = Some(Telemetry::Search(SearchTelemetry {
                    completed_trials: protocol.trials,
                    last: last.clone(),
                }));
            }
            Ok(())
        },
    );
    let mut protocol = lock(&protocol);
    let terminal = match &report.terminal {
        ProcessTerminal::Cancelled => JobTerminal::Cancelled,
        ProcessTerminal::Success(_) | ProcessTerminal::ToolFailed(_) => match protocol.finish() {
            Err(error) => JobTerminal::Failed(JobFailure::new(error)),
            Ok(()) => match protocol.terminal.take() {
                Some(Ok(result)) if matches!(report.terminal, ProcessTerminal::Success(_)) => {
                    JobTerminal::Completed(result)
                }
                Some(Err(failure)) if matches!(report.terminal, ProcessTerminal::ToolFailed(_)) => {
                    JobTerminal::Failed(failure)
                }
                None if matches!(report.terminal, ProcessTerminal::ToolFailed(_)) => {
                    process_failure(&report)
                }
                _ => JobTerminal::Failed(JobFailure::new(
                    "ab-av1 exit status and JSON terminal result disagree",
                )),
            },
        },
        _ => process_failure(&report),
    };
    let terminal = diagnostic(
        terminal,
        &report,
        &[
            (request.input.as_path(), "<input>"),
            (temporary.path(), "<temporary>"),
        ],
    );
    match temporary.close() {
        Ok(()) => terminal,
        Err(error) => append_cleanup(
            terminal,
            format!("search workspace cleanup failed: {:?}", error.kind()),
        ),
    }
}

fn search_command(
    tools: &MediaTools,
    request: &SearchRequest,
    directory: &std::path::Path,
) -> Command {
    let mut command = Command::new(&tools.ab_av1);
    command
        .args([
            "crf-search",
            "--stdout-format",
            "json",
            "--encoder",
            "libsvtav1",
            "--pix-format",
            PIXEL_FORMAT,
            "--keyint",
            KEYFRAME_INTERVAL,
            "--scd",
            "false",
            "--cache",
            "false",
            "--input",
        ])
        .arg(&request.input)
        .arg("--min-vmaf")
        .arg(request.target_vmaf.to_string())
        .arg("--max-encoded-percent")
        .arg(request.max_encoded_percent.to_string())
        .arg("--preset")
        .arg(request.preset.to_string())
        .arg("--sample-duration")
        .arg(format!("{}ms", request.sample_duration.as_millis()))
        .arg("--temp-dir")
        .arg(directory)
        .current_dir(directory);
    if let Some(samples) = request.samples {
        command.arg("--samples").arg(samples.to_string());
    }
    if request.thorough {
        command.arg("--thorough");
    }
    if let DecodeMode::Hardware(decoder) = request.decode_mode {
        command
            .arg("--enc-input")
            .arg(format!("c:v={}", crate::media::decoder_name(decoder)));
    }
    command
}

pub(crate) fn run_encode(
    tools: MediaTools,
    request: EncodeRequest,
    cancellation: ProcessCancellation,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    #[cfg(feature = "contract-test-fixture")] fault: FaultInjection,
) -> JobTerminal<()> {
    let existed = request.output.exists();
    let mut command = encode_command(&tools, &request);
    let state = Arc::new(Mutex::new(EncodeProgress::default()));
    let reader_state = Arc::clone(&state);
    let report = process_supervisor::run_streaming(
        &mut command,
        &cancellation,
        ProcessLimits::new(None, 0, STDERR_BYTES),
        redactions(&request.input, &[request.output.as_path()]),
        move |bytes| {
            let mut state = lock(&reader_state);
            state.push(bytes)?;
            *lock(&telemetry) = Some(Telemetry::Encode(state.update.clone()));
            #[cfg(feature = "contract-test-fixture")]
            if fault == FaultInjection::PanicAfterFirstProgress && state.update.frame > 0 {
                panic!("injected encode telemetry panic");
            }
            Ok(())
        },
    );
    let mut terminal = match &report.terminal {
        ProcessTerminal::Success(_) if lock(&state).finish().is_ok() => JobTerminal::Completed(()),
        ProcessTerminal::Success(_) => JobTerminal::Failed(JobFailure::new(
            "FFmpeg exited without a complete progress report",
        )),
        ProcessTerminal::Cancelled => JobTerminal::Cancelled,
        _ => process_failure(&report),
    };
    #[cfg(feature = "contract-test-fixture")]
    if fault == FaultInjection::PanicAfterFirstProgress && lock(&state).update.frame > 0 {
        terminal = JobTerminal::Panicked {
            cleanup_failure: match &report.terminal {
                ProcessTerminal::CleanupFailed(failure) => Some(failure.message.clone()),
                _ => None,
            },
        };
    }
    if !matches!(terminal, JobTerminal::Completed(_))
        && !existed
        && let Err(error) = std::fs::remove_file(&request.output)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        terminal = append_cleanup(
            terminal,
            format!("encode artifact cleanup failed: {:?}", error.kind()),
        );
    }
    diagnostic(
        terminal,
        &report,
        &[
            (request.input.as_path(), "<input>"),
            (request.output.as_path(), "<staging>"),
        ],
    )
}

fn encode_command(tools: &MediaTools, request: &EncodeRequest) -> Command {
    let mut command = Command::new(&tools.ffmpeg);
    command.args([
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
    ]);
    if let DecodeMode::Hardware(decoder) = request.decode_mode {
        command.args(["-c:v", crate::media::decoder_name(decoder)]);
    }
    command
        .arg("-i")
        .arg(&request.input)
        .args([
            "-map",
            "0",
            "-map",
            "-0:d",
            "-c",
            "copy",
            "-c:v:0",
            "libsvtav1",
            "-pix_fmt:v:0",
            PIXEL_FORMAT,
            "-g:v:0",
            KEYFRAME_INTERVAL,
            "-preset:v:0",
        ])
        .arg(request.preset.to_string())
        .arg("-crf:v:0")
        .arg(request.crf.min(63.0).to_string())
        .arg("-svtav1-params:v:0")
        .arg(format!("scd=0:crf={}", request.crf))
        .arg(&request.output);
    command
}

#[derive(Default)]
struct EncodeProgress {
    pending: Vec<u8>,
    update: EncodeTelemetry,
    saw_end: bool,
}
impl EncodeProgress {
    fn finish(&self) -> Result<(), String> {
        if self.saw_end && self.pending.is_empty() {
            Ok(())
        } else {
            Err("FFmpeg progress stream is incomplete".into())
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        for &byte in bytes {
            if self.saw_end {
                return Err("FFmpeg wrote data after progress=end".into());
            }
            if byte != b'\n' {
                if self.pending.len() >= PROGRESS_LINE_BYTES {
                    return Err("FFmpeg progress line is too long".into());
                }
                self.pending.push(byte);
                continue;
            }
            let line =
                std::str::from_utf8(&self.pending).map_err(|_| "invalid FFmpeg progress text")?;
            if let Some((key, value)) = line.trim_end_matches('\r').split_once('=') {
                match key {
                    "frame" => {
                        self.update.frame = value
                            .trim()
                            .parse()
                            .map_err(|_| "invalid FFmpeg frame count")?
                    }
                    "fps" if value != "N/A" => {
                        let fps: f32 = value.parse().map_err(|_| "invalid FFmpeg fps")?;
                        if !fps.is_finite() || fps < 0.0 {
                            return Err("invalid FFmpeg fps".into());
                        }
                        self.update.fps = fps;
                    }
                    "out_time_us" if value != "N/A" => {
                        let value: i64 = value.parse().map_err(|_| "invalid FFmpeg output time")?;
                        self.update.position =
                            Duration::from_micros(u64::try_from(value).unwrap_or_default());
                    }
                    "progress" if value == "end" => self.saw_end = true,
                    _ => {}
                }
            }
            self.pending.clear();
        }
        Ok(())
    }
}
fn process_failure<T>(report: &ProcessReport) -> JobTerminal<T> {
    let message = match &report.terminal {
        ProcessTerminal::ToolFailed(status) => {
            format!("media tool failed with exit status {status}")
        }
        ProcessTerminal::SpawnFailed(failure)
        | ProcessTerminal::SupervisionFailed(failure)
        | ProcessTerminal::CleanupFailed(failure) => failure.message.clone(),
        ProcessTerminal::TimedOut => "media tool exceeded its deadline".into(),
        _ => "media tool did not complete".into(),
    };
    let kind = match &report.terminal {
        ProcessTerminal::ToolFailed(status) => ProcessFailureKind::Rejected {
            code: status.code(),
        },
        ProcessTerminal::SpawnFailed(_) => ProcessFailureKind::Unavailable,
        ProcessTerminal::TimedOut => ProcessFailureKind::TimedOut,
        ProcessTerminal::CleanupFailed(_) => ProcessFailureKind::Cleanup,
        _ => ProcessFailureKind::Supervision,
    };
    let mut failure = JobFailure::new(message);
    failure.kind = JobFailureKind::Process { kind };
    JobTerminal::Failed(failure)
}
fn diagnostic<T>(
    terminal: JobTerminal<T>,
    report: &ProcessReport,
    paths: &[(&std::path::Path, &str)],
) -> JobTerminal<T> {
    match terminal {
        JobTerminal::Failed(mut failure) => {
            failure.diagnostic = scrub_tail(
                &String::from_utf8_lossy(report.stderr_tail.as_bytes()),
                paths,
            );
            JobTerminal::Failed(failure)
        }
        terminal => terminal,
    }
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn redactions(input: &std::path::Path, other: &[&std::path::Path]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut patterns = vec![(native_bytes(input.as_os_str()), b"<input>".to_vec())];
    if let Some(stem) = input.file_stem() {
        patterns.push((native_bytes(stem), b"<media>".to_vec()));
    }
    patterns.extend(
        other
            .iter()
            .map(|path| (native_bytes(path.as_os_str()), b"<temporary>".to_vec())),
    );
    patterns
}
fn native_bytes(value: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        value.to_string_lossy().as_bytes().to_vec()
    }
}
fn append_cleanup<T>(terminal: JobTerminal<T>, cleanup: String) -> JobTerminal<T> {
    match terminal {
        JobTerminal::Failed(mut failure) => {
            failure.message = format!("{}; {cleanup}", failure.message);
            JobTerminal::Failed(failure)
        }
        JobTerminal::Panicked { cleanup_failure } => JobTerminal::Panicked {
            cleanup_failure: Some(
                cleanup_failure.map_or(cleanup.clone(), |prior| format!("{prior}; {cleanup}")),
            ),
        },
        terminal => {
            let mut failure = JobFailure::new(match terminal {
                JobTerminal::Cancelled => format!("operation was cancelled; {cleanup}"),
                _ => cleanup,
            });
            failure.kind = JobFailureKind::Process {
                kind: ProcessFailureKind::Cleanup,
            };
            JobTerminal::Failed(failure)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoding_progress_requires_a_complete_final_record() -> Result<(), String> {
        let mut progress = EncodeProgress::default();
        progress.push(b"frame=12\nfps=3.5\nout_time_us=2500000\nprogress=end\n")?;
        progress.finish()?;
        assert_eq!(progress.update.frame, 12);
        assert_eq!(progress.update.position.as_millis(), 2500);
        assert!(progress.push(b"trailing").is_err());
        let mut incomplete = EncodeProgress::default();
        incomplete.push(b"progress=en")?;
        assert!(incomplete.finish().is_err());
        Ok(())
    }
    #[test]
    fn cleanup_failure_preserves_the_primary_kind_and_diagnostic() {
        let mut failure = JobFailure::new("protocol rejected");
        failure.kind = JobFailureKind::Process {
            kind: ProcessFailureKind::Rejected { code: Some(9) },
        };
        failure.diagnostic = crfty_core::DiagnosticTail::truncated("useful diagnostic");
        let terminal: JobTerminal<()> = append_cleanup(
            JobTerminal::Failed(failure.clone()),
            "workspace removal failed".into(),
        );
        let JobTerminal::Failed(result) = terminal else {
            panic!("missing primary failure");
        };
        assert_eq!(result.kind, failure.kind);
        assert_eq!(result.diagnostic, failure.diagnostic);
        assert!(result.message.contains("protocol rejected"));
        assert!(result.message.contains("workspace removal failed"));
    }
}
