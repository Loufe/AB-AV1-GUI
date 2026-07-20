#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crfty_core::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, AppState, ArtifactIdentity, ClaimId, Command,
    Crf, DestructiveIdentity, DurableDelta, DurableState, EphemeralDelta, ExecutionSettings,
    FailureFacts, FailureKind, ItemOutcome, JobPhase, JobProgress, Operation, OutputDelta,
    OutputTarget, QueueCommand, QueueItemId, Replacement, Reply, RunId, SearchMeasurement,
    SessionCommand, Settings, SettingsCommand, SystemCommand, Telemetry, ToolAvailability,
    ToolRevisions, ToolSource, UnixMillis, VmafScore, WorkerCommand, apply, fold, replay,
};
use crfty_engine::{
    coordinator::{EngineConfig, EngineRuntime, ToolsConfig},
    driver::{DriverEvent, DriverHandle, DriverStartError},
    journal::JournalWriter,
    output::{ArtifactInspector, FixtureByteInspector, OutputManager},
    vendor::discovery::{CurrentTools, DiscoveredTools, MediaTools},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The ab-av1 encoder runtime is a process-wide singleton, so tests that
/// start a full `EngineRuntime` must not overlap.
static ENGINE_GUARD: Mutex<()> = Mutex::new(());

fn execution() -> ExecutionSettings {
    let mut profile = AnalysisProfile::production();
    profile.ab_av1_revision = "fixture".to_owned();
    profile.ffmpeg_revision = "fixture".to_owned();
    profile.encoder_revision = "fixture".to_owned();
    ExecutionSettings::production(profile, false)
}

fn fixture_tools(media: MediaTools) -> ToolsConfig {
    ToolsConfig::Fixed(DiscoveredTools::Available(CurrentTools {
        media,
        source: ToolSource::System,
        revisions: ToolRevisions {
            ab_av1: "fixture".to_owned(),
            ffmpeg: "fixture".to_owned(),
            encoder: "fixture".to_owned(),
        },
    }))
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

fn add(item_id: QueueItemId) -> Command {
    Command::Queue(QueueCommand::Add {
        item_id,
        input: PathBuf::from("video.mkv"),
        operation: Operation::Convert,
        intent: AnalysisIntent::ReuseIfFresh,
        output_target: OutputTarget::Replace,
    })
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

#[test]
fn data_lock_makes_a_second_driver_start_fail_as_already_running() {
    let directory = TestDirectory::new("data-lock");
    let path = directory.path().join("state.jsonl");
    let config_path = directory.path().join("config.json");
    let driver = DriverHandle::start(&path, &config_path).expect("first driver");
    let second = DriverHandle::start(&path, &config_path);
    assert!(matches!(
        second,
        Err(DriverStartError::AlreadyRunning { .. })
    ));
    driver.shutdown().expect("driver shutdown");
    // Releasing the lock is what admits the next instance.
    let restarted = DriverHandle::start(&path, &config_path).expect("restart after shutdown");
    restarted.shutdown().expect("restart shutdown");
}

#[test]
fn journal_records_replay() {
    let directory = TestDirectory::new("journal-replay");
    let path = directory.path().join("state.jsonl");
    let (mut writer, initial) = JournalWriter::open(&path).expect("first journal writer");
    assert_eq!(initial.next_sequence.0, 0);
    let delta = DurableDelta::QueueAdded {
        item: crfty_core::QueueItem {
            id: QueueItemId(1),
            input: PathBuf::from("video.mkv"),
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Replace,
            state: crfty_core::QueueItemState::Queued,
        },
    };
    let (_records, _token) = writer.append_batch(&[delta]).expect("journal append");
    drop(writer);
    let replayed = replay(&fs::read(path).expect("journal bytes"));
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state.queue.len(), 1);
}

fn queue_added(id: QueueItemId) -> DurableDelta {
    DurableDelta::QueueAdded {
        item: crfty_core::QueueItem {
            id,
            input: PathBuf::from(format!("video-{}.mkv", id.0)),
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Replace,
            state: crfty_core::QueueItemState::Queued,
        },
    }
}

/// A crash mid-append leaves a partial final record. The writer must truncate
/// it on reopen: appending after the torn bytes would merge the partial record
/// and the fresh one into a single unparseable line, and the journal would
/// load as corrupt on the following start.
#[test]
fn torn_tail_is_truncated_on_reopen_so_later_appends_stay_replayable() {
    let directory = TestDirectory::new("torn-tail");
    let path = directory.path().join("state.jsonl");
    {
        let (mut writer, _initial) = JournalWriter::open(&path).expect("journal writer");
        let (_records, _token) = writer
            .append_batch(&[queue_added(QueueItemId(1))])
            .expect("journal append");
    }
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("crash-simulation handle");
        file.write_all(b"{\"schema_version\":12,\"record\":{\"Deltas\":{\"seq")
            .expect("partial record");
    }
    let (mut writer, reopened) = JournalWriter::open(&path).expect("reopen after torn tail");
    assert!(reopened.ignored_torn_tail);
    assert!(reopened.corruption.is_none());
    assert_eq!(reopened.state.queue.len(), 1);
    assert_eq!(reopened.next_sequence.0, 1);
    let (_records, _token) = writer
        .append_batch(&[queue_added(QueueItemId(2))])
        .expect("append after truncation");
    drop(writer);
    let replayed = replay(&fs::read(&path).expect("journal bytes"));
    assert!(replayed.corruption.is_none());
    assert!(!replayed.ignored_torn_tail);
    assert_eq!(replayed.state.queue.len(), 2);
}

/// Semantic corruption must leave the journal byte-identical: the file is the
/// evidence that gets archived as `.corrupt-<timestamp>` before any discard,
/// so reopening never rewrites it (#33 §10).
#[test]
fn corrupt_journal_is_preserved_byte_identical_on_reopen() {
    let directory = TestDirectory::new("corrupt-preserve");
    let path = directory.path().join("state.jsonl");
    {
        let (mut writer, _initial) = JournalWriter::open(&path).expect("journal writer");
        let (_records, _token) = writer
            .append_batch(&[queue_added(QueueItemId(1))])
            .expect("journal append");
    }
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("corruption handle");
        file.write_all(b"not-json\n").expect("corrupt record");
    }
    let before = fs::read(&path).expect("corrupt journal bytes");
    let (writer, reopened) = JournalWriter::open(&path).expect("reopen over corruption");
    assert!(reopened.corruption.is_some());
    assert!(!reopened.ignored_torn_tail);
    assert_eq!(reopened.state.queue.len(), 1);
    drop(writer);
    assert_eq!(fs::read(&path).expect("journal bytes"), before);
}

