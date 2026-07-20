//! FFmpeg-free startup contract: the durable engine starts, replays, and
//! serves non-media commands when no tools are discovered, and startup
//! recovery defers unsettled output transactions instead of settling blind.
#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crfty_core::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, AppState, ClaimId, Command, Crf,
    EphemeralDelta, ExecutionSettings, ItemOutcome, MediaTool, Operation, OutputDelta,
    OutputTarget, QueueCommand, QueueItemId, QueueItemState, Replacement, Reply, RunId,
    SearchMeasurement, SessionCommand, Settings, SettingsCommand, ToolAvailability, ToolRevisions,
    ToolSource, ToolsState, UnixMillis, VmafScore, WorkerCommand, apply,
};
use crfty_engine::{
    ab_av1::{AB_AV1_REVISION, MediaTools},
    coordinator::{EngineConfig, EngineRuntime, ToolsConfig},
    driver::{DriverEvent, DriverHandle},
    journal::JournalWriter,
    output::{FixtureByteInspector, OutputManager},
    vendor::discovery::{self, CurrentTools, DiscoveredTools, DiscoveryEnvironment},
};

/// `AbAv1Runtime` is a process-wide singleton; engine-starting tests in this
/// file must not overlap.
static ENGINE_GUARD: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn execution() -> ExecutionSettings {
    let mut profile = AnalysisProfile::production();
    profile.ab_av1_revision = "fixture".to_owned();
    profile.ffmpeg_revision = "fixture".to_owned();
    profile.encoder_revision = "fixture".to_owned();
    ExecutionSettings::production(profile, false)
}

fn missing_tools() -> ToolsConfig {
    ToolsConfig::Fixed(DiscoveredTools::Missing {
        missing: vec![MediaTool::Ffmpeg, MediaTool::Ffprobe],
        detail: "fixture: no tools installed".to_owned(),
    })
}

fn fixture_available() -> ToolAvailability {
    ToolAvailability::Available {
        source: ToolSource::System,
        revisions: ToolRevisions {
            ab_av1: "fixture".to_owned(),
            ffmpeg: "fixture".to_owned(),
            encoder: "fixture".to_owned(),
        },
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let unique = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "crfty-{name}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _result = fs::remove_dir_all(&self.0);
    }
}

fn engine_config(directory: &TestDirectory, tools: ToolsConfig) -> EngineConfig {
    EngineConfig {
        journal_path: directory.path().join("state.jsonl"),
        config_path: directory.path().join("config.json"),
        vendor_root: directory.path().join("vendor"),
        tools,
        execution: execution(),
    }
}

#[test]
fn startup_without_tools_replays_and_serves_non_media_commands() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("tool-free-startup");
    let journal_path = directory.path().join("state.jsonl");
    let config_path = directory.path().join("config.json");

    let settings = Settings {
        hardware_decode: false,
        ..Settings::default()
    };
    let seeder = DriverHandle::start(&journal_path, &config_path).expect("seeding driver");
    let _snapshot = seeder
        .events()
        .expect("event receiver")
        .recv()
        .expect("seed snapshot");
    assert_eq!(
        seeder
            .commands
            .submit(Command::Queue(QueueCommand::Add {
                item_id: QueueItemId(1),
                input: PathBuf::from("video.mkv"),
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
            }))
            .expect("seed add reply"),
        Reply::Accepted
    );
    assert_eq!(
        seeder
            .commands
            .submit(Command::Settings(SettingsCommand::Set {
                settings: settings.clone(),
            }))
            .expect("seed settings reply"),
        Reply::Accepted
    );
    seeder.shutdown().expect("seeding driver shutdown");

    let engine =
        EngineRuntime::start(engine_config(&directory, missing_tools())).expect("tool-free start");
    let DriverEvent::Snapshot(snapshot) = engine.events.recv().expect("startup snapshot") else {
        panic!("expected startup snapshot first");
    };
    assert_eq!(snapshot.durable.queue.len(), 1);
    assert_eq!(snapshot.durable.queue[0].id, QueueItemId(1));
    assert_eq!(snapshot.settings, settings);
    let availability = engine.events.recv().expect("availability event");
    let DriverEvent::Ephemeral(EphemeralDelta::ToolsChanged(ToolsState {
        availability: ToolAvailability::Missing { missing, detail },
        ..
    })) = availability
    else {
        panic!("expected missing-tools availability after the snapshot: {availability:?}");
    };
    assert_eq!(missing, vec![MediaTool::Ffmpeg, MediaTool::Ffprobe]);
    assert!(detail.contains("no tools installed"), "{detail}");

    assert_eq!(
        engine
            .commands
            .submit_queue(QueueCommand::Add {
                item_id: QueueItemId(2),
                input: PathBuf::from("another.mkv"),
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
            })
            .expect("tool-free add reply"),
        Reply::Accepted
    );
    let mut updated = settings.clone();
    updated.output.overwrite_existing = true;
    assert_eq!(
        engine
            .commands
            .submit_settings(SettingsCommand::Set { settings: updated })
            .expect("tool-free settings reply"),
        Reply::Accepted
    );
    let start = engine
        .commands
        .submit_session(SessionCommand::Start)
        .expect("session start reply");
    let Reply::Rejected { reason } = start else {
        panic!("expected tool-free session start rejection: {start:?}");
    };
    assert!(
        reason.starts_with("media tools are unavailable"),
        "{reason}"
    );
    for event in engine.events.try_iter() {
        assert!(
            !matches!(
                event,
                DriverEvent::Ephemeral(EphemeralDelta::SessionChanged(
                    crfty_core::SessionState::Running
                ))
            ),
            "session must not run without tools: {event:?}"
        );
    }
    engine.shutdown().expect("tool-free shutdown");
}

