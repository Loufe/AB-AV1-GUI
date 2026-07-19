#![forbid(unsafe_code)]

use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    thread,
    time::Duration,
};

use crfty_core::{
    AnalysisProfile, DEFAULT_VMAF_TARGET, DecodeMode, DecodePreference, ExecutionSettings,
    MIN_VMAF_FALLBACK_TARGET, Operation, OutputTarget, QueueCommand, QueueItemId, SessionCommand,
    VMAF_FALLBACK_STEP,
};

const CONTRACT_PRESET: u8 = 12;
const CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS: u32 = 50_000;
const CONTRACT_SAMPLE_COUNT: u64 = 1;
const CONTRACT_SAMPLE_DURATION_MS: u64 = 1_000;
const CONTRACT_EXPECTED_FINISHED_ITEMS: u8 = 2;
const CONTRACT_EVENT_TIMEOUT: Duration = Duration::from_secs(10);
use crfty_engine::ab_av1::{
    AbAv1Runtime, EncodeOutcome, EncodeRequest, FaultInjection, JobHandle, JobTerminal, MediaTools,
    SearchRequest, StartJobError,
};
use crfty_engine::coordinator::{EngineConfig, EngineRuntime};

fn main() {
    if let Err(error) = dispatch() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn dispatch() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os();
    let executable = arguments.next().ok_or("executable path is missing")?;
    if arguments.next().as_deref() == Some(OsString::from("heartbeat").as_os_str()) {
        let path = arguments
            .next()
            .map(PathBuf::from)
            .ok_or("heartbeat path is missing")?;
        return heartbeat(&path);
    }

    let stem = Path::new(&executable)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if stem.eq_ignore_ascii_case("ffprobe") {
        return fake_ffprobe();
    }
    if stem.eq_ignore_ascii_case("ffmpeg") {
        return fake_ffmpeg();
    }

    let mut arguments = env::args_os();
    let _executable = arguments.next();
    match arguments.next().as_deref() {
        Some(argument) if argument == "run" => run_contract(arguments),
        Some(argument) if argument == "coordinate" => run_coordinator_contract(arguments),
        _ => Err("expected: crfty-contract-fixture run INPUT OUTPUT-DIR FFMPEG FFPROBE".into()),
    }
}

fn fake_ffprobe() -> Result<(), Box<dyn Error>> {
    const PROBE: &str = r#"{
        "streams": [{
            "index": 0,
            "codec_name": "h264",
            "codec_type": "video",
            "codec_tag_string": "[0][0][0][0]",
            "codec_tag": "0x0000",
            "r_frame_rate": "30/1",
            "avg_frame_rate": "30/1",
            "time_base": "1/1000",
            "width": 1280,
            "height": 720,
            "pix_fmt": "yuv420p",
            "disposition": {
                "default": 0,
                "dub": 0,
                "original": 0,
                "comment": 0,
                "lyrics": 0,
                "karaoke": 0,
                "forced": 0,
                "hearing_impaired": 0,
                "visual_impaired": 0,
                "clean_effects": 0,
                "attached_pic": 0,
                "timed_thumbnails": 0
            }
        }],
        "format": {
            "filename": "fixture.mkv",
            "nb_streams": 1,
            "nb_programs": 0,
            "format_name": "matroska",
            "format_long_name": "Matroska",
            "duration": "10",
            "size": "8192",
            "probe_score": 100
        }
    }"#;
    let verifies_staging = env::args_os().any(|argument| {
        Path::new(&argument)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".part"))
    });
    let probe = if verifies_staging {
        PROBE.replace("\"codec_name\": \"h264\"", "\"codec_name\": \"av1\"")
    } else {
        PROBE.to_owned()
    };
    let mut stdout = io::stdout().lock();
    stdout.write_all(probe.as_bytes())?;
    stdout.write_all(b"\n")?;
    Ok(())
}