/// Compaction replaces the journal with one snapshot head line; folded state,
/// sequence numbering, later appends, and restarts are all unaffected
/// (#33 §10).
#[test]
fn compaction_folds_journal_to_snapshot_head_and_restart_replays_it() {
    let directory = TestDirectory::new("compaction");
    let path = directory.path().join("state.jsonl");
    let (mut writer, _initial) = JournalWriter::open(&path).expect("journal writer");
    let mut folded = DurableState::default();
    for id in 1..=3_u64 {
        let delta = queue_added(QueueItemId(id));
        fold(&mut folded, &delta);
        writer.append_batch(&[delta]).expect("journal append");
    }
    writer
        .compact(&folded, "test-app", UnixMillis(1_000))
        .expect("compaction");
    assert_eq!(
        writer.journal_bytes(),
        fs::metadata(&path).expect("journal metadata").len()
    );
    let bytes = fs::read(&path).expect("journal bytes");
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, folded);
    assert_eq!(replayed.next_sequence.0, 3);

    writer
        .append_batch(&[queue_added(QueueItemId(4))])
        .expect("append after compaction");
    drop(writer);
    let (_writer, reopened) = JournalWriter::open(&path).expect("reopen after compaction");
    assert!(reopened.corruption.is_none());
    assert_eq!(reopened.state.queue.len(), 4);
    assert_eq!(reopened.next_sequence.0, 4);
}

/// A stray temp file from a compaction that crashed before its atomic replace
/// is inert: it was never the journal and reopening ignores it.
#[test]
fn stray_compaction_temp_file_does_not_affect_reopen() {
    let directory = TestDirectory::new("stray-temp");
    let path = directory.path().join("state.jsonl");
    {
        let (mut writer, _initial) = JournalWriter::open(&path).expect("journal writer");
        writer
            .append_batch(&[queue_added(QueueItemId(1))])
            .expect("journal append");
    }
    fs::write(directory.path().join(".tmpAbC123"), b"{\"partial\":").expect("stray temp fixture");
    let (_writer, reopened) = JournalWriter::open(&path).expect("reopen with stray temp");
    assert!(reopened.corruption.is_none());
    assert_eq!(reopened.state.queue.len(), 1);
}

/// A failed compaction must leave the old generation authoritative and the
/// writer still able to append — never a data loss, never a fatal.
#[cfg(unix)]
#[test]
fn failed_compaction_leaves_old_journal_authoritative_and_writer_usable() {
    use std::os::unix::fs::PermissionsExt;
    let directory = TestDirectory::new("compaction-failure");
    let path = directory.path().join("state.jsonl");
    let (mut writer, _initial) = JournalWriter::open(&path).expect("journal writer");
    let mut folded = DurableState::default();
    let delta = queue_added(QueueItemId(1));
    fold(&mut folded, &delta);
    writer.append_batch(&[delta]).expect("journal append");
    let before = fs::read(&path).expect("journal bytes");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o555))
        .expect("read-only directory");
    let failed = writer.compact(&folded, "test-app", UnixMillis(1_000));
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
        .expect("restore directory");
    assert!(failed.is_err());
    assert_eq!(fs::read(&path).expect("journal bytes"), before);
    writer
        .append_batch(&[queue_added(QueueItemId(2))])
        .expect("append after failed compaction");
    let replayed = replay(&fs::read(&path).expect("journal bytes"));
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state.queue.len(), 2);
}

#[test]
fn journal_group_commit_is_one_atomic_replay_record() {
    let directory = TestDirectory::new("journal-batch");
    let path = directory.path().join("state.jsonl");
    let (mut writer, _initial) = JournalWriter::open(&path).expect("journal writer");
    let deltas: Vec<_> = [QueueItemId(1), QueueItemId(2)]
        .into_iter()
        .map(|id| DurableDelta::QueueAdded {
            item: crfty_core::QueueItem {
                id,
                input: PathBuf::from(format!("video-{}.mkv", id.0)),
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                state: crfty_core::QueueItemState::Queued,
            },
        })
        .collect();
    let (records, _token) = writer.append_batch(&deltas).expect("journal batch");
    assert_eq!(records.len(), 1);
    drop(writer);
    let bytes = fs::read(path).expect("journal bytes");
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state.queue.len(), 2);
}

