use std::path::PathBuf;

use proptest::prelude::*;

use crate::{
    AnalysisProfile, AnalysisResult, AppState, ArtifactIdentity, ArtifactObservation, ClaimId,
    Command, ContentKey, Crf, DurableDelta, Effect, ExecutionSettings, FileSystemFacts,
    ItemOutcome, JOURNAL_SCHEMA_VERSION, JobPhase, JournalEnvelope, JournalSequence, Operation,
    OutputDelta, OutputRecoveryAction, OutputState, OutputTarget, OutputTransaction, QueueCommand,
    QueueItemId, QueueItemState, Replacement, Reply, RunId, SearchMeasurement, SessionCommand,
    SessionState, Telemetry, ToolRevisions, VmafScore, WorkerCommand, apply, encode_record,
    recover_output, replay,
};

fn execution() -> ExecutionSettings {
    ExecutionSettings::production(
        AnalysisProfile::production(
            ToolRevisions {
                ab_av1: "test-ab-av1".to_owned(),
                ffmpeg: "test-ffmpeg".to_owned(),
                encoder: "test-svt".to_owned(),
            },
            true,
        ),
        false,
    )
}

fn analysis() -> AnalysisResult {
    let execution = execution();
    AnalysisResult {
        requested_target: execution.requested_target,
        successful_target: execution.requested_target,
        fallback_floor: execution.fallback_floor,
        fallback_step: execution.fallback_step,
        failed_attempts: Vec::new(),
        measurement: SearchMeasurement {
            crf: Crf(30_000),
            score: VmafScore(9_500),
            predicted_size: 1_000,
            predicted_percent_basis_points: 5_000,
            predicted_duration_ms: 60_000,
            from_cache: false,
        },
        profile: execution.profile,
    }
}

fn add_command(item_id: QueueItemId, input: impl Into<PathBuf>) -> Command {
    Command::Queue(QueueCommand::Add {
        item_id,
        input: input.into(),
        operation: Operation::Convert,
        output_target: OutputTarget::Replace,
    })
}

fn identity(name: &str, size: u64) -> ArtifactIdentity {
    ArtifactIdentity {
        content_key: ContentKey(name.to_owned()),
        size,
        modified_ns: Some(u128::from(size)),
        file_id: Some(format!("file-{name}")),
    }
}

fn transaction(state: OutputState, replacement: Replacement) -> OutputTransaction {
    OutputTransaction {
        run_id: RunId(9),
        input: PathBuf::from("input.mkv"),
        input_identity: identity("input", 10),
        staging: PathBuf::from(".output.mkv.crfty-9.part"),
        initial_staging_identity: identity("empty", 0),
        final_path: PathBuf::from("output.mkv"),
        final_preimage: None,
        replacement,
        state,
    }
}

#[test]
fn reducer_enforces_session_claim_and_terminal_ordering() {
    let mut state = AppState::default();
    let add = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    assert_eq!(add.reply, Reply::Accepted);
    let start = apply(&mut state, Command::Session(SessionCommand::Start));
    assert_eq!(start.effects, vec![Effect::StartWorker]);
    let claim = apply(
        &mut state,
        Command::Worker(WorkerCommand::ClaimNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
            execution: execution(),
        }),
    );
    assert!(matches!(claim.reply, Reply::Claimed(Some(_))));
    let stale = apply(
        &mut state,
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(99),
            run_id: RunId(3),
        }),
    );
    assert!(matches!(stale.reply, Reply::Rejected { .. }));
    assert!(matches!(
        state.durable.queue.first().expect("queue item").state,
        QueueItemState::Claimed { .. }
    ));
    let terminal = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed {
                message: "fixture".to_owned(),
            },
            final_telemetry: Some(Telemetry {
                run_id: RunId(3),
                sequence: 20,
                phase: JobPhase::Encoding,
                completed_units: 100,
            }),
        }),
    );
    assert_eq!(terminal.reply, Reply::Accepted);
    assert_eq!(state.session, SessionState::Running);
    let finished = apply(&mut state, Command::Worker(WorkerCommand::Finished));
    assert_eq!(finished.reply, Reply::Accepted);
    assert_eq!(state.session, SessionState::Idle);
    assert_eq!(
        state
            .telemetry
            .get(&RunId(3))
            .expect("terminal telemetry")
            .sequence,
        20
    );
}