#[test]
fn startup_recovery_without_ffprobe_defers_output_settlement() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("tool-free-recovery");
    let journal_path = directory.path().join("state.jsonl");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"input bytes").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let transaction = manager
        .plan(
            RunId(3),
            &input,
            &final_path,
            Replacement::RetireOriginal,
            false,
        )
        .expect("plan transaction");
    let initial = manager
        .create_staging(&transaction)
        .expect("create staging");
    fs::write(&transaction.staging, b"crash-left partial bytes").expect("partial staging");

    let settings = execution();
    let mut state = AppState::default();
    let mut durable = Vec::new();
    for command in [
        Command::Queue(QueueCommand::Add {
            item_id: QueueItemId(1),
            input: input.clone(),
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Replace,
        }),
        Command::System(crfty_core::SystemCommand::ToolsDiscovered {
            availability: fixture_available(),
            update_available: false,
        }),
        Command::Session(SessionCommand::Start),
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: None,
            execution: settings.clone(),
        }),
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            at: UnixMillis(1_000),
        }),
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(AnalysisResult {
                requested_target: settings.requested_target,
                successful_target: settings.requested_target,
                fallback_floor: settings.fallback_floor,
                fallback_step: settings.fallback_step,
                failed_attempts: Vec::new(),
                measurement: SearchMeasurement {
                    crf: Crf(30_000),
                    score: VmafScore(9_500),
                    predicted_size: 1_000,
                    predicted_percent_basis_points: 5_000,
                    predicted_duration_ms: 60_000,
                    from_cache: false,
                },
                profile: settings.profile.clone(),
            }),
        }),
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
            transaction: Box::new(transaction.clone()),
        })),
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial,
        })),
    ] {
        let applied = apply(&mut state, command);
        assert!(!matches!(applied.reply, Reply::Rejected { .. }));
        durable.extend(applied.durable);
    }
    let (mut writer, _replay) = JournalWriter::open(&journal_path).expect("journal writer");
    writer.append_batch(&durable).expect("crash fixture batch");
    drop(writer);

    let deferred =
        EngineRuntime::start(engine_config(&directory, missing_tools())).expect("tool-free start");
    let DriverEvent::Snapshot(snapshot) = deferred.events.recv().expect("deferred snapshot") else {
        panic!("expected deferred snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue[0].state,
            QueueItemState::Running { .. }
        ),
        "item must stay active while settlement is deferred: {snapshot:?}"
    );
    let deferred_transaction = snapshot
        .durable
        .outputs
        .get(&RunId(3))
        .expect("deferred transaction");
    assert!(!deferred_transaction.is_settled());
    assert!(transaction.staging.exists(), "staging must be untouched");
    deferred.shutdown().expect("deferred shutdown");

    let executable = std::env::current_exe().expect("test executable");
    let recovered = EngineRuntime::start(engine_config(
        &directory,
        ToolsConfig::Fixed(DiscoveredTools::Available(CurrentTools {
            media: MediaTools {
                ffmpeg: executable.clone(),
                ffprobe: executable,
            },
            source: ToolSource::System,
            revisions: ToolRevisions {
                ab_av1: "fixture".to_owned(),
                ffmpeg: "fixture".to_owned(),
                encoder: "fixture".to_owned(),
            },
        })),
    ))
    .expect("recovery with tools");
    let DriverEvent::Snapshot(snapshot) = recovered.events.recv().expect("recovered snapshot")
    else {
        panic!("expected recovered snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue[0].state,
            QueueItemState::Finished(ItemOutcome::Stopped)
        ),
        "deferred recovery must complete once tools exist: {snapshot:?}"
    );
    assert!(!transaction.staging.exists());
    recovered.shutdown().expect("recovered shutdown");
}