#[test]
fn driver_persists_before_emitting_and_replays_after_restart() {
    let directory = TestDirectory::new("driver");
    let path = directory.path().join("state.jsonl");
    let config_path = directory.path().join("config.json");
    let driver = DriverHandle::start(&path, &config_path).expect("driver");
    assert!(matches!(
        driver
            .events()
            .expect("event receiver")
            .recv()
            .expect("snapshot"),
        DriverEvent::Snapshot(_)
    ));
    let reply = driver
        .commands
        .submit(add(QueueItemId(7)))
        .expect("driver reply");
    assert_eq!(reply, Reply::Accepted);
    assert!(matches!(
        driver
            .events()
            .expect("event receiver")
            .recv()
            .expect("durable event"),
        DriverEvent::Durable(DurableDelta::QueueAdded { .. })
    ));
    driver.shutdown().expect("driver shutdown");

    let restarted = DriverHandle::start(&path, &config_path).expect("restarted driver");
    let DriverEvent::Snapshot(snapshot) = restarted
        .events()
        .expect("event receiver")
        .recv()
        .expect("replayed snapshot")
    else {
        panic!("expected replayed snapshot");
    };
    assert_eq!(snapshot.durable.queue.len(), 1);
    assert_eq!(snapshot.durable.queue[0].id, QueueItemId(7));
    restarted.shutdown().expect("restarted shutdown");
}

#[test]
fn driver_persists_settings_and_restores_them_after_restart() {
    let directory = TestDirectory::new("driver-settings");
    let journal_path = directory.path().join("state.jsonl");
    let config_path = directory.path().join("config.json");
    let driver = DriverHandle::start(&journal_path, &config_path).expect("driver");
    let _snapshot = driver
        .events()
        .expect("event receiver")
        .recv()
        .expect("initial snapshot");
    let settings = Settings {
        hardware_decode: false,
        ..Settings::default()
    };
    assert_eq!(
        driver
            .commands
            .submit(Command::Settings(SettingsCommand::Set {
                settings: settings.clone(),
            }))
            .expect("settings reply"),
        Reply::Accepted
    );
    assert!(matches!(
        driver
            .events()
            .expect("event receiver")
            .recv()
            .expect("config event"),
        DriverEvent::Config(crfty_core::ConfigDelta::SettingsChanged {
            settings: persisted,
        }) if persisted == settings
    ));
    driver.shutdown().expect("driver shutdown");

    let restarted = DriverHandle::start(&journal_path, &config_path).expect("restarted driver");
    let DriverEvent::Snapshot(snapshot) = restarted
        .events()
        .expect("event receiver")
        .recv()
        .expect("restarted snapshot")
    else {
        panic!("expected restarted snapshot");
    };
    assert_eq!(snapshot.settings, settings);
    restarted.shutdown().expect("restarted shutdown");
}

#[test]
fn corrupted_journal_starts_degraded_and_rejects_mutation() {
    let directory = TestDirectory::new("degraded");
    let path = directory.path().join("state.jsonl");
    fs::write(&path, b"not-json\n").expect("corrupt journal fixture");
    let driver =
        DriverHandle::start(&path, directory.path().join("config.json")).expect("degraded driver");
    assert!(matches!(
        driver
            .events()
            .expect("event receiver")
            .recv()
            .expect("snapshot"),
        DriverEvent::Snapshot(_)
    ));
    assert!(matches!(
        driver
            .events()
            .expect("event receiver")
            .recv()
            .expect("degraded event"),
        DriverEvent::Degraded { .. }
    ));
    let reply = driver
        .commands
        .submit(add(QueueItemId(1)))
        .expect("rejection reply");
    assert!(matches!(reply, Reply::Rejected { .. }));
    driver.shutdown().expect("degraded shutdown");
}

#[test]
fn telemetry_pressure_coalesces_and_terminal_value_wins() {
    let directory = TestDirectory::new("telemetry");
    let path = directory.path().join("state.jsonl");
    let driver = DriverHandle::start(path, directory.path().join("config.json")).expect("driver");
    let _snapshot = driver
        .events()
        .expect("event receiver")
        .recv()
        .expect("snapshot");
    assert_eq!(
        driver
            .commands
            .submit(add(QueueItemId(1)))
            .expect("add reply"),
        Reply::Accepted
    );
    assert_eq!(
        driver
            .commands
            .submit(Command::System(SystemCommand::ToolsDiscovered {
                availability: fixture_available(),
                update_available: false,
            }))
            .expect("discovery reply"),
        Reply::Accepted
    );
    assert_eq!(
        driver
            .commands
            .submit(Command::Session(SessionCommand::Start))
            .expect("start reply"),
        Reply::Accepted
    );
    assert!(matches!(
        driver
            .commands
            .submit(Command::Worker(WorkerCommand::ReserveNext {
                claim_id: ClaimId(2),
                run_id: RunId(3),
            }))
            .expect("reservation reply"),
        Reply::Reserved(Some(_))
    ));
    assert!(matches!(
        driver
            .commands
            .submit(Command::Worker(WorkerCommand::PrepareReserved {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
                observation: None,
                execution: execution(),
            }))
            .expect("preparation reply"),
        Reply::Claimed(Some(_))
    ));
    for sequence in 0..100_000 {
        driver.commands.publish_telemetry(Telemetry {
            run_id: RunId(3),
            sequence,
            phase: JobPhase::Encoding,
            progress: JobProgress::OutputPositionMs(sequence),
        });
    }
    std::thread::sleep(Duration::from_millis(30));
    let terminal_sequence = 100_001;
    assert_eq!(
        driver
            .commands
            .submit(Command::Worker(WorkerCommand::Terminal {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
                outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
                at: UnixMillis(1_000),
                phase_spans: Vec::new(),
                final_telemetry: Some(Telemetry {
                    run_id: RunId(3),
                    sequence: terminal_sequence,
                    phase: JobPhase::Finalizing,
                    progress: JobProgress::OutputPositionMs(100),
                }),
            }))
            .expect("terminal reply"),
        Reply::Accepted
    );
    let mut maximum = 0;
    while let Ok(event) = driver.events().expect("event receiver").try_recv() {
        if let DriverEvent::Ephemeral(EphemeralDelta::Telemetry(update)) = event {
            maximum = maximum.max(update.sequence);
        }
    }
    assert_eq!(maximum, terminal_sequence);
    driver.shutdown().expect("driver shutdown");
}

