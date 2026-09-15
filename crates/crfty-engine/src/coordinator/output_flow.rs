//! The output transaction: preparing a destination, opening the ledger
//! entry, settling it against the filesystem, and abandoning it on failure.

use std::path::PathBuf;

use crfty_core::{
    ClaimedJob, Command, CompletionEvidence, ConflictKind, DurableDelta, DurableState,
    FailureFacts, FailureKind, ItemOutcome, JobPhase, JobProgress, OutputDelta, OutputState,
    OutputTransaction, Replacement, RunId, SkipReason, Telemetry, WorkerCommand, fold,
};

use crate::{
    driver::CommandSender,
    failure::scrub_tail,
    output::{MediaArtifactInspector, OutputError, OutputManager},
};

use super::job::SuccessfulJob;
use super::output_path::resolve_output;
use super::require_accepted;
use super::session::{JobServices, PhaseTracker, map_progress, publish_phase, terminal};

pub(super) type MediaOutputManager = OutputManager<MediaArtifactInspector>;

pub(super) struct OutputDestination {
    final_path: PathBuf,
    replacement: Replacement,
}

pub(super) struct PreparedOutput {
    pub(super) manager: MediaOutputManager,
    pub(super) transaction: OutputTransaction,
}

pub(super) fn abandon_output(
    manager: &MediaOutputManager,
    commands: &CommandSender,
    job: &ClaimedJob,
    transaction: &OutputTransaction,
    outcome: ItemOutcome,
    final_telemetry: Option<Telemetry>,
    tracker: &mut PhaseTracker,
) -> Result<(), String> {
    let outcome = settle_abandoned(manager, commands, transaction)?.outcome(outcome);
    terminal(commands, job, tracker, outcome, final_telemetry)
}

pub(super) fn resolve_output_destination(
    commands: &CommandSender,
    job: &ClaimedJob,
    tracker: &mut PhaseTracker,
) -> Result<Option<OutputDestination>, String> {
    let (final_path, replacement) = match resolve_output(job) {
        Ok(resolved) => resolved,
        Err(message) => {
            terminal(
                commands,
                job,
                tracker,
                ItemOutcome::Failed(FailureFacts::new(FailureKind::OutputPrepare, message)),
                None,
            )?;
            return Ok(None);
        }
    };
    if let Some(parent) = final_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        terminal(
            commands,
            job,
            tracker,
            ItemOutcome::Failed(FailureFacts::new(
                FailureKind::OutputPrepare,
                format!("failed to create output directory: {error}"),
            )),
            None,
        )?;
        return Ok(None);
    }
    if final_path.exists() && !job.spec.execution.overwrite_existing && final_path != job.spec.input
    {
        terminal(
            commands,
            job,
            tracker,
            ItemOutcome::Skipped {
                reason: SkipReason::OutputExists,
            },
            None,
        )?;
        return Ok(None);
    }
    Ok(Some(OutputDestination {
        final_path,
        replacement,
    }))
}

pub(super) fn begin_output(
    services: JobServices<'_>,
    job: &ClaimedJob,
    destination: OutputDestination,
    tracker: &mut PhaseTracker,
) -> Result<Option<PreparedOutput>, String> {
    let commands = services.commands;
    let manager = OutputManager::new(MediaArtifactInspector::new(
        services.tools.ffprobe.clone(),
        services.run.probes().clone(),
    ));
    let mut transaction = match manager.plan(
        job.spec.run_id,
        &job.spec.input,
        &destination.final_path,
        destination.replacement,
        job.spec.execution.overwrite_existing,
    ) {
        Ok(transaction) => transaction,
        Err(error) if error.is_destination_exists() => {
            terminal(
                commands,
                job,
                tracker,
                ItemOutcome::Skipped {
                    reason: SkipReason::OutputExists,
                },
                None,
            )?;
            return Ok(None);
        }
        Err(error) => {
            terminal(
                commands,
                job,
                tracker,
                ItemOutcome::Failed(FailureFacts::new(
                    FailureKind::OutputPrepare,
                    error.to_string(),
                )),
                None,
            )?;
            return Ok(None);
        }
    };
    // The intent must be durable before the staging file exists: a crash
    // after this submit is recovered from the journal (staging absent →
    // abandoned), whereas a file created before the journal record would
    // leak with no record to recover it from.
    submit_output(
        commands,
        OutputDelta::OutputStarted {
            transaction: Box::new(transaction.clone()),
        },
    )?;
    let initial = match manager.create_staging(&transaction) {
        Ok(initial) => initial,
        Err(error) => {
            submit_output(
                commands,
                OutputDelta::Abandoned {
                    run_id: job.spec.run_id,
                },
            )?;
            terminal(
                commands,
                job,
                tracker,
                ItemOutcome::Failed(FailureFacts::new(
                    FailureKind::OutputPrepare,
                    error.to_string(),
                )),
                None,
            )?;
            return Ok(None);
        }
    };
    let created = OutputDelta::StagingCreated {
        run_id: job.spec.run_id,
        initial: initial.clone(),
    };
    if let Err(error) = submit_output(commands, created.clone()) {
        return match manager.remove_staging(&transaction.staging, &initial) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; unjournaled staging cleanup failed: {cleanup}"
            )),
        };
    }
    fold_transaction(&mut transaction, created);
    Ok(Some(PreparedOutput {
        manager,
        transaction,
    }))
}