#[test]
fn claim_next_is_atomic_and_uses_current_queue_order() {
    let mut state = AppState::default();
    let _first = apply(&mut state, add_command(QueueItemId(1), "first.mkv"));
    let _second = apply(&mut state, add_command(QueueItemId(2), "second.mkv"));
    let moved = apply(
        &mut state,
        Command::Queue(QueueCommand::Move {
            item_id: QueueItemId(2),
            before: Some(QueueItemId(1)),
        }),
    );
    assert_eq!(moved.reply, Reply::Accepted);
    let _started = apply(&mut state, Command::Session(SessionCommand::Start));
    let claimed = apply(
        &mut state,
        Command::Worker(WorkerCommand::ClaimNext {
            claim_id: ClaimId(10),
            run_id: RunId(11),
            execution: execution(),
        }),
    );
    let Reply::Claimed(Some(job)) = claimed.reply else {
        panic!("expected an atomic claim");
    };
    assert_eq!(job.spec.item_id, QueueItemId(2));

    let competing = apply(
        &mut state,
        Command::Worker(WorkerCommand::ClaimNext {
            claim_id: ClaimId(12),
            run_id: RunId(13),
            execution: execution(),
        }),
    );
    assert!(matches!(competing.reply, Reply::Rejected { .. }));
}

#[test]
fn stop_after_current_does_not_kill_but_force_stop_does() {
    let mut state = active_state();
    let graceful = apply(
        &mut state,
        Command::Session(SessionCommand::StopAfterCurrent),
    );
    assert!(graceful.effects.is_empty());
    assert_eq!(state.session, SessionState::StopAfterCurrent);
    let forced = apply(&mut state, Command::Session(SessionCommand::ForceStop));
    assert_eq!(
        forced.effects,
        vec![Effect::KillActiveRun { run_id: RunId(3) }]
    );
}

#[test]
fn terminal_is_rejected_until_output_ledger_is_settled() {
    let mut state = active_state();
    let mut started = transaction(OutputState::Started, Replacement::KeepOriginal);
    started.run_id = RunId(3);
    let output = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::EncodeStarted {
            transaction: Box::new(started),
        })),
    );
    assert_eq!(output.reply, Reply::Accepted);
    let terminal = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Stopped,
            final_telemetry: None,
        }),
    );
    assert!(matches!(terminal.reply, Reply::Rejected { .. }));
}

#[test]
fn output_ledger_rejects_skipped_and_mismatched_transitions() {
    let mut state = active_state();
    let skipped = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputCommitted {
            run_id: RunId(3),
            final_identity: identity("encoded", 8),
        })),
    );
    assert!(matches!(skipped.reply, Reply::Rejected { .. }));

    let mut started = transaction(OutputState::Started, Replacement::KeepOriginal);
    started.run_id = RunId(3);
    let accepted = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::EncodeStarted {
            transaction: Box::new(started),
        })),
    );
    assert_eq!(accepted.reply, Reply::Accepted);
    let empty_ready = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: identity("empty", 0),
        })),
    );
    assert!(matches!(empty_ready.reply, Reply::Rejected { .. }));
}

#[test]
fn journal_ignores_only_an_unterminated_final_record() {
    let delta = DurableDelta::QueueAdded {
        item: crate::QueueItem {
            id: QueueItemId(1),
            input: PathBuf::from("one.mkv"),
            operation: Operation::Convert,
            output_target: OutputTarget::Replace,
            state: QueueItemState::Queued,
        },
    };
    let first = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(0),
        delta: delta.clone(),
    };
    let second = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(1),
        delta,
    };
    let mut bytes = encode_record(&first).expect("first record");
    let mut torn = encode_record(&second).expect("second record");
    assert_eq!(torn.pop(), Some(b'\n'));
    bytes.extend(torn);
    let replayed = replay(&bytes);
    assert!(replayed.ignored_torn_tail);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state.queue.len(), 1);
    assert_eq!(replayed.next_sequence, JournalSequence(1));
}