#[test]
fn terminal_publishes_final_telemetry_and_clear_before_item_finished() {
    let directory = TestDirectory::new("terminal-order");
    let driver = DriverHandle::start(
        directory.path().join("state.jsonl"),
        directory.path().join("config.json"),
    )
    .expect("driver");
    for command in [
        add(QueueItemId(1)),
        Command::System(SystemCommand::ToolsDiscovered {
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
            execution: execution(),
        }),
    ] {
        let reply = driver.commands.submit(command).expect("setup reply");
        assert!(!matches!(reply, Reply::Rejected { .. }));
    }
    while driver.events().expect("event receiver").try_recv().is_ok() {}

    let final_sequence = 42;
    assert_eq!(
        driver
            .commands
            .submit(Command::Worker(WorkerCommand::Terminal {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
                outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
                at: UnixMillis(1_000),
                phase_spans: Vec::new(),
                final_telemetry: Some(Telemetry {
                    run_id: RunId(3),
                    sequence: final_sequence,
                    phase: JobPhase::Finalizing,
                    progress: JobProgress::OutputPositionMs(100),
                }),
            }))
            .expect("terminal reply"),
        Reply::Accepted
    );
    let events: Vec<DriverEvent> = driver
        .events()
        .expect("event receiver")
        .try_iter()
        .collect();
    let telemetry = events.iter().position(|event| {
        matches!(
            event,
            DriverEvent::Ephemeral(EphemeralDelta::Telemetry(update))
                if update.sequence == final_sequence
        )
    });
    let cleared = events.iter().position(|event| {
        matches!(
            event,
            DriverEvent::Ephemeral(EphemeralDelta::TelemetryCleared { run_id: RunId(3) })
        )
    });
    let finished = events.iter().position(|event| {
        matches!(
            event,
            DriverEvent::Durable(DurableDelta::ItemFinished { .. })
        )
    });
    let telemetry = telemetry.expect("final telemetry event");
    let cleared = cleared.expect("telemetry cleared event");
    let finished = finished.expect("item finished event");
    assert!(
        telemetry < cleared && cleared < finished,
        "observed order telemetry={telemetry} cleared={cleared} finished={finished}"
    );
    driver.shutdown().expect("driver shutdown");
}

#[test]
fn restart_after_fsynced_terminal_folds_to_finished_snapshot() {
    let directory = TestDirectory::new("fsynced-terminal");
    let journal_path = directory.path().join("state.jsonl");
    let mut state = AppState::default();
    let mut durable = Vec::new();
    for command in [
        add(QueueItemId(1)),
        Command::System(SystemCommand::ToolsDiscovered {
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
            execution: execution(),
        }),
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            at: UnixMillis(1_000),
        }),
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
            final_telemetry: Some(Telemetry {
                run_id: RunId(3),
                sequence: 9,
                phase: JobPhase::Finalizing,
                progress: JobProgress::OutputPositionMs(100),
            }),
        }),
    ] {
        let applied = apply(&mut state, command);
        assert!(!matches!(applied.reply, Reply::Rejected { .. }));
        durable.extend(applied.durable);
    }
    let (mut writer, _replay) = JournalWriter::open(&journal_path).expect("journal writer");
    writer.append_batch(&durable).expect("terminal batch");
    drop(writer);

    // The journal holds the fsynced terminal but no subscriber ever observed
    // its publication — the crash-between-fsync-and-publish state. A restart
    // must fold straight to the finished snapshot with no telemetry.
    let driver =
        DriverHandle::start(&journal_path, directory.path().join("config.json")).expect("driver");
    let DriverEvent::Snapshot(snapshot) = driver
        .events()
        .expect("event receiver")
        .recv()
        .expect("replayed snapshot")
    else {
        panic!("expected replayed snapshot");
    };
    assert!(matches!(
        snapshot.durable.queue.first().expect("queue item").state,
        crfty_core::QueueItemState::Finished(ItemOutcome::Failed { .. })
    ));
    let run = snapshot
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("conversion run");
    assert!(run.outcome.is_some());
    for event in driver.events().expect("event receiver").try_iter() {
        assert!(
            !matches!(event, DriverEvent::Ephemeral(EphemeralDelta::Telemetry(_))),
            "telemetry must not follow a recovered terminal: {event:?}"
        );
    }
    driver.shutdown().expect("driver shutdown");
}

#[test]
fn output_recovery_promotes_and_retires_original() {
    let directory = TestDirectory::new("replace");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"original-content").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let mut state = DurableState::default();
    let transaction = stage_output(
        &manager,
        &mut state,
        RunId(5),
        &input,
        &final_path,
        Replacement::RetireOriginal,
        false,
    );
    assert!(transaction.staging.ends_with(".input.crfty-5.part.mkv"));
    fs::write(&transaction.staging, b"encoded-content").expect("fake encode");
    let ready = manager.mark_ready(&transaction).expect("ready output");
    fold_output(&mut state, ready);

    let committed = manager
        .recover_once(&current(&state, RunId(5)))
        .expect("promote output")
        .expect("committed delta");
    assert!(matches!(committed, OutputDelta::OutputCommitted { .. }));
    fold_output(&mut state, committed);
    assert_eq!(
        fs::read(&final_path).expect("final bytes"),
        b"encoded-content"
    );
    assert!(input.exists());

    let intent = manager
        .recover_once(&current(&state, RunId(5)))
        .expect("retire intent")
        .expect("intent delta");
    assert_eq!(
        intent,
        OutputDelta::RetireOriginalIntent { run_id: RunId(5) }
    );
    fold_output(&mut state, intent);
    let retired = manager
        .recover_once(&current(&state, RunId(5)))
        .expect("retire original")
        .expect("retired delta");
    fold_output(&mut state, retired);
    assert!(!input.exists());
    assert!(current(&state, RunId(5)).is_settled());
}

