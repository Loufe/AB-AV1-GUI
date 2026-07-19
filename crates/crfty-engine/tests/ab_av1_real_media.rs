#![forbid(unsafe_code)]
#![cfg(feature = "contract-test-fixture")]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use crfty_core::{
    AnalysisProfile, Command as CoreCommand, DurableDelta, ExecutionSettings, Operation,
    OutputTarget, QueueCommand, QueueItemId, SessionCommand, VmafTarget,
};

const REAL_CONTRACT_TARGET: VmafTarget = VmafTarget(80);
const REAL_CONTRACT_PRESET: u8 = 12;
const REAL_CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS: u32 = 50_000;
const REAL_CONTRACT_SAMPLE_COUNT: u64 = 1;
const REAL_CONTRACT_SAMPLE_DURATION_MS: u64 = 1_000;
use crfty_engine::ab_av1::{
    AbAv1Runtime, EncodeRequest, FaultInjection, JobTerminal, MediaTools, SearchRequest,
};
use crfty_engine::coordinator::{EngineConfig, EngineRuntime};

#[test]
#[ignore = "requires CRFTY_FFMPEG and CRFTY_FFPROBE with libsvtav1 and libvmaf"]
fn real_search_encode_cancel_panic_and_reuse() {
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
    let tools = tools_from_environment();
    let directory = env::temp_dir().join(format!(
        "crfty-real-coordinator-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&directory).expect("create coordinator directory");
    let input = directory.join("input.mkv");
    generate_fixture(&tools.ffmpeg, &input, "3", "256x144");
    let engine = EngineRuntime::start(EngineConfig {
        journal_path: directory.join("state.jsonl"),
        media_tools: tools.clone(),
        execution: ExecutionSettings {
            requested_target: REAL_CONTRACT_TARGET,
            fallback_floor: REAL_CONTRACT_TARGET,
            fallback_step: crfty_core::VMAF_FALLBACK_STEP,
            overwrite_existing: false,
            profile: AnalysisProfile {
                preset: REAL_CONTRACT_PRESET,
                max_encoded_percent_basis_points: REAL_CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS,
                samples: Some(REAL_CONTRACT_SAMPLE_COUNT),
                sample_duration_ms: REAL_CONTRACT_SAMPLE_DURATION_MS,
                thorough: false,
                hardware_decode: false,
                ab_av1_revision: "real-contract".to_owned(),
                ffmpeg_revision: "real-contract".to_owned(),
                encoder_revision: "real-contract".to_owned(),
            },
        },
    })
    .expect("start coordinator");
    let _snapshot = engine.events.recv().expect("startup snapshot");
    engine
        .commands
        .submit(CoreCommand::Queue(QueueCommand::Add {
            item_id: QueueItemId(1),
            input: input.clone(),
            operation: Operation::Convert,
            output_target: OutputTarget::Suffix {
                suffix: "_av1".to_owned(),
            },
        }))
        .expect("queue command");
    engine
        .commands
        .submit(CoreCommand::Session(SessionCommand::Start))
        .expect("start command");
    loop {
        if matches!(
            engine.events.recv().expect("coordinator event"),
            crfty_engine::driver::DriverEvent::Durable(DurableDelta::ItemFinished { .. })
        ) {
            break;
        }
    }
    let output = directory.join("input_av1.mkv");
    assert_eq!(probe_codec(&tools.ffprobe, &output), "av1");
    engine.shutdown().expect("coordinator shutdown");
    fs::remove_dir_all(directory).expect("remove coordinator directory");
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
        .arg(format!("testsrc2=size={size}:rate=24:duration={duration}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!(
            "sine=frequency=1000:sample_rate=48000:duration={duration}"
        ))
        .args(["-c:v", "ffv1", "-c:a", "pcm_s16le"])
        .arg(output)
        .status()
        .expect("run fixture-generation ffmpeg");
    assert!(status.success(), "fixture generation failed: {status}");
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
    }
}

fn encode_request(input: &Path, output: &Path, crf: f32, preset: u8) -> EncodeRequest {
    EncodeRequest {
        input: input.to_owned(),
        output: output.to_owned(),
        crf,
        preset,
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
