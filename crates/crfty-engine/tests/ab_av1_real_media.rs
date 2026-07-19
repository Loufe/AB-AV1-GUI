#![forbid(unsafe_code)]
#![cfg(feature = "contract-test-fixture")]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, MutexGuard},
    thread,
    time::Duration,
};

use crfty_core::{
    AnalysisProfile, DecodeMode, DecodePreference, DurableDelta, ExecutionSettings, Operation,
    OutputTarget, QueueCommand, QueueItemId, SessionCommand, VmafTarget,
};

const REAL_CONTRACT_TARGET: VmafTarget = VmafTarget(80);
const REAL_CONTRACT_PRESET: u8 = 12;
const REAL_CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS: u32 = 50_000;
const REAL_CONTRACT_SAMPLE_COUNT: u64 = 1;
const REAL_CONTRACT_SAMPLE_DURATION_MS: u64 = 1_000;
const REAL_COORDINATOR_FIXTURE_DURATION: &str = "2";
const REAL_COORDINATOR_FIXTURE_SIZE: &str = "1280x720";
const REAL_FIXTURE_FRAME_RATE: u8 = 24;
const REAL_FIXTURE_AUDIO_FREQUENCY: u16 = 1_000;
const REAL_FIXTURE_AUDIO_SAMPLE_RATE: u32 = 48_000;
const REAL_FIXTURE_AV1_PRESET: &str = "12";
const REAL_FIXTURE_AV1_CRF: &str = "35";
static REAL_MEDIA_TEST_LOCK: Mutex<()> = Mutex::new(());
use crfty_engine::ab_av1::{
    AbAv1Runtime, EncodeRequest, FaultInjection, JobTerminal, MediaTools, SearchRequest,
};
use crfty_engine::coordinator::{EngineConfig, EngineRuntime};