#[test]
fn invalid_partial_staging_is_cleaned_without_media_validation() {
    let directory = TestDirectory::new("invalid-partial");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"input bytes").expect("input fixture");
    let manager = OutputManager::new(RejectingMediaInspector);
    let mut state = DurableState::default();
    let transaction = stage_output(
        &manager,
        &mut state,
        RunId(77),
        &input,
        &final_path,
        Replacement::RetireOriginal,
        false,
    );
    fs::write(&transaction.staging, b"not a valid media container").expect("partial staging");
    let intent = manager
        .abandon_intent(&current(&state, RunId(77)))
        .expect("identity-only abandonment intent");
    fold(&mut state, &DurableDelta::Output(intent));
    let abandoning = state.outputs.get(&RunId(77)).expect("transaction");
    let abandoned = manager
        .recover_once(abandoning)
        .expect("identity-only cleanup")
        .expect("abandoned delta");
    assert!(matches!(abandoned, OutputDelta::Abandoned { .. }));
    assert!(!transaction.staging.exists());
}

#[test]
fn engine_startup_recovers_an_active_partial_staging_transaction() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("engine-startup-recovery");
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

    let settings = journal_active_output_run(&journal_path, &input, &transaction, Some(initial));

    let executable = std::env::current_exe().expect("test executable");
    let engine = EngineRuntime::start(EngineConfig {
        journal_path,
        config_path: directory.path().join("config.json"),
        vendor_root: directory.path().join("vendor"),
        tools: fixture_tools(MediaTools {
            ffmpeg: executable.clone(),
            ffprobe: executable,
        }),
        execution: settings,
    })
    .expect("engine startup recovery");
    let DriverEvent::Snapshot(snapshot) = engine.events.recv().expect("recovered snapshot") else {
        panic!("expected recovered snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue.first().expect("queue item").state,
            crfty_core::QueueItemState::Finished(ItemOutcome::Stopped)
        ),
        "unexpected recovered snapshot: {snapshot:?}"
    );
    assert!(!transaction.staging.exists());
    // Recovery may forward an idempotent TelemetryCleared after the snapshot,
    // but live telemetry must never follow a recovered terminal.
    while let Ok(event) = engine.events.try_recv() {
        assert!(
            !matches!(event, DriverEvent::Ephemeral(EphemeralDelta::Telemetry(_))),
            "telemetry after recovered terminal: {event:?}"
        );
    }
    engine.shutdown().expect("engine shutdown");
}

/// Journals the durable trail of an active convert run whose output
/// transaction reached `OutputStarted`, optionally followed by
/// `StagingCreated`, mirroring the coordinator's journal order.
fn journal_active_output_run(
    journal_path: &Path,
    input: &Path,
    transaction: &crfty_core::OutputTransaction,
    staging_created: Option<DestructiveIdentity>,
) -> ExecutionSettings {
    let settings = execution();
    let run_id = transaction.run_id;
    let mut commands = vec![
        Command::Queue(QueueCommand::Add {
            item_id: QueueItemId(1),
            input: input.to_path_buf(),
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Replace,
        }),
        Command::System(SystemCommand::ToolsDiscovered {
            availability: fixture_available(),
            update_available: false,
        }),
        Command::Session(SessionCommand::Start),
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id,
        }),
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id,
            observation: None,
            execution: settings.clone(),
        }),
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id,
            at: UnixMillis(1_000),
        }),
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id,
            result: Box::new(fixture_analysis(&settings)),
        }),
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
            transaction: Box::new(transaction.clone()),
        })),
    ];
    if let Some(initial) = staging_created {
        commands.push(Command::Worker(WorkerCommand::Output(
            OutputDelta::StagingCreated { run_id, initial },
        )));
    }
    let mut state = AppState::default();
    let mut durable = Vec::new();
    for command in commands {
        let applied = apply(&mut state, command);
        assert!(!matches!(applied.reply, Reply::Rejected { .. }));
        durable.extend(applied.durable);
    }
    let (mut writer, _replay) = JournalWriter::open(journal_path).expect("journal writer");
    writer
        .append_batch(&durable)
        .expect("recovery fixture batch");
    settings
}

#[test]
fn engine_startup_abandons_intent_when_staging_was_never_created() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("engine-startup-no-staging");
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
    // Crash window: `OutputStarted` is durable but the staging file was never
    // created.
    let settings = journal_active_output_run(&journal_path, &input, &transaction, None);

    let executable = std::env::current_exe().expect("test executable");
    let engine = EngineRuntime::start(EngineConfig {
        journal_path,
        config_path: directory.path().join("config.json"),
        vendor_root: directory.path().join("vendor"),
        tools: fixture_tools(MediaTools {
            ffmpeg: executable.clone(),
            ffprobe: executable,
        }),
        execution: settings,
    })
    .expect("engine startup recovery");
    let DriverEvent::Snapshot(snapshot) = engine.events.recv().expect("recovered snapshot") else {
        panic!("expected recovered snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue.first().expect("queue item").state,
            crfty_core::QueueItemState::Finished(ItemOutcome::Stopped)
        ),
        "unexpected recovered snapshot: {snapshot:?}"
    );
    assert_no_staging_leftovers(directory.path());
    engine.shutdown().expect("engine shutdown");
}