fn run_coordinator_contract(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<(), Box<dyn Error>> {
    let input = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("input path is missing")?;
    let output_dir = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("output directory is missing")?;
    let tools = MediaTools {
        ffmpeg: arguments
            .next()
            .map(PathBuf::from)
            .ok_or("ffmpeg path is missing")?,
        ffprobe: arguments
            .next()
            .map(PathBuf::from)
            .ok_or("ffprobe path is missing")?,
    };
    let engine = EngineRuntime::start(EngineConfig {
        journal_path: output_dir.join("coordinator.jsonl"),
        media_tools: tools,
        execution: ExecutionSettings {
            requested_target: DEFAULT_VMAF_TARGET,
            fallback_floor: MIN_VMAF_FALLBACK_TARGET,
            fallback_step: VMAF_FALLBACK_STEP,
            overwrite_existing: false,
            decode_preference: DecodePreference::SoftwareOnly,
            profile: AnalysisProfile {
                preset: CONTRACT_PRESET,
                max_encoded_percent_basis_points: CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS,
                samples: Some(CONTRACT_SAMPLE_COUNT),
                sample_duration_ms: CONTRACT_SAMPLE_DURATION_MS,
                thorough: false,
                decode_mode: DecodeMode::Software,
                ab_av1_revision: "contract".to_owned(),
                ffmpeg_revision: "contract".to_owned(),
                encoder_revision: "contract".to_owned(),
            },
        },
    })?;
    let _snapshot = engine.events.recv()?;
    accepted_reply(engine.commands.submit_queue(QueueCommand::Add {
        item_id: QueueItemId(1),
        input: input.clone(),
        operation: Operation::Analyze,
        output_target: OutputTarget::Replace,
    })?)?;
    accepted_reply(engine.commands.submit_queue(QueueCommand::Add {
        item_id: QueueItemId(2),
        input: input.clone(),
        operation: Operation::Convert,
        output_target: OutputTarget::Suffix {
            suffix: "_coordinated".to_owned(),
        },
    })?)?;
    accepted_reply(engine.commands.submit_session(SessionCommand::Start)?)?;

    let mut finished = 0_u8;
    let mut searches = 0_u8;
    let mut prepared_with_reuse = 0_u8;
    let mut first_content = None;
    let mut content_changed = false;
    let mut first_profile = None;
    let mut profile_changed = false;
    while finished < CONTRACT_EXPECTED_FINISHED_ITEMS {
        match engine.events.recv_timeout(CONTRACT_EVENT_TIMEOUT)? {
            crfty_engine::driver::DriverEvent::Durable(
                crfty_core::DurableDelta::MediaObserved { observation },
            ) => {
                if let Some(first) = &first_content {
                    content_changed |= first != &observation.binding.content_key;
                } else {
                    first_content = Some(observation.binding.content_key.clone());
                }
            }
            crfty_engine::driver::DriverEvent::Durable(
                crfty_core::DurableDelta::ItemPrepared { spec },
            ) => {
                if let Some(first) = &first_profile {
                    profile_changed |= first != &spec.execution.profile;
                } else {
                    first_profile = Some(spec.execution.profile.clone());
                }
                if spec.selected_analysis.is_some() {
                    prepared_with_reuse = prepared_with_reuse.saturating_add(1);
                }
            }
            crfty_engine::driver::DriverEvent::Durable(
                crfty_core::DurableDelta::AnalysisRecorded { .. },
            ) => searches = searches.saturating_add(1),
            crfty_engine::driver::DriverEvent::Durable(
                crfty_core::DurableDelta::ItemFinished { .. },
            ) => finished = finished.saturating_add(1),
            crfty_engine::driver::DriverEvent::Ephemeral(
                crfty_core::EphemeralDelta::CommandRejected { reason },
            ) => return Err(format!("coordinator command rejected: {reason}").into()),
            crfty_engine::driver::DriverEvent::Ephemeral(
                crfty_core::EphemeralDelta::WorkerCrashed { message },
            )
            | crfty_engine::driver::DriverEvent::Fatal { message } => {
                return Err(format!("coordinator worker failed: {message}").into());
            }
            _ => {}
        }
    }
    if searches != 1 {
        return Err(format!(
            "analysis reuse expected one search, observed {searches}; prepared reuse count {prepared_with_reuse}; content changed {content_changed}; profile changed {profile_changed}"
        )
        .into());
    }
    let expected = input.with_file_name("input_coordinated.mkv");
    if !expected.exists() {
        return Err(format!("coordinator did not promote {}", expected.display()).into());
    }
    wait_for_idle(&engine)?;
    accepted_reply(engine.commands.submit_queue(QueueCommand::Add {
        item_id: QueueItemId(3),
        input: input.clone(),
        operation: Operation::Convert,
        output_target: OutputTarget::Suffix {
            suffix: "_cancelled".to_owned(),
        },
    })?)?;
    accepted_reply(engine.commands.submit_session(SessionCommand::Start)?)?;
    wait_for_encoding(&engine)?;
    accepted_reply(engine.commands.submit_session(SessionCommand::ForceStop)?)?;
    wait_for_stopped(&engine)?;
    if input.with_file_name("input_cancelled.mkv").exists() {
        return Err("force-stopped coordinator promoted an output".into());
    }
    let partial_remains = fs::read_dir(&output_dir)?.any(|entry| {
        entry.ok().is_some_and(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "part")
        })
    });
    if partial_remains {
        return Err("force-stopped coordinator left a staging file".into());
    }
    engine.shutdown()?;
    Ok(())
}