#[test]
fn journal_degrades_on_nonfinal_corruption() {
    let first = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(0),
        delta: DurableDelta::QueueAdded {
            item: crate::QueueItem {
                id: QueueItemId(1),
                input: PathBuf::from("one.mkv"),
                operation: Operation::Convert,
                output_target: OutputTarget::Replace,
                state: QueueItemState::Queued,
            },
        },
    };
    let mut bytes = encode_record(&first).expect("first record");
    bytes.extend(b"not-json\n");
    bytes.extend(encode_record(&first).expect("following record"));
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_some());
    assert!(!replayed.ignored_torn_tail);
    assert_eq!(replayed.state.queue.len(), 1);
}

#[test]
fn recovery_covers_every_destructive_boundary() {
    let started = transaction(OutputState::Started, Replacement::KeepOriginal);
    let started_facts = FileSystemFacts {
        staging: ArtifactObservation::Present(identity("empty", 0)),
        final_path: ArtifactObservation::Absent,
        original: ArtifactObservation::Present(identity("input", 10)),
    };
    assert!(matches!(
        recover_output(&started, &started_facts),
        OutputRecoveryAction::DeleteStaging { .. }
    ));

    let ready_identity = identity("encoded", 7);
    let ready = transaction(
        OutputState::Ready {
            staging_identity: ready_identity.clone(),
        },
        Replacement::KeepOriginal,
    );
    let before_rename = FileSystemFacts {
        staging: ArtifactObservation::Present(ready_identity.clone()),
        final_path: ArtifactObservation::Absent,
        original: ArtifactObservation::Present(identity("input", 10)),
    };
    assert!(matches!(
        recover_output(&ready, &before_rename),
        OutputRecoveryAction::Promote { .. }
    ));
    let after_rename = FileSystemFacts {
        staging: ArtifactObservation::Absent,
        final_path: ArtifactObservation::Present(ready_identity.clone()),
        original: ArtifactObservation::Present(identity("input", 10)),
    };
    assert!(matches!(
        recover_output(&ready, &after_rename),
        OutputRecoveryAction::Append(OutputDelta::OutputCommitted { .. })
    ));

    let retire = transaction(
        OutputState::RetireIntent {
            final_identity: ready_identity.clone(),
        },
        Replacement::RetireOriginal,
    );
    assert!(matches!(
        recover_output(&retire, &after_rename),
        OutputRecoveryAction::DeleteOriginal { .. }
    ));
    let after_delete = FileSystemFacts {
        original: ArtifactObservation::Absent,
        ..after_rename
    };
    assert!(matches!(
        recover_output(&retire, &after_delete),
        OutputRecoveryAction::Append(OutputDelta::OriginalRetired { .. })
    ));
}

proptest! {
    #[test]
    fn replay_equals_live_fold(operations in prop::collection::vec(any::<bool>(), 0..80)) {
        let mut live = AppState::default();
        let mut emitted = Vec::new();
        for (index, remove) in operations.into_iter().enumerate() {
            let item_id = QueueItemId(u64::try_from(index).expect("small generated index"));
            let added = apply(
                &mut live,
                add_command(item_id, PathBuf::from(format!("video-{index}.mkv"))),
            );
            emitted.extend(added.durable);
            if remove {
                let removed = apply(
                    &mut live,
                    Command::Queue(QueueCommand::Remove { item_id }),
                );
                emitted.extend(removed.durable);
            }
        }
        let mut bytes = Vec::new();
        for (sequence, delta) in emitted.into_iter().enumerate() {
            let envelope = JournalEnvelope {
                schema_version: JOURNAL_SCHEMA_VERSION,
                sequence: JournalSequence(u64::try_from(sequence).expect("small sequence")),
                delta,
            };
            bytes.extend(encode_record(&envelope).expect("serializable delta"));
        }
        let replayed = replay(&bytes);
        prop_assert!(replayed.corruption.is_none());
        prop_assert_eq!(replayed.state, live.durable);
    }
}

fn active_state() -> AppState {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = apply(&mut state, Command::Session(SessionCommand::Start));
    let _claimed = apply(
        &mut state,
        Command::Worker(WorkerCommand::ClaimNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
            execution: execution(),
        }),
    );
    let _running = apply(
        &mut state,
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    let _analysis = apply(
        &mut state,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(analysis()),
        }),
    );
    state
}
