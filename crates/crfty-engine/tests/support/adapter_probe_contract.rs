use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    thread,
    time::Duration,
};

use crfty_engine::{
    ab_av1::{AbAv1Runtime, CancelMode, JobFailureKind, JobTerminal, ProbeFailureKind},
    tools::MediaTools,
};

const DIAGNOSTIC_NOISE_BYTES: usize = 32 * 1024;
const OVERSIZED_JSON_BYTES: usize = 2 * 1024 * 1024;
const HEARTBEAT_SETTLEMENT_CHECK: Duration = Duration::from_millis(200);

pub(super) fn fake_probe(input: &Path) -> Result<bool, Box<dyn Error>> {
    let name = input
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let hang = name.starts_with("adapter-closed-hang")
        || name.starts_with("adapter-entry-hang")
        || (name.starts_with("adapter-sample-hang") && name.contains(".crf"));
    let orphan = name.starts_with("adapter-orphan");
    if hang || orphan {
        let marker = marker()?;
        fs::write(marker.with_extension("pid"), process::id().to_string())?;
        let mut descendant = Command::new(env::current_exe()?);
        descendant.arg("heartbeat").arg(&marker);
        if name.starts_with("adapter-orphan-closed") || name.starts_with("adapter-closed-hang") {
            descendant.stdout(Stdio::null()).stderr(Stdio::null());
        }
        let _child = descendant.spawn()?;
        super::wait_for_file(&marker)?;
        if orphan {
            return Ok(false);
        }
        let mut stderr = io::stderr().lock();
        stderr.write_all(&vec![b'x'; DIAGNOSTIC_NOISE_BYTES])?;
        stderr.write_all(b"fixture probe diagnostic tail")?;
        stderr.flush()?;
        drop(stderr);
        #[cfg(unix)]
        if name.starts_with("adapter-closed-hang") {
            use std::os::unix::process::CommandExt;
            return Err(Command::new(env::current_exe()?)
                .arg("heartbeat")
                .arg(&marker)
                .arg("closed")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .exec()
                .into());
        }
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    if name.starts_with("adapter-rejected")
        || (name.starts_with("adapter-sample-rejected") && name.contains(".crf"))
    {
        let mut stderr = io::stderr().lock();
        stderr.write_all(&vec![b'x'; DIAGNOSTIC_NOISE_BYTES])?;
        stderr.write_all(b"fixture probe diagnostic tail")?;
        write!(stderr, " input: {}", input.display())?;
        if name.contains("boundary") {
            let end = b"fixture probe diagnostic tail";
            stderr.write_all(&vec![
                b'y';
                crfty_core::DIAGNOSTIC_TAIL_MAX_BYTES - 16 - end.len()
            ])?;
            stderr.write_all(end)?;
            // The cap-crossing suffix must not include dispatch's error prose.
            stderr.flush()?;
            process::exit(1);
        }
        return Err("fixture probe rejected".into());
    }
    if name.starts_with("adapter-invalid") {
        io::stdout().write_all(b"not a JSON document")?;
        return Ok(true);
    }
    if name.starts_with("adapter-oversized") {
        io::stdout().write_all(&vec![b'x'; OVERSIZED_JSON_BYTES])?;
        return Ok(true);
    }
    Ok(false)
}

fn marker() -> Result<PathBuf, Box<dyn Error>> {
    env::var_os("CRFTY_ADAPTER_PROBE_MARKER")
        .map(PathBuf::from)
        .ok_or_else(|| "adapter probe marker is missing".into())
}

pub(super) fn run(mut arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn Error>> {
    let mode = arguments.next().ok_or("probe contract mode missing")?;
    let directory = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("fixture directory missing")?;
    let tools = MediaTools {
        ffmpeg: arguments
            .next()
            .map(PathBuf::from)
            .ok_or("ffmpeg path missing")?,
        ffprobe: arguments
            .next()
            .map(PathBuf::from)
            .ok_or("ffprobe path missing")?,
    };
    let name = match mode.to_str().ok_or("invalid fixture mode")? {
        "cancel-search-entry"
        | "cancel-encode-entry"
        | "timeout-entry"
        | "cancel-fifo"
        | "timeout-fifo" => "adapter-entry-hang.mkv",
        "cancel-sample-probe" | "timeout-sample" => "adapter-sample-hang.mkv",
        "cancel-orphan-probe" => "adapter-orphan.mkv",
        "closed-orphan-probe" => "adapter-orphan-closed.mkv",
        "reject" => "adapter-rejected.mkv",
        "reject-boundary" => "adapter-rejected-boundary.mkv",
        "reject-sample-boundary" => "adapter-sample-rejected-boundary.mkv",
        "reject-sample" => "adapter-sample-rejected.mkv",
        "cancel-closed-probe" | "timeout-closed-probe" => "adapter-closed-hang.mkv",
        "invalid" => "adapter-invalid.mkv",
        "oversized" => "adapter-oversized.mkv",
        _ => return Err("unknown adapter probe mode".into()),
    };
    let input = directory.join(name);
    if !mode.to_string_lossy().ends_with("-fifo") {
        super::write_distinct_media(&input, 17)?;
    }
    let runtime = AbAv1Runtime::start()?;
    let is_search = mode == "cancel-search-entry"
        || mode == "cancel-sample-probe"
        || mode == "timeout-sample"
        || mode == "reject-sample"
        || mode == "reject-sample-boundary";
    let terminal = if is_search {
        let job = runtime.start_search(tools.clone(), super::search_request(&input))?;
        if mode.to_string_lossy().starts_with("cancel-") {
            super::wait_for_file(&marker()?)?;
            if mode == "cancel-closed-probe" {
                super::wait_for_file(&marker()?.with_extension("closed"))?;
            }
            job.cancel(CancelMode::Force);
        }
        completion_only(job.wait()?.terminal)
    } else {
        let job = runtime.start_encode(
            tools.clone(),
            super::encode_request(&input, &directory.join("attempt.mkv")),
        )?;
        if mode.to_string_lossy().starts_with("cancel-") {
            super::wait_for_file(&marker()?)?;
            if mode == "cancel-closed-probe" {
                super::wait_for_file(&marker()?.with_extension("closed"))?;
            }
            job.cancel(CancelMode::Force);
        }
        job.wait()?.terminal
    };
    if mode.to_string_lossy().starts_with("cancel-") {
        if terminal != JobTerminal::Cancelled {
            return Err(format!("probe cancellation failed: {terminal:?}").into());
        }
        verify_settlement()?;
    } else if mode == "closed-orphan-probe" {
        if terminal != JobTerminal::Completed(()) {
            return Err(format!("closed-pipe probe did not complete: {terminal:?}").into());
        }
        verify_settlement()?;
    } else {
        let expected = if mode.to_string_lossy().starts_with("reject") {
            ProbeFailureKind::Rejected { code: Some(1) }
        } else if mode == "invalid" || mode == "oversized" {
            ProbeFailureKind::InvalidOutput
        } else {
            ProbeFailureKind::TimedOut
        };
        let JobTerminal::Failed(failure) = terminal else {
            return Err(format!("probe did not fail: {terminal:?}").into());
        };
        if failure.kind != (JobFailureKind::Probe { kind: expected }) {
            return Err(format!("probe failure lost its type: {failure:?}").into());
        }
        if (mode == "reject"
            || mode == "reject-sample"
            || mode.to_string_lossy().starts_with("timeout-"))
            && (!failure
                .diagnostic
                .as_str()
                .contains("fixture probe diagnostic tail")
                || !failure.message.contains("diagnostic tail truncated")
                || failure.diagnostic.as_str().len() > crfty_core::DIAGNOSTIC_TAIL_MAX_BYTES)
        {
            return Err(format!("probe diagnostic was lost or unbounded: {failure:?}").into());
        }
        if (mode.to_string_lossy().starts_with("reject"))
            && (!failure.diagnostic.as_str().contains("<probe-input>")
                || failure
                    .diagnostic
                    .as_str()
                    .contains(&directory.to_string_lossy().to_string()))
        {
            return Err(format!("probe diagnostic exposed its path: {failure:?}").into());
        }
        if mode.to_string_lossy().starts_with("timeout-") {
            verify_settlement()?;
        }
    }
    let valid = directory.join("valid.mkv");
    super::write_distinct_media(&valid, 19)?;
    super::successful_search(&runtime, &tools, &valid)?;
    super::verify_encode(
        runtime
            .start_encode(
                tools,
                super::encode_request(&valid, &directory.join("after-probe.mkv")),
            )?
            .wait()?
            .terminal,
        &directory.join("after-probe.mkv"),
    )?;
    runtime.shutdown()?;
    Ok(())
}

fn completion_only<T>(terminal: JobTerminal<T>) -> JobTerminal<()> {
    match terminal {
        JobTerminal::Completed(_) => JobTerminal::Completed(()),
        JobTerminal::Failed(failure) => JobTerminal::Failed(failure),
        JobTerminal::Cancelled => JobTerminal::Cancelled,
        JobTerminal::Panicked { cleanup_failure } => JobTerminal::Panicked { cleanup_failure },
    }
}

fn verify_settlement() -> Result<(), Box<dyn Error>> {
    let marker = marker()?;
    let stopped = fs::read(&marker)?;
    thread::sleep(HEARTBEAT_SETTLEMENT_CHECK);
    if fs::read(&marker)? != stopped {
        return Err("probe left a live descendant".into());
    }
    let pid = fs::read_to_string(marker.with_extension("pid"))?;
    #[cfg(target_os = "linux")]
    if Path::new("/proc").join(pid.trim()).exists() {
        return Err("probe leader was not reaped".into());
    }
    #[cfg(windows)]
    {
        let tasklist = PathBuf::from(env::var_os("SystemRoot").ok_or("SystemRoot missing")?)
            .join("System32/tasklist.exe");
        let output = Command::new(tasklist)
            .args([
                "/FI",
                &format!("PID eq {}", pid.trim()),
                "/FO",
                "CSV",
                "/NH",
            ])
            .output()?;
        if !output.status.success()
            || String::from_utf8_lossy(&output.stdout).contains(&format!("\"{}\"", pid.trim()))
        {
            return Err("probe leader still present".into());
        }
    }
    Ok(())
}
