//! Native tool contract (ADR-023): real FFmpeg/ffprobe (from `CRFTY_FFMPEG`
//! and `CRFTY_FFPROBE`, copied into a directory whose path contains spaces and
//! non-ASCII characters) configured through Settings paths, verified by the
//! session-start capability probe, and executed through the full coordinator
//! pipeline. Tool paths, media paths, and the probe all meet the hostile
//! characters, and the probed revisions must be the provenance recorded on
//! the claim.

#![forbid(unsafe_code)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use crfty_core::{
    AnalysisIntent, AnalysisProfile, DecodeMode, DecodePreference, DurableDelta, EphemeralDelta,
    ExecutionSettings, Operation, OutputTarget, OverwriteDecision, QueueAddRequest, QueueCommand,
    QueueItemId, Reply, SessionCommand, Settings, SettingsCommand, ToolAvailability,
    ToolLocationFailure, ToolPathSettings, ToolRevisions, ToolSource, ToolVerification, VmafTarget,
};
use crfty_engine::{
    ab_av1::AB_AV1_REVISION,
    coordinator::{EngineConfig, EngineRuntime, ToolsConfig},
    driver::DriverEvent,
    tools::discovery::DiscoveryEnvironment,
};

const FIXTURE_DURATION_SECONDS: &str = "2";
const FIXTURE_SIZE: &str = "1280x720";
const FIXTURE_FRAME_RATE: u8 = 24;
const CONTRACT_TARGET: VmafTarget = VmafTarget(80);
const CONTRACT_PRESET: u8 = 12;
const CONTRACT_MAX_ENCODED_PERCENT_BASIS_POINTS: u32 = 50_000;
/// Generous bound for two real probe encodes plus a two-second conversion on
/// a CI runner; the probe's own per-step timeout is far shorter.
const EVENT_TIMEOUT: Duration = Duration::from_secs(600);

#[test]
#[ignore = "requires CRFTY_FFMPEG and CRFTY_FFPROBE with libsvtav1 and libvmaf"]
#[expect(clippy::expect_used, reason = "test assertion")]
fn configured_tools_are_probed_and_execute_from_a_spaces_and_unicode_directory() {
    let source_ffmpeg = absolute_environment_path("CRFTY_FFMPEG");
    let source_ffprobe = absolute_environment_path("CRFTY_FFPROBE");
    let base = env::temp_dir().join(format!(
        "crfty tools tëst … 動画 {}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let tools_dir = base.join("media tööls");
    fs::create_dir_all(&tools_dir).expect("create tools directory");
    let ffmpeg = copy_tool(&source_ffmpeg, &tools_dir);
    let ffprobe = copy_tool(&source_ffprobe, &tools_dir);

    // No overrides and no search path: only the Settings paths can locate
    // the tools, so the discovery tier under test is the one that wins.
    let engine = EngineRuntime::start(EngineConfig {
        journal_path: base.join("state.jsonl"),
        config_path: base.join("config.json"),
        tools: ToolsConfig::Discover(DiscoveryEnvironment::default()),
        execution: execution(),
    })
    .expect("start coordinator in unicode directory");
    let _snapshot = engine.events.recv().expect("startup snapshot");
    let missing = wait_for_tools(
        &engine.events,
        |tools| matches!(tools, ToolAvailability::Missing { .. }),
        "missing tools before configuration",
    );
    assert!(matches!(
        missing,
        ToolAvailability::Missing { ref failures }
            if failures.iter().all(|failure| matches!(failure, ToolLocationFailure::NotOnSearchPath { .. }))
    ));

    let settings = Settings {
        tools: ToolPathSettings {
            ffmpeg: Some(ffmpeg.clone()),
            ffprobe: Some(ffprobe.clone()),
        },
        ..Settings::default()
    };
    assert_eq!(
        engine
            .commands
            .submit_settings(SettingsCommand::Set { settings })
            .expect("settings reply"),
        Reply::Accepted
    );
    let located = wait_for_tools(
        &engine.events,
        |tools| matches!(tools, ToolAvailability::Located { .. }),
        "tools located from settings paths",
    );
    let ToolAvailability::Located {
        tools,
        verification: ToolVerification::Pending,
    } = located
    else {
        panic!("expected pending located tools: {located:?}");
    };
    assert_eq!(tools.ffmpeg.source, ToolSource::Settings);
    assert_eq!(tools.ffprobe.source, ToolSource::Settings);
    assert_eq!(tools.ffmpeg.path, ffmpeg);
    assert_eq!(tools.ffprobe.path, ffprobe);

    let input = base.join("sample tëst….mkv");
    generate_fixture(&ffmpeg, &input);
    assert_eq!(
        engine
            .commands
            .submit_queue(QueueCommand::AddMany {
                requests: vec![QueueAddRequest {
                    item_id: QueueItemId(1),
                    input: input.clone(),
                    path_hash: None,
                    identity: None,
                    timestamp_reliability: crfty_core::TimestampReliability::Unknown,
                    operation: Operation::Convert,
                    intent: AnalysisIntent::ReuseIfFresh,
                    output_target: OutputTarget::Suffix {
                        suffix: "_av1".to_owned(),
                    },
                    overwrite: OverwriteDecision::FollowSettings,
                }],
            })
            .expect("queue reply"),
        Reply::Accepted
    );
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("start reply"),
        Reply::Accepted
    );

    let verified = wait_for_tools(
        &engine.events,
        |tools| {
            matches!(
                tools,
                ToolAvailability::Located {
                    verification: ToolVerification::Verified { .. },
                    ..
                }
            )
        },
        "verified tools",
    );
    let ToolAvailability::Located {
        verification: ToolVerification::Verified { revisions, .. },
        ..
    } = verified
    else {
        unreachable!();
    };
    assert_real_revisions(&revisions);

    let prepared = drain_until(
        &engine.events,
        |event| {
            matches!(
                event,
                DriverEvent::Durable(DurableDelta::ItemPrepared { .. })
            )
        },
        "prepared claim",
    );
    let Some(DriverEvent::Durable(DurableDelta::ItemPrepared { spec })) = prepared.last() else {
        unreachable!();
    };
    assert_eq!(spec.execution.profile.ab_av1_revision, revisions.ab_av1);
    assert_eq!(spec.execution.profile.ffmpeg_revision, revisions.ffmpeg);
    assert_eq!(spec.execution.profile.encoder_revision, revisions.encoder);

    let finished = drain_until(
        &engine.events,
        |event| {
            matches!(
                event,
                DriverEvent::Durable(DurableDelta::ItemFinished { .. })
            )
        },
        "finished item",
    );
    let Some(DriverEvent::Durable(DurableDelta::ItemFinished { outcome, .. })) = finished.last()
    else {
        unreachable!();
    };
    assert!(
        matches!(
            outcome,
            crfty_core::ItemOutcome::Converted(crfty_core::CompletionEvidence::LiveEncode { .. })
        ),
        "unexpected outcome: {outcome:?}"
    );
    let output = base.join("sample tëst…_av1.mkv");
    assert_eq!(probe_codec(&ffprobe, &output), "av1");
    engine.shutdown().expect("coordinator shutdown");
    fs::remove_dir_all(&base).expect("remove unicode contract directory");
}

/// The probe reports the running build, so the exact strings are unknown
/// here; what the contract fixes is their shape and that the encoder
/// revision is the FFmpeg build that hosts it.
fn assert_real_revisions(revisions: &ToolRevisions) {
    assert_eq!(revisions.ab_av1, AB_AV1_REVISION);
    assert!(
        !revisions.ffmpeg.trim().is_empty(),
        "probe recorded no ffmpeg revision"
    );
    assert_eq!(revisions.encoder, revisions.ffmpeg);
}

fn execution() -> ExecutionSettings {
    ExecutionSettings {
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
            ab_av1_revision: String::new(),
            ffmpeg_revision: String::new(),
            encoder_revision: String::new(),
        },
    }
}