#[test]
fn engine_startup_removes_staging_left_before_staging_created_was_durable() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("engine-startup-unjournaled-staging");
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
    // Crash window: the staging file exists on disk but `StagingCreated`
    // never reached the journal, so no identity was recorded for it.
    let _initial = manager
        .create_staging(&transaction)
        .expect("create staging");
    fs::write(&transaction.staging, b"crash-left partial bytes").expect("partial staging");
    let settings = journal_active_output_run(&journal_path, &input, &transaction, None);

    let executable = std::env::current_exe().expect("test executable");
    let engine = EngineRuntime::start(EngineConfig {
        journal_path,
        config_path: directory.path().join("config.json"),
        vendor_root: directory.path().join("vendor"),
        tools: fixture_tools(MediaTools {
            ffmpeg: executable.clone(),
            ffprobe: executable,
        }),
        execution: settings,
    })
    .expect("engine startup recovery");
    let DriverEvent::Snapshot(snapshot) = engine.events.recv().expect("recovered snapshot") else {
        panic!("expected recovered snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue.first().expect("queue item").state,
            crfty_core::QueueItemState::Finished(ItemOutcome::Stopped)
        ),
        "unexpected recovered snapshot: {snapshot:?}"
    );
    assert!(!transaction.staging.exists());
    assert_no_staging_leftovers(directory.path());
    engine.shutdown().expect("engine shutdown");
}

fn assert_no_staging_leftovers(directory: &Path) {
    let leftovers = fs::read_dir(directory)
        .expect("read output directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().contains(".part."))
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "leaked staging files: {leftovers:?}");
}

struct SettledSuccessFixture {
    journal_path: PathBuf,
    input: PathBuf,
    final_path: PathBuf,
    staging: PathBuf,
}

/// Journal a run whose output transaction is fully settled — every output
/// delta durable, files promoted on disk — but whose `Terminal` record was
/// lost to a crash.
fn settled_success_journal(
    directory: &TestDirectory,
    replacement: Replacement,
) -> SettledSuccessFixture {
    let journal_path = directory.path().join("state.jsonl");
    let input = directory.path().join("input.mp4");
    fs::write(&input, b"input bytes").expect("input fixture");
    let (final_path, output_target) = match replacement {
        Replacement::RetireOriginal => (directory.path().join("input.mkv"), OutputTarget::Replace),
        Replacement::KeepOriginal => (
            directory.path().join("input.av1.mkv"),
            OutputTarget::Suffix {
                suffix: ".av1".to_owned(),
            },
        ),
    };
    let manager = OutputManager::new(FixtureByteInspector);
    let transaction = manager
        .plan(RunId(3), &input, &final_path, replacement, false)
        .expect("plan transaction");
    let initial = manager
        .create_staging(&transaction)
        .expect("create staging");
    fs::write(&transaction.staging, b"encoded output bytes").expect("staging bytes");
    let staging_identity = FixtureByteInspector
        .inspect_media(&transaction.staging)
        .expect("staging identity");
    fs::rename(&transaction.staging, &final_path).expect("promote staging");
    if replacement == Replacement::RetireOriginal {
        fs::remove_file(&input).expect("retire original");
    }

    let settings = execution();
    let mut state = AppState::default();
    let mut durable = Vec::new();
    let mut commands = vec![
        Command::Queue(QueueCommand::Add {
            item_id: QueueItemId(1),
            input: input.clone(),
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target,
        }),
        Command::System(SystemCommand::ToolsDiscovered {
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
            result: Box::new(fixture_analysis(&settings)),
        }),
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
            transaction: Box::new(transaction.clone()),
        })),
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial,
        })),
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: staging_identity.clone(),
        })),
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputCommitted {
            run_id: RunId(3),
            final_identity: staging_identity,
        })),
    ];
    if replacement == Replacement::RetireOriginal {
        commands.push(Command::Worker(WorkerCommand::Output(
            OutputDelta::RetireOriginalIntent { run_id: RunId(3) },
        )));
        commands.push(Command::Worker(WorkerCommand::Output(
            OutputDelta::OriginalRetired { run_id: RunId(3) },
        )));
    }
    for command in commands {
        let applied = apply(&mut state, command);
        assert!(
            !matches!(applied.reply, Reply::Rejected { .. }),
            "fixture command rejected: {:?}",
            applied.reply
        );
        durable.extend(applied.durable);
    }
    assert!(
        state
            .durable
            .outputs
            .get(&RunId(3))
            .expect("settled transaction")
            .is_settled()
    );
    let (mut writer, _replay) = JournalWriter::open(&journal_path).expect("journal writer");
    writer
        .append_batch(&durable)
        .expect("settled fixture batch");
    drop(writer);
    SettledSuccessFixture {
        journal_path,
        input,
        final_path,
        staging: transaction.staging,
    }
}

