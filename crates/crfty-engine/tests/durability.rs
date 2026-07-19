#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crfty_core::{
    AnalysisProfile, ClaimId, Command, DurableDelta, DurableState, EphemeralDelta,
    ExecutionSettings, ItemOutcome, JobPhase, Operation, OutputDelta, OutputTarget, QueueCommand,
    QueueItemId, Replacement, Reply, RunId, SessionCommand, Telemetry, ToolRevisions,
    WorkerCommand, fold, replay,
};
use crfty_engine::{
    driver::{DriverEvent, DriverHandle},
    journal::JournalWriter,
    output::{FixtureByteInspector, OutputManager},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn execution() -> ExecutionSettings {
    ExecutionSettings::production(
        AnalysisProfile::production(
            ToolRevisions {
                ab_av1: "fixture".to_owned(),
                ffmpeg: "fixture".to_owned(),
                encoder: "fixture".to_owned(),
            },
            true,
        ),
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
            .submit(Command::Worker(WorkerCommand::ClaimNext {
                claim_id: ClaimId(2),
                run_id: RunId(3),
                execution: execution(),
            }))
            .expect("claim reply"),
        Reply::Claimed(Some(_))
    ));
    for sequence in 0..100_000 {
        driver.commands.publish_telemetry(Telemetry {
            run_id: RunId(3),
            sequence,
            phase: JobPhase::Encoding,
            completed_units: sequence,
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
                    completed_units: 100,
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
