//! The output transaction: preparing a destination, opening the ledger
//! entry, settling it against the filesystem, and abandoning it on failure.

use std::path::PathBuf;

use crfty_core::{
    ClaimedJob, Command, CompletionEvidence, ConflictKind, DurableDelta, DurableState,
    FailureFacts, FailureKind, ItemOutcome, JobPhase, JobProgress, OutputDelta, OutputState,
    OutputTransaction, Replacement, RunId, SkipReason, StreamByteSizes, Telemetry, WorkerCommand,
    fold,
};

use crate::{
    driver::CommandSender,
    output::{MediaArtifactInspector, OutputManager},
    vendor::discovery::MediaTools,
};

use super::job::SuccessfulJob;
use super::output_path::resolve_output;
use super::require_accepted;
use super::session::{PhaseTracker, map_progress, publish_phase, terminal};

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
    settle_abandoned(manager, commands, transaction)?;
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
    commands: &CommandSender,
    tools: &MediaTools,
    job: &ClaimedJob,
    destination: OutputDestination,
    tracker: &mut PhaseTracker,
) -> Result<Option<PreparedOutput>, String> {
    let manager = OutputManager::new(MediaArtifactInspector::new(tools.ffprobe.clone()));
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
    // leak with no record to recover it from (#47).
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
            settle_abandoned(&manager, commands, &transaction)?;
            let final_telemetry = map_progress(
                job.spec.run_id,
                tracker,
                JobPhase::Verifying,
                final_progress,
            );
            terminal(
                commands,
                job,
                tracker,
                ItemOutcome::Failed(FailureFacts::new(
                    FailureKind::OutputPromote,
                    error.to_string(),
                )),
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
                settle_conflict(commands, job.spec.run_id, error.to_string())?;
                let final_telemetry = map_progress(
                    job.spec.run_id,
                    tracker,
                    JobPhase::Finalizing,
                    final_progress,
                );
                terminal(
                    commands,
                    job,
                    tracker,
                    ItemOutcome::Failed(FailureFacts::new(
                        FailureKind::OutputConflict,
                        error.to_string(),
                    )),
                    final_telemetry,
                )?;
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

/// Maps a settled transaction plus the adapter's success facts to the
/// terminal outcome. Success requires the replacement-consistent settled
/// state; a Conflict settlement becomes a structured output-conflict failure.
fn settled_outcome(success: SuccessfulJob, transaction: &OutputTransaction) -> ItemOutcome {
    if let Some(final_identity) = transaction.settled_identity() {
        return match success {
            SuccessfulJob::Encode {
                outcome,
                decode_mode,
            } => ItemOutcome::Converted(CompletionEvidence::LiveEncode {
                input_size: outcome.input_size,
                output_size: outcome.output_size,
                stream_sizes: StreamByteSizes {
                    video: outcome.stream_sizes.video,
                    audio: outcome.stream_sizes.audio,
                    subtitle: outcome.stream_sizes.subtitle,
                    other: outcome.stream_sizes.other,
                },
                encode_decode: decode_mode,
            }),
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
    submit_output(
        commands,
        OutputDelta::Conflict {
            run_id,
            kind: ConflictKind::InspectionFailed,
            detail,
        },
    )
}

fn settle_abandoned(
    manager: &OutputManager<MediaArtifactInspector>,
    commands: &CommandSender,
    transaction: &crfty_core::OutputTransaction,
) -> Result<(), String> {
    let intent = match manager.abandon_intent(transaction) {
        Ok(intent) => intent,
        Err(error) => {
            return settle_conflict(commands, transaction.run_id, error.to_string());
        }
    };
    submit_output(commands, intent.clone())?;
    let mut abandoning = transaction.clone();
    fold_transaction(&mut abandoning, intent);
    match manager.recover_once(&abandoning) {
        Ok(Some(abandoned)) => {
            submit_output(commands, abandoned)?;
        }
        Ok(None) => {}
        Err(error) => settle_conflict(commands, transaction.run_id, error.to_string())?,
    }
    Ok(())
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
