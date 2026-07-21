//! Native vendor contract: managed binaries discovered from and executed out
//! of a vendor root whose path contains spaces and non-ASCII characters. Runs
//! real FFmpeg (from `CRFTY_FFMPEG`/`CRFTY_FFPROBE`, copied into the managed
//! layout) through the full coordinator pipeline, so process spawning, tool
//! paths, and media paths are all exercised with the hostile characters.

#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use crfty_core::{
    AnalysisIntent, AnalysisProfile, DecodeMode, DecodePreference, DurableDelta, ExecutionSettings,
    Operation, OutputTarget, OverwriteDecision, QueueAddRequest, QueueCommand, QueueItemId,
    SessionCommand, ToolSource, VmafTarget,
};
use crfty_engine::{
    coordinator::{EngineConfig, EngineRuntime, ToolsConfig},
    vendor::discovery::{DiscoveredTools, DiscoveryEnvironment, InstalledMetadata, discover_with},
};

const INSTALL_VERSION: &str = "native-contract-build";
const FFMPEG_REVISION: &str = "native-contract-ffmpeg";
const ENCODER_REVISION: &str = "native-contract-encoder";
const FIXTURE_DURATION_SECONDS: &str = "2";
const FIXTURE_SIZE: &str = "1280x720";
const FIXTURE_FRAME_RATE: u8 = 24;
const CONTRACT_TARGET: VmafTarget = VmafTarget(80);
const CONTRACT_PRESET: u8 = 12;
const CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS: u32 = 50_000;

#[test]
#[ignore = "requires CRFTY_FFMPEG and CRFTY_FFPROBE with libsvtav1 and libvmaf"]
fn managed_tools_execute_from_a_spaces_and_unicode_vendor_root() {
    let source_ffmpeg = absolute_environment_path("CRFTY_FFMPEG");
    let source_ffprobe = absolute_environment_path("CRFTY_FFPROBE");
    let base = env::temp_dir().join(format!(
        "crfty vendor tëst … 動画 {}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let vendor_root = base.join("vendor");
    seed_managed_install(&vendor_root, &source_ffmpeg, &source_ffprobe);

    // Real discovery with no explicit overrides and no PATH: the managed
    // install in the Unicode root must win, carrying its metadata revisions.
    let report = discover_with(&vendor_root, &DiscoveryEnvironment::default());
    let DiscoveredTools::Available(current) = report.tools else {
        panic!("managed discovery failed: {:?}", report.tools);
    };
    assert_eq!(current.source, ToolSource::Managed);
    assert_eq!(current.revisions.ffmpeg, FFMPEG_REVISION);
    assert_eq!(current.revisions.encoder, ENCODER_REVISION);
    assert!(current.media.ffmpeg.starts_with(&vendor_root));
    assert!(current.media.ffprobe.starts_with(&vendor_root));

    let input = base.join("sample tëst….mkv");
    generate_fixture(&current.media.ffmpeg, &input);
    let ffprobe = current.media.ffprobe.clone();
    let revisions = current.revisions.clone();

    let engine = EngineRuntime::start(EngineConfig {
        journal_path: base.join("state.jsonl"),
        config_path: base.join("config.json"),
        vendor_root: vendor_root.clone(),
        tools: ToolsConfig::Fixed(DiscoveredTools::Available(current)),
        execution: ExecutionSettings {
            requested_target: CONTRACT_TARGET,
            fallback_floor: CONTRACT_TARGET,
            fallback_step: crfty_core::VMAF_FALLBACK_STEP,
            overwrite_existing: false,
            decode_preference: DecodePreference::SoftwareOnly,
            profile: AnalysisProfile {
                preset: CONTRACT_PRESET,
                max_encoded_percent_basis_points: CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS,
                samples: Some(1),
                sample_duration_ms: 1_000,
                thorough: false,
                decode_mode: DecodeMode::Software,
                ab_av1_revision: revisions.ab_av1,
                ffmpeg_revision: revisions.ffmpeg,
                encoder_revision: revisions.encoder,
            },
        },
    })
    .expect("start coordinator from unicode vendor root");
    let _snapshot = engine.events.recv().expect("startup snapshot");
    engine
        .commands
        .submit_queue(QueueCommand::AddMany {
            requests: vec![QueueAddRequest {
                item_id: QueueItemId(1),
                input: input.clone(),
                path_hash: None,
                stamp: None,
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Suffix {
                    suffix: "_av1".to_owned(),
                },
                overwrite: OverwriteDecision::FollowSettings,
            }],
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
            assert!(
                matches!(
                    outcome,
                    crfty_core::ItemOutcome::Converted(
                        crfty_core::CompletionEvidence::LiveEncode { .. }
                    )
                ),
                "unexpected outcome: {outcome:?}"
            );
            break;
        }
    }
    let output = base.join("sample tëst…_av1.mkv");
    assert_eq!(probe_codec(&ffprobe, &output), "av1");
    engine.shutdown().expect("coordinator shutdown");
    fs::remove_dir_all(&base).expect("remove unicode contract directory");
}

/// Lays out `installs/<version>/bin/` with copies of the real binaries and a
/// `current.json` naming them, exactly as a completed install leaves them.
fn seed_managed_install(vendor_root: &Path, source_ffmpeg: &Path, source_ffprobe: &Path) {
    let bin = vendor_root
        .join("installs")
        .join(INSTALL_VERSION)
        .join("bin");
    fs::create_dir_all(&bin).expect("create managed install layout");
    let mut relative = Vec::new();
    for source in [source_ffmpeg, source_ffprobe] {
        let file_name = source.file_name().expect("tool file name");
        fs::copy(source, bin.join(file_name)).expect("copy tool into managed install");
        relative.push(
            PathBuf::from("installs")
                .join(INSTALL_VERSION)
                .join("bin")
                .join(file_name),
        );
    }
    let metadata = InstalledMetadata {
        version: INSTALL_VERSION.to_owned(),
        ffmpeg: relative[0].clone(),
        ffprobe: relative[1].clone(),
        ffmpeg_revision: FFMPEG_REVISION.to_owned(),
        encoder_revision: ENCODER_REVISION.to_owned(),
    };
    let serialized = serde_json::to_vec_pretty(&metadata).expect("serialize install record");
    fs::write(vendor_root.join("current.json"), serialized).expect("write install record");
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

fn generate_fixture(ffmpeg: &Path, output: &Path) {
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .arg("-i")
        .arg(format!(
            "testsrc2=size={FIXTURE_SIZE}:rate={FIXTURE_FRAME_RATE}:duration={FIXTURE_DURATION_SECONDS}"
        ))
        .args(["-c:v", "ffv1"])
        .arg(output)
        .status()
        .expect("run fixture-generation ffmpeg");
    assert!(status.success(), "fixture generation failed: {status}");
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