fn wait_for_idle(engine: &EngineRuntime) -> Result<(), Box<dyn Error>> {
    loop {
        match engine.events.recv_timeout(CONTRACT_EVENT_TIMEOUT)? {
            crfty_engine::driver::DriverEvent::Ephemeral(
                crfty_core::EphemeralDelta::SessionChanged(crfty_core::SessionState::Idle),
            ) => return Ok(()),
            crfty_engine::driver::DriverEvent::Fatal { message } => return Err(message.into()),
            _ => {}
        }
    }
}

fn wait_for_encoding(engine: &EngineRuntime) -> Result<(), Box<dyn Error>> {
    loop {
        match engine.events.recv_timeout(CONTRACT_EVENT_TIMEOUT)? {
            crfty_engine::driver::DriverEvent::Ephemeral(
                crfty_core::EphemeralDelta::Telemetry(crfty_core::Telemetry {
                    phase: crfty_core::JobPhase::Encoding,
                    ..
                }),
            ) => return Ok(()),
            crfty_engine::driver::DriverEvent::Fatal { message } => return Err(message.into()),
            _ => {}
        }
    }
}

fn wait_for_stopped(engine: &EngineRuntime) -> Result<(), Box<dyn Error>> {
    loop {
        match engine.events.recv_timeout(CONTRACT_EVENT_TIMEOUT)? {
            crfty_engine::driver::DriverEvent::Durable(
                crfty_core::DurableDelta::ItemFinished {
                    outcome: crfty_core::ItemOutcome::Stopped,
                    ..
                },
            ) => return Ok(()),
            crfty_engine::driver::DriverEvent::Fatal { message } => return Err(message.into()),
            _ => {}
        }
    }
}

fn accepted_reply(reply: crfty_core::Reply) -> Result<(), Box<dyn Error>> {
    if reply == crfty_core::Reply::Accepted {
        Ok(())
    } else {
        Err(format!("command was rejected: {reply:?}").into())
    }
}

fn fake_ffmpeg() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    let scoring = arguments
        .iter()
        .any(|argument| argument.to_string_lossy().contains("libvmaf"));
    if scoring {
        emit_progress()?;
        eprintln!("[Parsed_libvmaf_0] VMAF score: 95.000000");
        return Ok(());
    }

    let output = arguments
        .last()
        .map(PathBuf::from)
        .ok_or("ffmpeg output argument is missing")?;
    if output != Path::new("-") {
        fs::write(&output, vec![0_u8; 4096])?;
    }
    emit_progress()?;
    thread::sleep(Duration::from_millis(50));

    if output
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| stem.contains("cancel") || stem.contains("panic"))
    {
        if output.to_string_lossy().contains("descendant") {
            let heartbeat = output.with_extension("heartbeat");
            let _child = Command::new(env::current_exe()?)
                .arg("heartbeat")
                .arg(heartbeat)
                .spawn()?;
        }
        thread::sleep(Duration::from_secs(30));
    }

    eprintln!(
        "video:1kB audio:2kB subtitle:0kB other streams:1kB global headers:0kB muxing overhead: 0.0%"
    );
    Ok(())
}