fn recover_settled_success(directory: &TestDirectory, fixture: &SettledSuccessFixture) {
    let executable = std::env::current_exe().expect("test executable");
    let engine = EngineRuntime::start(EngineConfig {
        journal_path: fixture.journal_path.clone(),
        config_path: directory.path().join("config.json"),
        vendor_root: directory.path().join("vendor"),
        tools: fixture_tools(MediaTools {
            ffmpeg: executable.clone(),
            ffprobe: executable,
        }),
        execution: execution(),
    })
    .expect("engine startup recovery");
    let DriverEvent::Snapshot(snapshot) = engine.events.recv().expect("recovered snapshot") else {
        panic!("expected recovered snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue.first().expect("queue item").state,
            crfty_core::QueueItemState::Finished(ItemOutcome::Converted(
                crfty_core::CompletionEvidence::RecoveredAtStartup
            ))
        ),
        "settled success must recover as converted: {snapshot:?}"
    );
    let run = snapshot
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("conversion run");
    assert!(matches!(
        run.outcome,
        Some(ItemOutcome::Converted(
            crfty_core::CompletionEvidence::RecoveredAtStartup
        ))
    ));
    assert_eq!(run.started_at, Some(UnixMillis(1_000)));
    assert!(run.finished_at.is_some());
    assert!(fixture.final_path.exists());
    assert!(!fixture.staging.exists());
    while let Ok(event) = engine.events.try_recv() {
        assert!(
            !matches!(event, DriverEvent::Ephemeral(EphemeralDelta::Telemetry(_))),
            "telemetry after recovered terminal: {event:?}"
        );
    }
    engine.shutdown().expect("engine shutdown");
}

#[test]
fn startup_recovery_labels_settled_keep_original_success_converted() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("settled-keep-original-recovery");
    let fixture = settled_success_journal(&directory, Replacement::KeepOriginal);
    recover_settled_success(&directory, &fixture);
    assert!(fixture.input.exists());
}

#[test]
fn startup_recovery_labels_settled_retired_original_success_converted() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("settled-retired-original-recovery");
    let fixture = settled_success_journal(&directory, Replacement::RetireOriginal);
    recover_settled_success(&directory, &fixture);
    assert!(!fixture.input.exists());
}

fn fixture_analysis(settings: &ExecutionSettings) -> AnalysisResult {
    AnalysisResult {
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
    }
}

#[derive(Debug, Clone, Copy)]
struct RejectingMediaInspector;

impl ArtifactInspector for RejectingMediaInspector {
    fn inspect_file(&self, path: &Path) -> std::io::Result<DestructiveIdentity> {
        FixtureByteInspector.inspect_file(path)
    }

    fn inspect_media(&self, _path: &Path) -> std::io::Result<ArtifactIdentity> {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "fixture rejects media",
        ))
    }

    fn verify_output(&self, path: &Path) -> std::io::Result<ArtifactIdentity> {
        self.inspect_media(path)
    }
}

#[test]
fn abandonment_intent_authorizes_only_the_observed_partial_staging() {
    let directory = TestDirectory::new("abandon");
    let input = directory.path().join("input.mkv");
    let final_path = directory.path().join("output.mkv");
    fs::write(&input, b"original").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let mut state = DurableState::default();
    let transaction = stage_output(
        &manager,
        &mut state,
        RunId(20),
        &input,
        &final_path,
        Replacement::KeepOriginal,
        false,
    );
    fs::write(&transaction.staging, b"partial-encode").expect("partial staging");
    let intent = manager
        .abandon_intent(&current(&state, RunId(20)))
        .expect("abandon intent");
    fold_output(&mut state, intent);
    let abandoned = manager
        .recover_once(&current(&state, RunId(20)))
        .expect("abandon recovery")
        .expect("abandoned delta");
    assert!(matches!(abandoned, OutputDelta::Abandoned { .. }));
    assert!(!current(&state, RunId(20)).staging.exists());
}

#[test]
fn recovery_recognizes_crashes_after_rename_and_delete() {
    let directory = TestDirectory::new("crash-boundaries");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"original").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let mut state = DurableState::default();
    let staging = stage_output(
        &manager,
        &mut state,
        RunId(8),
        &input,
        &final_path,
        Replacement::RetireOriginal,
        false,
    )
    .staging;
    fs::write(&staging, b"encoded").expect("fake encode");
    let ready = manager
        .mark_ready(&current(&state, RunId(8)))
        .expect("ready");
    fold_output(&mut state, ready);

    fs::rename(&staging, &final_path).expect("simulated promotion before acknowledgement");
    let committed = manager
        .recover_once(&current(&state, RunId(8)))
        .expect("recover promotion")
        .expect("commit acknowledgement");
    fold_output(&mut state, committed);
    let intent = manager
        .recover_once(&current(&state, RunId(8)))
        .expect("intent")
        .expect("intent delta");
    fold_output(&mut state, intent);

    fs::remove_file(&input).expect("simulated deletion before acknowledgement");
    let retired = manager
        .recover_once(&current(&state, RunId(8)))
        .expect("recover deletion")
        .expect("retirement acknowledgement");
    assert!(matches!(retired, OutputDelta::OriginalRetired { .. }));
}

#[test]
fn same_path_replacement_preserves_hardlink_sibling() {
    let directory = TestDirectory::new("hardlink");
    let input = directory.path().join("video.mkv");
    let sibling = directory.path().join("sibling.mkv");
    fs::write(&input, b"original").expect("input fixture");
    fs::hard_link(&input, &sibling).expect("hardlink sibling");
    let manager = OutputManager::new(FixtureByteInspector);
    let mut state = DurableState::default();
    let staging = stage_output(
        &manager,
        &mut state,
        RunId(11),
        &input,
        &input,
        Replacement::KeepOriginal,
        false,
    )
    .staging;
    fs::write(&staging, b"encoded").expect("fake encode");
    let ready = manager
        .mark_ready(&current(&state, RunId(11)))
        .expect("ready");
    fold_output(&mut state, ready);
    let committed = manager
        .recover_once(&current(&state, RunId(11)))
        .expect("promotion")
        .expect("commit delta");
    fold_output(&mut state, committed);
    assert_eq!(fs::read(&input).expect("new input bytes"), b"encoded");
    assert_eq!(fs::read(&sibling).expect("sibling bytes"), b"original");
}