/// A PATH-style directory holding contract-fixture copies that answer the
/// ffprobe JSON version probe.
fn fixture_path_directory(directory: &TestDirectory) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture"));
    let path_dir = directory.path().join("bin");
    fs::create_dir(&path_dir).expect("fixture PATH directory");
    fs::copy(&fixture, path_dir.join(tool_file_name("ffmpeg"))).expect("fixture ffmpeg");
    fs::copy(&fixture, path_dir.join(tool_file_name("ffprobe"))).expect("fixture ffprobe");
    path_dir
}

fn tool_file_name(binary: &str) -> String {
    if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_owned()
    }
}

#[test]
fn discovery_reports_missing_when_no_tier_provides_tools() {
    let directory = TestDirectory::new("discovery-none");
    let report = discovery::discover_with(
        &directory.path().join("vendor"),
        &DiscoveryEnvironment::default(),
    );
    let DiscoveredTools::Missing { missing, detail } = report.tools else {
        panic!("expected missing tools: {:?}", report.tools);
    };
    assert_eq!(missing, vec![MediaTool::Ffmpeg, MediaTool::Ffprobe]);
    assert!(detail.contains("managed install"), "{detail}");
    assert!(!report.update_available);
}

#[test]
fn discovery_finds_system_tools_and_probes_their_revisions() {
    let directory = TestDirectory::new("discovery-system");
    let path_dir = fixture_path_directory(&directory);
    let report = discovery::discover_with(
        &directory.path().join("vendor"),
        &DiscoveryEnvironment {
            search_path: Some(path_dir.clone().into_os_string()),
            ..DiscoveryEnvironment::default()
        },
    );
    let DiscoveredTools::Available(current) = report.tools else {
        panic!("expected system tools: {:?}", report.tools);
    };
    assert_eq!(current.source, ToolSource::System);
    assert_eq!(
        current.media.ffmpeg,
        path_dir.join(tool_file_name("ffmpeg"))
    );
    assert_eq!(current.revisions.ab_av1, AB_AV1_REVISION);
    assert_eq!(current.revisions.ffmpeg, "fixture-8.1.2");
    assert_eq!(current.revisions.encoder, "fixture-8.1.2");
    assert!(!report.update_available);
}

#[test]
fn invalid_explicit_path_is_fail_closed_despite_a_usable_path() {
    let directory = TestDirectory::new("discovery-explicit-invalid");
    let path_dir = fixture_path_directory(&directory);
    let report = discovery::discover_with(
        &directory.path().join("vendor"),
        &DiscoveryEnvironment {
            ffmpeg_override: Some(directory.path().join("missing-ffmpeg").into_os_string()),
            ffprobe_override: None,
            search_path: Some(path_dir.into_os_string()),
        },
    );
    let DiscoveredTools::Missing { missing, detail } = report.tools else {
        panic!("expected fail-closed missing tools: {:?}", report.tools);
    };
    assert_eq!(missing, vec![MediaTool::Ffmpeg]);
    assert!(detail.contains("CRFTY_FFMPEG"), "{detail}");
}

#[test]
fn explicit_paths_win_over_managed_and_path_tiers() {
    let directory = TestDirectory::new("discovery-explicit");
    let vendor_root = directory.path().join("vendor");
    write_managed_install(&vendor_root, "some-older-build");
    let path_dir = fixture_path_directory(&directory);
    let report = discovery::discover_with(
        &vendor_root,
        &DiscoveryEnvironment {
            ffmpeg_override: Some(path_dir.join(tool_file_name("ffmpeg")).into_os_string()),
            ffprobe_override: Some(path_dir.join(tool_file_name("ffprobe")).into_os_string()),
            search_path: None,
        },
    );
    let DiscoveredTools::Available(current) = report.tools else {
        panic!("expected explicit tools: {:?}", report.tools);
    };
    assert_eq!(current.source, ToolSource::Explicit);
    assert_eq!(current.revisions.ffmpeg, "fixture-8.1.2");
}