pub(super) fn finish_successful_output(
    commands: &CommandSender,
    job: &ClaimedJob,
    manager: MediaOutputManager,
    mut transaction: OutputTransaction,
    success: SuccessfulJob,
    final_progress: Option<JobProgress>,
    tracker: &mut PhaseTracker,
) -> Result<(), String> {
    publish_phase(commands, job.spec.run_id, tracker, JobPhase::Verifying);
    let ready = match manager.mark_ready(&transaction) {
        Ok(ready) => ready,
        Err(error) => {
            let settlement = settle_abandoned(&manager, commands, &transaction)?;
            let final_telemetry = map_progress(
                job.spec.run_id,
                tracker,
                JobPhase::Verifying,
                final_progress,
            );
            // A force-stopped verification is a stop, not a broken output.
            let outcome = if error.is_cancelled() {
                ItemOutcome::Stopped
            } else {
                ItemOutcome::Failed(output_failure(
                    FailureKind::OutputPromote,
                    &error,
                    job,
                    &transaction,
                ))
            };
            terminal(
                commands,
                job,
                tracker,
                settlement.outcome(outcome),
                final_telemetry,
            )?;
            return Ok(());
        }
    };
    submit_output(commands, ready.clone())?;
    fold_transaction(&mut transaction, ready);
    publish_phase(commands, job.spec.run_id, tracker, JobPhase::Finalizing);
    while !transaction.is_settled() {
        let next = match manager.recover_once(&transaction) {
            Ok(Some(next)) => next,
            Ok(None) => {
                return Err("output recovery made no progress before settlement".to_owned());
            }
            Err(error) => {
                let final_telemetry = map_progress(
                    job.spec.run_id,
                    tracker,
                    JobPhase::Finalizing,
                    final_progress,
                );
                let outcome = if error.is_cancelled()
                    && matches!(transaction.state, OutputState::Ready { .. })
                {
                    // Ready is journaled before rename. Only a still-present,
                    // identity-matched staging can authorize abandonment.
                    match settle_abandoned(&manager, commands, &transaction)? {
                        AbandonSettlement::Abandoned => ItemOutcome::Stopped,
                        conflict => conflict.outcome(ItemOutcome::Failed(output_failure(
                            FailureKind::OutputConflict,
                            &error,
                            job,
                            &transaction,
                        ))),
                    }
                } else {
                    settle_conflict(commands, job.spec.run_id, error.to_string())?;
                    ItemOutcome::Failed(output_failure(
                        FailureKind::OutputConflict,
                        &error,
                        job,
                        &transaction,
                    ))
                };
                terminal(commands, job, tracker, outcome, final_telemetry)?;
                return Ok(());
            }
        };
        submit_output(commands, next.clone())?;
        fold_transaction(&mut transaction, next);
    }
    // The outcome is built only now, after settlement: remux evidence needs
    // the settled final identity, and a delta-borne conflict settlement must
    // surface as the structured failure it is rather than a claimed success.
    let outcome = settled_outcome(success, &transaction);
    let final_telemetry = map_progress(
        job.spec.run_id,
        tracker,
        JobPhase::Finalizing,
        final_progress,
    );
    terminal(commands, job, tracker, outcome, final_telemetry)
}

/// Failure facts for an output-transaction error. A tool's stderr tail, when
/// the error carries one, travels as a scrubbed diagnostic rather than
/// message prose: it may quote the run's paths.
fn output_failure(
    kind: FailureKind,
    error: &OutputError,
    job: &ClaimedJob,
    transaction: &OutputTransaction,
) -> FailureFacts {
    let facts = FailureFacts::new(kind, error.to_string());
    match error.diagnostic() {
        Some(diagnostic) => facts.with_diagnostic(scrub_tail(
            String::from_utf8_lossy(diagnostic.as_bytes()).trim(),
            &[
                (job.spec.input.as_path(), "<input>"),
                (transaction.staging.as_path(), "<staging>"),
                (transaction.final_path.as_path(), "<output>"),
            ],
        )),
        None => facts,
    }
}

/// Maps a settled transaction plus the adapter's success facts to the
/// terminal outcome. Success requires the replacement-consistent settled
/// state; a Conflict settlement becomes a structured output-conflict failure.
fn settled_outcome(success: SuccessfulJob, transaction: &OutputTransaction) -> ItemOutcome {
    if let Some(final_identity) = transaction.settled_identity() {
        return match success {
            SuccessfulJob::Encode { decode_mode } => {
                ItemOutcome::Converted(CompletionEvidence::LiveEncode {
                    input_size: transaction.input_identity.size,
                    output_size: final_identity.destructive.size,
                    encode_decode: decode_mode,
                })
            }
            // The remux adapter reports only the output path; sizes come from
            // the identities the settlement itself verified.
            SuccessfulJob::Remux => ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
                input_size: transaction.input_identity.size,
                output_size: final_identity.destructive.size,
            }),
        };
    }
    match transaction.state {
        OutputState::Conflict { .. } => ItemOutcome::Failed(FailureFacts::new(
            FailureKind::OutputConflict,
            "output transaction settled as a conflict",
        )),
        _ => ItemOutcome::Stopped,
    }
}

