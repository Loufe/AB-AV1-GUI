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
    ab_av1::MediaTools,
    coordinator::{EngineConfig, EngineRuntime},
    driver::{DriverEvent, DriverHandle},
    journal::JournalWriter,
    output::{FixtureByteInspector, OutputManager},
    tools::ToolDiscovery,
};

/// `AbAv1Runtime` is a process-wide singleton; engine-starting tests in this
/// file must not overlap.
static ENGINE_GUARD: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn execution() -> ExecutionSettings {
    ExecutionSettings::production(
        AnalysisProfile::production(ToolRevisions {
            ab_av1: "fixture".to_owned(),
            ffmpeg: "fixture".to_owned(),
            encoder: "fixture".to_owned(),
        }),
        false,
    )
}

fn missing_tools() -> ToolDiscovery {
    ToolDiscovery::Missing {
        missing: vec![MediaTool::Ffmpeg, MediaTool::Ffprobe],
        detail: "fixture: no tools installed".to_owned(),
    }
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

fn engine_config(directory: &TestDirectory, media_tools: ToolDiscovery) -> EngineConfig {
    EngineConfig {
        journal_path: directory.path().join("state.jsonl"),
        config_path: directory.path().join("config.json"),
        media_tools,
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
        ToolDiscovery::Available {
            source: ToolSource::System,
            tools: MediaTools {
                ffmpeg: executable.clone(),
                ffprobe: executable,
            },
        },
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