#[expect(clippy::expect_used, reason = "fixture setup")]
fn copy_tool(source: &Path, tools_dir: &Path) -> PathBuf {
    let file_name = source.file_name().expect("tool file name");
    let destination = tools_dir.join(file_name);
    fs::copy(source, &destination).expect("copy tool into unicode directory");
    destination
}

#[expect(clippy::expect_used, reason = "fixture setup")]
fn absolute_environment_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env::var_os(name).expect("media tool environment variable"));
    assert!(
        path.is_absolute() && path.is_file(),
        "invalid {name}: {}",
        path.display()
    );
    path
}

#[expect(clippy::expect_used, reason = "fixture setup")]
fn generate_fixture(ffmpeg: &Path, output: &Path) {
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .arg("-i")
        .arg(format!(
            "testsrc2=size={FIXTURE_SIZE}:rate={FIXTURE_FRAME_RATE}:duration={FIXTURE_DURATION_SECONDS}"
        ))
        .args(["-f", "lavfi", "-i"])
        .arg(format!(
            "sine=frequency=1000:sample_rate=48000:duration={FIXTURE_DURATION_SECONDS}"
        ))
        .args(["-c:v", "ffv1", "-c:a", "pcm_s16le"])
        .arg(output)
        .status()
        .expect("run fixture-generation ffmpeg");
    assert!(status.success(), "fixture generation failed: {status}");
}

#[expect(clippy::expect_used, reason = "fixture setup")]
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

fn drain_until(
    events: &std::sync::mpsc::Receiver<DriverEvent>,
    predicate: impl Fn(&DriverEvent) -> bool,
    what: &str,
) -> Vec<DriverEvent> {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("timed out waiting for {what}"));
        let event = events
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("stream ended waiting for {what}: {error}"));
        let matched = predicate(&event);
        seen.push(event);
        if matched {
            return seen;
        }
    }
}

fn wait_for_tools(
    events: &std::sync::mpsc::Receiver<DriverEvent>,
    predicate: impl Fn(&ToolAvailability) -> bool,
    what: &str,
) -> ToolAvailability {
    let seen = drain_until(
        events,
        |event| {
            matches!(
                event,
                DriverEvent::Ephemeral(EphemeralDelta::ToolsChanged(tools)) if predicate(tools)
            )
        },
        what,
    );
    match seen.into_iter().next_back() {
        Some(DriverEvent::Ephemeral(EphemeralDelta::ToolsChanged(tools))) => tools,
        other => panic!("drained past the matching tools event: {other:?}"),
    }
}

#[expect(clippy::expect_used, reason = "fixture setup")]
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos()
}