#[test]
fn system_tools_failing_the_version_probe_are_fail_closed() {
    let directory = TestDirectory::new("discovery-probe-failure");
    let path_dir = directory.path().join("bin");
    fs::create_dir(&path_dir).expect("plain PATH directory");
    fs::write(path_dir.join(tool_file_name("ffmpeg")), b"not a binary").expect("plain ffmpeg");
    fs::write(path_dir.join(tool_file_name("ffprobe")), b"not a binary").expect("plain ffprobe");
    let report = discovery::discover_with(
        &directory.path().join("vendor"),
        &DiscoveryEnvironment {
            search_path: Some(path_dir.into_os_string()),
            ..DiscoveryEnvironment::default()
        },
    );
    let DiscoveredTools::Missing { missing, .. } = report.tools else {
        panic!(
            "unprobeable tools must not be available: {:?}",
            report.tools
        );
    };
    assert_eq!(missing, vec![MediaTool::Ffprobe]);
}

fn write_managed_install(vendor_root: &Path, version: &str) {
    let bin = vendor_root.join("installs").join(version).join("bin");
    fs::create_dir_all(&bin).expect("managed install directory");
    fs::write(bin.join(tool_file_name("ffmpeg")), b"managed ffmpeg").expect("managed ffmpeg");
    fs::write(bin.join(tool_file_name("ffprobe")), b"managed ffprobe").expect("managed ffprobe");
    let record = format!(
        concat!(
            "{{\"version\": \"{version}\", ",
            "\"ffmpeg\": \"installs/{version}/bin/{ffmpeg}\", ",
            "\"ffprobe\": \"installs/{version}/bin/{ffprobe}\", ",
            "\"ffmpeg_revision\": \"managed-ffmpeg-{version}\", ",
            "\"encoder_revision\": \"managed-svt-{version}\"}}"
        ),
        version = version,
        ffmpeg = tool_file_name("ffmpeg"),
        ffprobe = tool_file_name("ffprobe"),
    );
    fs::write(vendor_root.join("current.json"), record).expect("managed install record");
}

#[test]
fn managed_install_provides_tools_from_metadata_without_probing() {
    let directory = TestDirectory::new("discovery-managed");
    let vendor_root = directory.path().join("vendor");
    write_managed_install(&vendor_root, "some-older-build");
    let stale = vendor_root.join("staging");
    fs::create_dir_all(&stale).expect("stale staging directory");
    fs::write(stale.join("download.partial"), b"stale bytes").expect("stale staging entry");
    let report = discovery::discover_with(&vendor_root, &DiscoveryEnvironment::default());
    let DiscoveredTools::Available(current) = report.tools else {
        panic!("expected managed tools: {:?}", report.tools);
    };
    assert_eq!(current.source, ToolSource::Managed);
    assert_eq!(current.revisions.ab_av1, AB_AV1_REVISION);
    assert_eq!(current.revisions.ffmpeg, "managed-ffmpeg-some-older-build");
    assert_eq!(current.revisions.encoder, "managed-svt-some-older-build");
    // The plain metadata files were never spawned: managed revisions come
    // from the install record alone.
    assert!(
        report.update_available,
        "an install older than the compiled-in manifest must offer an update"
    );
    assert!(!stale.exists(), "stale staging must be cleaned");
}

#[test]
fn managed_install_matching_the_manifest_offers_no_update() {
    let directory = TestDirectory::new("discovery-managed-current");
    let vendor_root = directory.path().join("vendor");
    let manifest =
        crfty_engine::vendor::manifest::current().expect("manifest exists on CI platforms");
    write_managed_install(&vendor_root, manifest.build);
    let report = discovery::discover_with(&vendor_root, &DiscoveryEnvironment::default());
    let DiscoveredTools::Available(current) = report.tools else {
        panic!("expected managed tools: {:?}", report.tools);
    };
    assert_eq!(current.source, ToolSource::Managed);
    assert!(!report.update_available);
}