fn emit_progress() -> Result<(), Box<dyn Error>> {
    eprint!(
        "frame=    1 fps= 2 q=40.0 size=       1kB time=00:00:01.00 bitrate=8.0kbits/s speed=1x    \r"
    );
    io::stderr().flush()?;
    Ok(())
}

fn heartbeat(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut counter = 0_u64;
    loop {
        fs::write(path, counter.to_string())?;
        counter = counter.wrapping_add(1);
        thread::sleep(Duration::from_millis(20));
    }
}

fn run_contract(mut arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn Error>> {
    let input = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("input path is missing")?;
    let output_dir = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("output directory is missing")?;
    let tools = MediaTools {
        ffmpeg: arguments
            .next()
            .map(PathBuf::from)
            .ok_or("ffmpeg path is missing")?,
        ffprobe: arguments
            .next()
            .map(PathBuf::from)
            .ok_or("ffprobe path is missing")?,
    };

    let runtime = AbAv1Runtime::start()?;
    if AbAv1Runtime::start().is_ok() {
        return Err("a second ab-av1 runtime started concurrently".into());
    }
    cancel_search_and_reuse(&runtime, &tools, &input, &output_dir)?;
    successful_search(&runtime, &tools, &input)?;
    successful_encode(&runtime, &tools, &input, &output_dir)?;
    cancel_descendant_and_reuse(&runtime, &tools, &input, &output_dir)?;
    panic_and_reuse(&runtime, &tools, &input, &output_dir)?;
    runtime.shutdown()?;
    shutdown_cancels_active_job(&tools, &input, &output_dir)?;
    Ok(())
}

fn cancel_search_and_reuse(
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    input: &Path,
    output_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let cancel_input = output_dir.join("cancel-search.mkv");
    fs::copy(input, &cancel_input)?;
    let job = runtime.start_search(tools.clone(), search_request(&cancel_input))?;
    thread::sleep(Duration::from_millis(150));
    job.cancel(crfty_engine::ab_av1::CancelMode::Force);
    let report = job.wait()?;
    if report.terminal != JobTerminal::Cancelled {
        return Err(format!(
            "search cancel returned the wrong terminal: {:?}",
            report.terminal
        )
        .into());
    }
    Ok(())
}

fn successful_search(
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    input: &Path,
) -> Result<(), Box<dyn Error>> {
    let report = runtime
        .start_search(tools.clone(), search_request(input))?
        .wait()?;
    match report.terminal {
        JobTerminal::Completed(outcome) if outcome.vmaf == 95.0 => {}
        terminal => {
            return Err(format!("search did not complete with typed data: {terminal:?}").into());
        }
    }
    if report.final_telemetry.is_none() {
        return Err("search report omitted final telemetry".into());
    }
    Ok(())
}

fn successful_encode(
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    input: &Path,
    output_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let output = output_dir.join("first.mkv");
    let report = runtime
        .start_encode(tools.clone(), encode_request(input, &output))?
        .wait()?;
    verify_encode(report.terminal)?;
    if report.final_telemetry.is_none() {
        return Err("encode report omitted final telemetry".into());
    }
    Ok(())
}

fn cancel_descendant_and_reuse(
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    input: &Path,
    output_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let cancelled = output_dir.join("cancel-descendant.mkv");
    let heartbeat = cancelled.with_extension("heartbeat");
    let job = runtime.start_encode(tools.clone(), encode_request(input, &cancelled))?;
    wait_for_telemetry(&job)?;
    wait_for_file(&heartbeat)?;
    if !matches!(
        runtime.start_encode(
            tools.clone(),
            encode_request(input, &output_dir.join("busy.mkv")),
        ),
        Err(StartJobError::Busy)
    ) {
        return Err("runtime accepted a concurrent job".into());
    }
    job.cancel(crfty_engine::ab_av1::CancelMode::Force);
    let report = job.wait()?;
    if report.terminal != JobTerminal::Cancelled {
        return Err(format!("cancel returned the wrong terminal: {:?}", report.terminal).into());
    }
    if cancelled.exists() {
        return Err("cancelled encode left its output behind".into());
    }
    let heartbeat_value = fs::read(&heartbeat)?;
    thread::sleep(Duration::from_millis(200));
    if fs::read(&heartbeat)? != heartbeat_value {
        return Err("cancelled encode left a running descendant".into());
    }

    let second = output_dir.join("second.mkv");
    verify_encode(
        runtime
            .start_encode(tools.clone(), encode_request(input, &second))?
            .wait()?
            .terminal,
    )
}

fn panic_and_reuse(
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    input: &Path,
    output_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let output = output_dir.join("panic.mkv");
    let report = runtime
        .start_encode_with_fault(
            tools.clone(),
            encode_request(input, &output),
            FaultInjection::PanicAfterFirstProgress,
        )?
        .wait()?;
    if !matches!(
        report.terminal,
        JobTerminal::Panicked {
            cleanup_failure: None
        }
    ) {
        return Err(format!("panic returned the wrong terminal: {:?}", report.terminal).into());
    }
    if output.exists() {
        return Err("panicked encode left its output behind".into());
    }

    let after = output_dir.join("after-fault.mkv");
    verify_encode(
        runtime
            .start_encode(tools.clone(), encode_request(input, &after))?
            .wait()?
            .terminal,
    )
}

fn shutdown_cancels_active_job(
    tools: &MediaTools,
    input: &Path,
    output_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let runtime = AbAv1Runtime::start()?;
    let output = output_dir.join("cancel-shutdown.mkv");
    let job = runtime.start_encode(tools.clone(), encode_request(input, &output))?;
    wait_for_telemetry(&job)?;
    runtime.shutdown()?;
    let report = job.wait()?;
    if report.terminal != JobTerminal::Cancelled || output.exists() {
        return Err(format!(
            "shutdown did not cleanly cancel the active job: {:?}",
            report.terminal
        )
        .into());
    }
    Ok(())
}

fn verify_encode(terminal: JobTerminal<EncodeOutcome>) -> Result<(), Box<dyn Error>> {
    match terminal {
        JobTerminal::Completed(outcome)
            if outcome.output_size == 4096 && outcome.stream_sizes.audio == 2048 =>
        {
            Ok(())
        }
        terminal => {
            Err(format!("encode did not preserve typed completion data: {terminal:?}").into())
        }
    }
}

fn search_request(input: &Path) -> SearchRequest {
    SearchRequest {
        input: input.to_owned(),
        target_vmaf: 90.0,
        max_encoded_percent: 500.0,
        preset: 12,
        samples: Some(1),
        sample_duration: Duration::from_secs(1),
        thorough: false,
        decode_mode: DecodeMode::Software,
    }
}

fn encode_request(input: &Path, output: &Path) -> EncodeRequest {
    EncodeRequest {
        input: input.to_owned(),
        output: output.to_owned(),
        crf: 30.0,
        preset: 12,
        decode_mode: DecodeMode::Software,
    }
}

fn wait_for_telemetry<T>(job: &JobHandle<T>) -> Result<(), Box<dyn Error>> {
    for _attempt in 0..500 {
        if job.latest_telemetry().is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("job produced no telemetry".into())
}

fn wait_for_file(path: &Path) -> Result<(), Box<dyn Error>> {
    for _attempt in 0..500 {
        if path.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(format!("file was not created: {}", path.display()).into())
}