#[test]
#[ignore = "requires CRFTY_FFMPEG and CRFTY_FFPROBE with libsvtav1 and libvmaf"]
fn real_search_encode_cancel_panic_and_reuse() {
    let _guard = lock_real_media_tests();
    let tools = tools_from_environment();
    let directory = env::temp_dir().join(format!(
        "crfty-real-media-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&directory).expect("create real-media directory");
    let short = directory.join("short.mkv");
    let long = directory.join("long.mkv");
    generate_fixture(&tools.ffmpeg, &short, "3", "256x144");
    generate_fixture(&tools.ffmpeg, &long, "20", "480x270");

    let runtime = AbAv1Runtime::start().expect("start adapter runtime");

    let cancelled_search = runtime
        .start_search(tools.clone(), search_request(&long))
        .expect("start cancellable search");
    thread::sleep(Duration::from_millis(200));
    cancelled_search.cancel(crfty_engine::ab_av1::CancelMode::Force);
    assert_eq!(
        cancelled_search.expect_report().terminal,
        JobTerminal::Cancelled
    );

    let search = runtime
        .start_search(tools.clone(), search_request(&short))
        .expect("start real search")
        .expect_report();
    let crf = match search.terminal {
        JobTerminal::Completed(outcome) => {
            assert!(outcome.vmaf >= 80.0);
            outcome.crf
        }
        terminal => panic!("real search failed: {terminal:?}"),
    };

    let encoded = directory.join("encoded.mkv");
    let report = runtime
        .start_encode(tools.clone(), encode_request(&short, &encoded, crf, 12))
        .expect("start real encode")
        .expect_report();
    assert!(matches!(report.terminal, JobTerminal::Completed(_)));
    assert_eq!(probe_codec(&tools.ffprobe, &encoded), "av1");

    let cancelled_output = directory.join("cancelled.mkv");
    let cancelled = runtime
        .start_encode(
            tools.clone(),
            encode_request(&long, &cancelled_output, crf, 4),
        )
        .expect("start cancellable encode");
    wait_for_telemetry(&cancelled);
    cancelled.cancel(crfty_engine::ab_av1::CancelMode::Force);
    assert_eq!(cancelled.expect_report().terminal, JobTerminal::Cancelled);
    assert!(!cancelled_output.exists());

    let panicked_output = directory.join("panicked.mkv");
    let panicked = runtime
        .start_encode_with_fault(
            tools.clone(),
            encode_request(&long, &panicked_output, crf, 4),
            FaultInjection::PanicAfterFirstProgress,
        )
        .expect("start fault-injected encode")
        .expect_report();
    assert!(matches!(
        panicked.terminal,
        JobTerminal::Panicked {
            cleanup_failure: None
        }
    ));
    assert!(!panicked_output.exists());

    let reused = directory.join("reused.mkv");
    let report = runtime
        .start_encode(tools.clone(), encode_request(&short, &reused, crf, 12))
        .expect("start encode after panic")
        .expect_report();
    assert!(matches!(report.terminal, JobTerminal::Completed(_)));
    assert_eq!(probe_codec(&tools.ffprobe, &reused), "av1");

    runtime.shutdown().expect("shutdown adapter runtime");
    fs::remove_dir_all(directory).expect("remove real-media directory");
}

#[test]
#[ignore = "requires CRFTY_FFMPEG and CRFTY_FFPROBE with libsvtav1 and libvmaf"]
fn real_coordinator_analyzes_encodes_verifies_and_promotes() {
    let _guard = lock_real_media_tests();
    let tools = tools_from_environment();
    let directory = env::temp_dir().join(format!(
        "crfty-real-coordinator-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&directory).expect("create coordinator directory");
    let input = directory.join("input.mkv");
    generate_fixture(
        &tools.ffmpeg,
        &input,
        REAL_COORDINATOR_FIXTURE_DURATION,
        REAL_COORDINATOR_FIXTURE_SIZE,
    );
    let engine =
        EngineRuntime::start(real_engine_config(&directory, &tools)).expect("start coordinator");
    let _snapshot = engine.events.recv().expect("startup snapshot");
    engine
        .commands
        .submit_queue(QueueCommand::Add {
            item_id: QueueItemId(1),
            input: input.clone(),
            operation: Operation::Convert,
            output_target: OutputTarget::Suffix {
                suffix: "_av1".to_owned(),
            },
        })
        .expect("queue command");
    engine
        .commands
        .submit_session(SessionCommand::Start)
        .expect("start command");
    loop {
        if let crfty_engine::driver::DriverEvent::Durable(DurableDelta::ItemFinished {
            outcome,
            ..
        }) = engine.events.recv().expect("coordinator event")
        {
            assert_eq!(outcome, crfty_core::ItemOutcome::Converted);
            break;
        }
    }
    let output = directory.join("input_av1.mkv");
    assert_eq!(probe_codec(&tools.ffprobe, &output), "av1");
    engine.shutdown().expect("coordinator shutdown");
    fs::remove_dir_all(directory).expect("remove coordinator directory");
}

#[test]
#[ignore = "requires CRFTY_FFMPEG and CRFTY_FFPROBE with libsvtav1"]
fn real_coordinator_remuxes_av1_mp4_without_reencoding() {
    let _guard = lock_real_media_tests();
    let tools = tools_from_environment();
    let directory = env::temp_dir().join(format!(
        "crfty-real-remux-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&directory).expect("create real-remux directory");
    let input = directory.join("input.mp4");
    generate_av1_mp4_fixture(&tools.ffmpeg, &input);
    let engine = EngineRuntime::start(real_engine_config(&directory, &tools))
        .expect("start remux coordinator");
    let _snapshot = engine.events.recv().expect("startup snapshot");
    engine
        .commands
        .submit_queue(QueueCommand::Add {
            item_id: QueueItemId(1),
            input: input.clone(),
            operation: Operation::Convert,
            output_target: OutputTarget::Suffix {
                suffix: "_remuxed".to_owned(),
            },
        })
        .expect("queue remux command");
    engine
        .commands
        .submit_session(SessionCommand::Start)
        .expect("start remux command");
    loop {
        if let crfty_engine::driver::DriverEvent::Durable(DurableDelta::ItemFinished {
            outcome,
            ..
        }) = engine.events.recv().expect("remux coordinator event")
        {
            assert_eq!(outcome, crfty_core::ItemOutcome::Remuxed);
            break;
        }
    }
    let output = directory.join("input_remuxed.mkv");
    assert_eq!(probe_codec(&tools.ffprobe, &output), "av1");
    assert_eq!(
        probe_video_packet_hashes(&tools.ffprobe, &input),
        probe_video_packet_hashes(&tools.ffprobe, &output)
    );
    assert_eq!(
        probe_stream_inventory(&tools.ffprobe, &input),
        probe_stream_inventory(&tools.ffprobe, &output)
    );
    engine.shutdown().expect("remux coordinator shutdown");
    fs::remove_dir_all(directory).expect("remove real-remux directory");
}

trait ExpectReport<T> {
    fn expect_report(self) -> crfty_engine::ab_av1::JobReport<T>;
}

impl<T> ExpectReport<T> for crfty_engine::ab_av1::JobHandle<T> {
    fn expect_report(self) -> crfty_engine::ab_av1::JobReport<T> {
        self.wait().expect("receive adapter report")
    }
}

fn tools_from_environment() -> MediaTools {
    MediaTools {
        ffmpeg: absolute_environment_path("CRFTY_FFMPEG"),
        ffprobe: absolute_environment_path("CRFTY_FFPROBE"),
    }
}

fn absolute_environment_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env::var_os(name).expect("media tool environment variable"));
    assert!(
        path.is_absolute() && path.is_file(),
        "invalid {name}: {}",
        path.display()
    );
    path
}

fn generate_fixture(ffmpeg: &Path, output: &Path, duration: &str, size: &str) {
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .arg("-i")
        .arg(format!(
            "testsrc2=size={size}:rate={REAL_FIXTURE_FRAME_RATE}:duration={duration}"
        ))
        .args(["-f", "lavfi", "-i"])
        .arg(format!(
            "sine=frequency={REAL_FIXTURE_AUDIO_FREQUENCY}:sample_rate={REAL_FIXTURE_AUDIO_SAMPLE_RATE}:duration={duration}"
        ))
        .args(["-c:v", "ffv1", "-c:a", "pcm_s16le"])
        .arg(output)
        .status()
        .expect("run fixture-generation ffmpeg");
    assert!(status.success(), "fixture generation failed: {status}");
}

fn generate_av1_mp4_fixture(ffmpeg: &Path, output: &Path) {
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .arg("-i")
        .arg(format!(
            "testsrc2=size={REAL_COORDINATOR_FIXTURE_SIZE}:rate={REAL_FIXTURE_FRAME_RATE}:duration={REAL_COORDINATOR_FIXTURE_DURATION}"
        ))
        .args(["-f", "lavfi", "-i"])
        .arg(format!(
            "sine=frequency={REAL_FIXTURE_AUDIO_FREQUENCY}:sample_rate={REAL_FIXTURE_AUDIO_SAMPLE_RATE}:duration={REAL_COORDINATOR_FIXTURE_DURATION}"
        ))
        .args([
            "-c:v",
            "libsvtav1",
            "-preset",
            REAL_FIXTURE_AV1_PRESET,
            "-crf",
            REAL_FIXTURE_AV1_CRF,
            "-c:a",
            "aac",
        ])
        .arg(output)
        .status()
        .expect("run AV1 fixture-generation ffmpeg");
    assert!(status.success(), "AV1 fixture generation failed: {status}");
}

fn real_engine_config(directory: &Path, tools: &MediaTools) -> EngineConfig {
    EngineConfig {
        journal_path: directory.join("state.jsonl"),
        config_path: directory.join("config.json"),
        media_tools: tools.clone(),
        execution: ExecutionSettings {
            requested_target: REAL_CONTRACT_TARGET,
            fallback_floor: REAL_CONTRACT_TARGET,
            fallback_step: crfty_core::VMAF_FALLBACK_STEP,
            overwrite_existing: false,
            decode_preference: DecodePreference::SoftwareOnly,
            profile: AnalysisProfile {
                preset: REAL_CONTRACT_PRESET,
                max_encoded_percent_basis_points: REAL_CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS,
                samples: Some(REAL_CONTRACT_SAMPLE_COUNT),
                sample_duration_ms: REAL_CONTRACT_SAMPLE_DURATION_MS,
                thorough: false,
                decode_mode: DecodeMode::Software,
                ab_av1_revision: "real-contract".to_owned(),
                ffmpeg_revision: "real-contract".to_owned(),
                encoder_revision: "real-contract".to_owned(),
            },
        },
    }
}

fn probe_video_packet_hashes(ffprobe: &Path, input: &Path) -> Vec<u8> {
    probe_output(
        ffprobe,
        input,
        &[
            "-select_streams",
            "v:0",
            "-show_packets",
            "-show_entries",
            "packet=data_hash",
            "-show_data_hash",
            "sha256",
            "-of",
            "json",
        ],
    )
}

fn probe_stream_inventory(ffprobe: &Path, input: &Path) -> Vec<u8> {
    probe_output(
        ffprobe,
        input,
        &[
            "-show_entries",
            "stream=codec_type,codec_name",
            "-of",
            "csv=p=0",
        ],
    )
}

fn probe_output(ffprobe: &Path, input: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = Command::new(ffprobe)
        .args(["-v", "error"])
        .args(arguments)
        .arg(input)
        .output()
        .expect("run contract ffprobe");
    assert!(
        output.status.success(),
        "contract probe failed: {}",
        output.status
    );
    output.stdout
}

fn search_request(input: &Path) -> SearchRequest {
    SearchRequest {
        input: input.to_owned(),
        target_vmaf: 80.0,
        max_encoded_percent: 500.0,
        preset: 12,
        samples: Some(1),
        sample_duration: Duration::from_secs(1),
        thorough: false,
        decode_mode: DecodeMode::Software,
    }
}

fn encode_request(input: &Path, output: &Path, crf: f32, preset: u8) -> EncodeRequest {
    EncodeRequest {
        input: input.to_owned(),
        output: output.to_owned(),
        crf,
        preset,
        decode_mode: DecodeMode::Software,
    }
}

fn wait_for_telemetry<T>(job: &crfty_engine::ab_av1::JobHandle<T>) {
    for _attempt in 0..6_000 {
        if job.latest_telemetry().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("real encode produced no telemetry");
}

fn probe_codec(ffprobe: &Path, input: &Path) -> String {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(input)
        .output()
        .expect("run output ffprobe");
    assert!(
        output.status.success(),
        "output probe failed: {}",
        output.status
    );
    String::from_utf8(output.stdout)
        .expect("ffprobe returned UTF-8")
        .trim()
        .to_owned()
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos()
}

fn lock_real_media_tests() -> MutexGuard<'static, ()> {
    REAL_MEDIA_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