#[test]
fn corrupt_managed_record_falls_back_to_the_path_tier() {
    let directory = TestDirectory::new("discovery-managed-corrupt");
    let vendor_root = directory.path().join("vendor");
    fs::create_dir_all(&vendor_root).expect("vendor root");
    fs::write(vendor_root.join("current.json"), b"{ not json").expect("corrupt record");
    let path_dir = fixture_path_directory(&directory);
    let report = discovery::discover_with(
        &vendor_root,
        &DiscoveryEnvironment {
            search_path: Some(path_dir.into_os_string()),
            ..DiscoveryEnvironment::default()
        },
    );
    let DiscoveredTools::Available(current) = report.tools else {
        panic!("expected PATH fallback: {:?}", report.tools);
    };
    assert_eq!(current.source, ToolSource::System);
    assert!(!report.update_available);
}

#[test]
fn managed_record_escaping_the_vendor_root_is_rejected() {
    let directory = TestDirectory::new("discovery-managed-escape");
    let vendor_root = directory.path().join("vendor");
    fs::create_dir_all(&vendor_root).expect("vendor root");
    fs::write(
        vendor_root.join("current.json"),
        br#"{"version": "v", "ffmpeg": "../outside/ffmpeg", "ffprobe": "../outside/ffprobe", "ffmpeg_revision": "r", "encoder_revision": "r"}"#,
    )
    .expect("escaping record");
    let report = discovery::discover_with(&vendor_root, &DiscoveryEnvironment::default());
    assert!(
        matches!(report.tools, DiscoveredTools::Missing { .. }),
        "an escaping record must not resolve tools: {:?}",
        report.tools
    );
    assert!(!report.update_available);
}

/// Waits for vendor-driven ephemeral tool updates until `predicate` accepts
/// one, panicking on stream close.
fn wait_for_tools_state(
    events: &std::sync::mpsc::Receiver<DriverEvent>,
    predicate: impl Fn(&ToolsState) -> bool,
    what: &str,
) -> ToolsState {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .unwrap_or_else(|| panic!("timed out waiting for {what}"));
        let event = events
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("stream ended waiting for {what}: {error}"));
        if let DriverEvent::Ephemeral(EphemeralDelta::ToolsChanged(tools)) = event
            && predicate(&tools)
        {
            return tools;
        }
    }
}

#[test]
fn vendor_check_rediscovers_tools_and_returns_to_idle() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("vendor-check-cycle");
    let executable = fixture_path_directory(&directory).join(tool_file_name("ffmpeg"));
    let tools = ToolsConfig::Fixed(DiscoveredTools::Available(CurrentTools {
        media: MediaTools {
            ffmpeg: executable.clone(),
            ffprobe: executable,
        },
        source: ToolSource::Explicit,
        revisions: ToolRevisions {
            ab_av1: "fixture".to_owned(),
            ffmpeg: "fixture".to_owned(),
            encoder: "fixture".to_owned(),
        },
    }));
    let engine = EngineRuntime::start(engine_config(&directory, tools)).expect("engine start");
    assert_eq!(
        engine
            .commands
            .submit_vendor(crfty_core::VendorCommand::Check)
            .expect("vendor check reply"),
        Reply::Accepted
    );
    // The reducer flips activity to Checking, then the vendor worker
    // republishes discovery and settles back to Idle.
    wait_for_tools_state(
        &engine.events,
        |tools| tools.activity == crfty_core::VendorActivity::Checking,
        "checking activity",
    );
    let settled = wait_for_tools_state(
        &engine.events,
        |tools| tools.activity == crfty_core::VendorActivity::Idle,
        "idle after check",
    );
    assert!(
        matches!(
            settled.availability,
            ToolAvailability::Available {
                source: ToolSource::Explicit,
                ..
            }
        ),
        "{settled:?}"
    );
}

#[test]
fn vendor_install_on_a_fixed_tool_engine_fails_typed() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("vendor-install-fixed");
    let engine =
        EngineRuntime::start(engine_config(&directory, missing_tools())).expect("engine start");
    assert_eq!(
        engine
            .commands
            .submit_vendor(crfty_core::VendorCommand::Install)
            .expect("vendor install reply"),
        Reply::Accepted
    );
    let failed = wait_for_tools_state(
        &engine.events,
        |tools| matches!(tools.activity, crfty_core::VendorActivity::Failed { .. }),
        "typed install failure",
    );
    let crfty_core::VendorActivity::Failed { detail } = failed.activity else {
        unreachable!();
    };
    assert!(detail.contains("fixed tool set"), "{detail}");
}
