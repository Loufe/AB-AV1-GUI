#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crfty_core::{
    AnalysisProfile, AnalysisResult, AppState, ArtifactIdentity, ClaimId, Command, Crf,
    DestructiveIdentity, DurableDelta, DurableState, EphemeralDelta, ExecutionSettings,
    ItemOutcome, JobPhase, JobProgress, Operation, OutputDelta, OutputTarget, QueueCommand,
    QueueItemId, Replacement, Reply, RunId, SearchMeasurement, SessionCommand, Telemetry,
    ToolRevisions, VmafScore, WorkerCommand, apply, fold, replay,
};
use crfty_engine::{
    ab_av1::MediaTools,
    coordinator::{EngineConfig, EngineRuntime},
    driver::{DriverEvent, DriverHandle},
    journal::JournalWriter,
    output::{ArtifactInspector, FixtureByteInspector, OutputManager},
};

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

fn add(item_id: QueueItemId) -> Command {
    Command::Queue(QueueCommand::Add {
        item_id,
        input: PathBuf::from("video.mkv"),
        operation: Operation::Convert,
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
fn journal_lock_is_exclusive_and_records_replay() {
    let directory = TestDirectory::new("journal-lock");
    let path = directory.path().join("state.jsonl");
    let (mut writer, initial) = JournalWriter::open(&path).expect("first journal writer");
    assert_eq!(initial.next_sequence.0, 0);
    assert!(JournalWriter::open(&path).is_err());
    let delta = DurableDelta::QueueAdded {
        item: crfty_core::QueueItem {
            id: QueueItemId(1),
            input: PathBuf::from("video.mkv"),
            operation: Operation::Convert,
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
    let driver = DriverHandle::start(&path).expect("driver");
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

    let restarted = DriverHandle::start(&path).expect("restarted driver");
    let DriverEvent::Snapshot(snapshot) = restarted
        .events()
        .expect("event receiver")
        .recv()
        .expect("replayed snapshot")
    else {
        panic!("expected replayed snapshot");
    };
    assert_eq!(snapshot.queue.len(), 1);
    assert_eq!(snapshot.queue[0].id, QueueItemId(7));
    restarted.shutdown().expect("restarted shutdown");
}

#[test]
fn corrupted_journal_starts_degraded_and_rejects_mutation() {
    let directory = TestDirectory::new("degraded");
    let path = directory.path().join("state.jsonl");
    fs::write(&path, b"not-json\n").expect("corrupt journal fixture");
    let driver = DriverHandle::start(&path).expect("degraded driver");
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
    let driver = DriverHandle::start(path).expect("driver");
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
            progress: JobProgress::EncodePositionMs(sequence),
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
                outcome: ItemOutcome::Failed {
                    message: "fixture".to_owned(),
                },
                final_telemetry: Some(Telemetry {
                    run_id: RunId(3),
                    sequence: terminal_sequence,
                    phase: JobPhase::Finalizing,
                    progress: JobProgress::EncodePositionMs(100),
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
fn output_recovery_promotes_and_retires_original() {
    let directory = TestDirectory::new("replace");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"original-content").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let started = manager
        .prepare(RunId(5), &input, &final_path, Replacement::RetireOriginal)
        .expect("prepare output");
    let mut state = DurableState::default();
    fold_output(
        &mut state,
        OutputDelta::EncodeStarted {
            transaction: Box::new(started),
        },
    );
    let transaction = current(&state, RunId(5));
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
    let started = manager
        .prepare(RunId(77), &input, &final_path, Replacement::RetireOriginal)
        .expect("prepare output");
    fs::write(&started.staging, b"not a valid media container").expect("partial staging");
    let intent = manager
        .abandon_intent(&started)
        .expect("identity-only abandonment intent");
    let mut state = DurableState::default();
    state.outputs.insert(started.run_id, started.clone());
    fold(&mut state, &DurableDelta::Output(intent));
    let abandoning = state.outputs.get(&started.run_id).expect("transaction");
    let abandoned = manager
        .recover_once(abandoning)
        .expect("identity-only cleanup")
        .expect("abandoned delta");
    assert!(matches!(abandoned, OutputDelta::Abandoned { .. }));
    assert!(!started.staging.exists());
}

#[test]
fn engine_startup_recovers_an_active_partial_staging_transaction() {
    let directory = TestDirectory::new("engine-startup-recovery");
    let journal_path = directory.path().join("state.jsonl");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"input bytes").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let transaction = manager
        .prepare(RunId(3), &input, &final_path, Replacement::RetireOriginal)
        .expect("prepare transaction");
    fs::write(&transaction.staging, b"crash-left partial bytes").expect("partial staging");

    let settings = execution();
    let mut state = AppState::default();
    let mut durable = Vec::new();
    for command in [
        Command::Queue(QueueCommand::Add {
            item_id: QueueItemId(1),
            input: input.clone(),
            operation: Operation::Convert,
            output_target: OutputTarget::Replace,
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
        }),
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(fixture_analysis(&settings)),
        }),
        Command::Worker(WorkerCommand::Output(OutputDelta::EncodeStarted {
            transaction: Box::new(transaction.clone()),
        })),
    ] {
        let applied = apply(&mut state, command);
        assert!(!matches!(applied.reply, Reply::Rejected { .. }));
        durable.extend(applied.durable);
    }
    let (mut writer, _replay) = JournalWriter::open(&journal_path).expect("journal writer");
    writer
        .append_batch(&durable)
        .expect("recovery fixture batch");
    drop(writer);

    let executable = std::env::current_exe().expect("test executable");
    let engine = EngineRuntime::start(EngineConfig {
        journal_path,
        media_tools: MediaTools {
            ffmpeg: executable.clone(),
            ffprobe: executable,
        },
        execution: settings,
    })
    .expect("engine startup recovery");
    let DriverEvent::Snapshot(snapshot) = engine.events.recv().expect("recovered snapshot") else {
        panic!("expected recovered snapshot");
    };
    assert!(
        matches!(
            snapshot.queue.first().expect("queue item").state,
            crfty_core::QueueItemState::Finished(ItemOutcome::Stopped)
        ),
        "unexpected recovered snapshot: {snapshot:?}"
    );
    assert!(!transaction.staging.exists());
    engine.shutdown().expect("engine shutdown");
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
    let started = manager
        .prepare(RunId(20), &input, &final_path, Replacement::KeepOriginal)
        .expect("prepare");
    fs::write(&started.staging, b"partial-encode").expect("partial staging");
    let intent = manager.abandon_intent(&started).expect("abandon intent");
    let mut state = DurableState::default();
    fold_output(
        &mut state,
        OutputDelta::EncodeStarted {
            transaction: Box::new(started),
        },
    );
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
    let started = manager
        .prepare(RunId(8), &input, &final_path, Replacement::RetireOriginal)
        .expect("prepare");
    let mut state = DurableState::default();
    fold_output(
        &mut state,
        OutputDelta::EncodeStarted {
            transaction: Box::new(started),
        },
    );
    let staging = current(&state, RunId(8)).staging;
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
    let started = manager
        .prepare(RunId(11), &input, &input, Replacement::KeepOriginal)
        .expect("prepare same-path replacement");
    let mut state = DurableState::default();
    fold_output(
        &mut state,
        OutputDelta::EncodeStarted {
            transaction: Box::new(started),
        },
    );
    let staging = current(&state, RunId(11)).staging;
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
fn changed_destination_becomes_conflict_without_deletion() {
    let directory = TestDirectory::new("destination-conflict");
    let input = directory.path().join("input.mkv");
    let final_path = directory.path().join("output.mkv");
    fs::write(&input, b"original").expect("input fixture");
    fs::write(&final_path, b"preimage").expect("destination fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let started = manager
        .prepare(RunId(12), &input, &final_path, Replacement::KeepOriginal)
        .expect("prepare");
    let mut state = DurableState::default();
    fold_output(
        &mut state,
        OutputDelta::EncodeStarted {
            transaction: Box::new(started),
        },
    );
    let staging = current(&state, RunId(12)).staging;
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

fn current(state: &DurableState, run_id: RunId) -> crfty_core::OutputTransaction {
    state
        .outputs
        .get(&run_id)
        .expect("output transaction")
        .clone()
}