#[test]
fn restage_recreates_missing_staging_and_the_ready_pin_follows() {
    let directory = TestDirectory::new("restage");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"original").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let mut state = DurableState::default();
    let staged = stage_output(
        &manager,
        &mut state,
        RunId(6),
        &input,
        &final_path,
        Replacement::KeepOriginal,
        false,
    );
    // The failed hardware attempt's adapter cleanup removed staging entirely.
    fs::remove_file(&staged.staging).expect("simulate adapter cleanup");
    let initial = manager
        .restage(&current(&state, RunId(6)))
        .expect("restaged identity");
    fold_output(
        &mut state,
        OutputDelta::StagingCreated {
            run_id: RunId(6),
            initial,
        },
    );
    let transaction = current(&state, RunId(6));
    assert!(transaction.staging.exists());
    fs::write(&transaction.staging, b"encoded-by-retry").expect("retry encode");
    let ready = manager
        .mark_ready(&transaction)
        .expect("ready after restage");
    fold_output(&mut state, ready);
    let committed = manager
        .recover_once(&current(&state, RunId(6)))
        .expect("promotion")
        .expect("commit delta");
    assert!(matches!(committed, OutputDelta::OutputCommitted { .. }));
    fold_output(&mut state, committed);
    assert_eq!(
        fs::read(&final_path).expect("promoted retry output"),
        b"encoded-by-retry"
    );

    // A settled transaction refuses to restage — retry-after-abandonment is
    // unrepresentable, and this manager-side guard mirrors the ledger's.
    assert!(manager.restage(&current(&state, RunId(6))).is_err());
}

#[test]
fn output_preparation_enforces_overwrite_policy_before_staging() {
    let directory = TestDirectory::new("overwrite-policy");
    let input = directory.path().join("input.mkv");
    let final_path = directory.path().join("output.mkv");
    fs::write(&input, b"input").expect("input fixture");
    fs::write(&final_path, b"existing").expect("existing output fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let error = manager
        .plan(
            RunId(30),
            &input,
            &final_path,
            Replacement::KeepOriginal,
            false,
        )
        .expect_err("overwrite-disabled planning must fail");
    assert!(error.is_destination_exists());
    assert_eq!(fs::read(&final_path).expect("existing output"), b"existing");
    assert!(!directory.path().join(".output.crfty-30.part.mkv").exists());
}

#[test]
fn planning_creates_no_filesystem_entries() {
    let directory = TestDirectory::new("plan-no-writes");
    let input = directory.path().join("input.mkv");
    let final_path = directory.path().join("output.mkv");
    fs::write(&input, b"input").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let planned = manager
        .plan(
            RunId(31),
            &input,
            &final_path,
            Replacement::KeepOriginal,
            false,
        )
        .expect("plan output");
    assert!(!planned.staging.exists());
    let entries = fs::read_dir(directory.path())
        .expect("read directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![std::ffi::OsString::from("input.mkv")]);
}

#[test]
fn changed_destination_becomes_conflict_without_deletion() {
    let directory = TestDirectory::new("destination-conflict");
    let input = directory.path().join("input.mkv");
    let final_path = directory.path().join("output.mkv");
    fs::write(&input, b"original").expect("input fixture");
    fs::write(&final_path, b"preimage").expect("destination fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let mut state = DurableState::default();
    let staging = stage_output(
        &manager,
        &mut state,
        RunId(12),
        &input,
        &final_path,
        Replacement::KeepOriginal,
        true,
    )
    .staging;
    fs::write(&staging, b"encoded").expect("fake encode");
    let ready = manager
        .mark_ready(&current(&state, RunId(12)))
        .expect("ready");
    fold_output(&mut state, ready);
    fs::write(&final_path, b"changed-by-someone-else").expect("change destination");
    let conflict = manager
        .recover_once(&current(&state, RunId(12)))
        .expect("conflict decision")
        .expect("conflict delta");
    assert!(matches!(conflict, OutputDelta::Conflict { .. }));
    assert_eq!(
        fs::read(&final_path).expect("unchanged destination"),
        b"changed-by-someone-else"
    );
    assert!(staging.exists());
}

fn fold_output(state: &mut DurableState, delta: OutputDelta) {
    fold(state, &DurableDelta::Output(delta));
}

/// Plans a transaction, folds its `OutputStarted` intent, creates the staging
/// file, and folds `StagingCreated` — the same order the coordinator journals.
fn stage_output<I: ArtifactInspector>(
    manager: &OutputManager<I>,
    state: &mut DurableState,
    run_id: RunId,
    input: &Path,
    final_path: &Path,
    replacement: Replacement,
    overwrite_existing: bool,
) -> crfty_core::OutputTransaction {
    let planned = manager
        .plan(run_id, input, final_path, replacement, overwrite_existing)
        .expect("plan output");
    fold_output(
        state,
        OutputDelta::OutputStarted {
            transaction: Box::new(planned),
        },
    );
    let initial = manager
        .create_staging(&current(state, run_id))
        .expect("create staging");
    fold_output(state, OutputDelta::StagingCreated { run_id, initial });
    current(state, run_id)
}

fn current(state: &DurableState, run_id: RunId) -> crfty_core::OutputTransaction {
    state
        .outputs
        .get(&run_id)
        .expect("output transaction")
        .clone()
}