pub(super) fn submit_output(commands: &CommandSender, delta: OutputDelta) -> Result<(), String> {
    require_accepted(
        "record output transition",
        commands.submit(Command::Worker(WorkerCommand::Output(delta))),
    )
}

/// Every caller reaches this after a filesystem inspection or action failed,
/// so the conflict kind is baked in; identity mismatches are detected by the
/// core recovery policy and arrive as deltas, not through this path.
fn settle_conflict(commands: &CommandSender, run_id: RunId, detail: String) -> Result<(), String> {
    tracing::warn!(
        "output transaction {} requires intervention: {detail}",
        run_id.0
    );
    submit_output(
        commands,
        OutputDelta::Conflict {
            run_id,
            kind: ConflictKind::InspectionFailed,
            detail,
        },
    )
}

enum AbandonSettlement {
    Abandoned,
    Conflict(String),
}

impl AbandonSettlement {
    fn outcome(self, outcome: ItemOutcome) -> ItemOutcome {
        match self {
            Self::Abandoned => outcome,
            Self::Conflict(detail) => {
                let mut facts = match outcome {
                    ItemOutcome::Failed(facts) => facts,
                    _ => FailureFacts::new(FailureKind::OutputConflict, "run stopped"),
                };
                facts.kind = FailureKind::OutputConflict;
                facts.message = format!("{}; output cleanup failed: {detail}", facts.message);
                ItemOutcome::Failed(facts)
            }
        }
    }
}

fn abandon_conflict(
    commands: &CommandSender,
    run_id: RunId,
    detail: String,
) -> Result<AbandonSettlement, String> {
    settle_conflict(commands, run_id, detail.clone())?;
    Ok(AbandonSettlement::Conflict(detail))
}

fn settle_abandoned(
    manager: &OutputManager<MediaArtifactInspector>,
    commands: &CommandSender,
    transaction: &crfty_core::OutputTransaction,
) -> Result<AbandonSettlement, String> {
    let intent = match manager.abandon_intent(transaction) {
        Ok(intent) => intent,
        Err(error) => {
            return abandon_conflict(commands, transaction.run_id, error.to_string());
        }
    };
    submit_output(commands, intent.clone())?;
    if matches!(intent, OutputDelta::Abandoned { .. }) {
        return Ok(AbandonSettlement::Abandoned);
    }
    let mut abandoning = transaction.clone();
    fold_transaction(&mut abandoning, intent);
    match manager.recover_once(&abandoning) {
        Ok(Some(delta @ OutputDelta::Abandoned { .. })) => {
            submit_output(commands, delta)?;
            Ok(AbandonSettlement::Abandoned)
        }
        Ok(Some(OutputDelta::Conflict { kind, detail, .. })) => {
            submit_output(
                commands,
                OutputDelta::Conflict {
                    run_id: transaction.run_id,
                    kind,
                    detail: detail.clone(),
                },
            )?;
            tracing::warn!("output abandonment failed: {detail}");
            Ok(AbandonSettlement::Conflict(detail))
        }
        Ok(_) => Err("output abandonment made no progress toward settlement".to_owned()),
        Err(error) => abandon_conflict(commands, transaction.run_id, error.to_string()),
    }
}

pub(super) fn fold_transaction(
    transaction: &mut crfty_core::OutputTransaction,
    delta: OutputDelta,
) {
    let mut state = DurableState::default();
    state
        .outputs
        .insert(transaction.run_id, transaction.clone());
    fold(&mut state, &DurableDelta::Output(delta));
    if let Some(updated) = state.outputs.remove(&transaction.run_id) {
        *transaction = updated;
    }
}

#[cfg(test)]
mod tests {
    use super::AbandonSettlement;
    use crfty_core::{FailureFacts, FailureKind, ItemOutcome};

    #[test]
    fn cleanup_conflict_does_not_report_a_clean_stop_or_discard_the_initial_failure() {
        assert_eq!(
            AbandonSettlement::Abandoned.outcome(ItemOutcome::Stopped),
            ItemOutcome::Stopped
        );
        for outcome in [
            ItemOutcome::Stopped,
            ItemOutcome::Failed(FailureFacts::new(
                FailureKind::OutputPromote,
                "probe rejected",
            )),
        ] {
            let initial_message = match &outcome {
                ItemOutcome::Failed(facts) => facts.message.as_str(),
                _ => "run stopped",
            };
            let expected = initial_message.to_owned();
            let ItemOutcome::Failed(facts) =
                AbandonSettlement::Conflict("staging removal denied".to_owned()).outcome(outcome)
            else {
                panic!("cleanup failure must remain visible");
            };
            assert_eq!(facts.kind, FailureKind::OutputConflict);
            assert!(facts.message.contains(&expected));
            assert!(facts.message.contains("staging removal denied"));
        }
    }
}
