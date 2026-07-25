//! Startup crash recovery: reconciling a journal whose last session died
//! mid-job against what the filesystem actually holds.

use crfty_core::{
    Command, CompletionEvidence, ConflictKind, DurableDelta, DurableState, FailureFacts,
    FailureKind, ItemOutcome, JobAction, OutputDelta, OutputState, QueueItemState, RunId,
    WorkerCommand, fold,
};

use crate::{
    driver::CommandSender,
    output::{MediaArtifactInspector, OutputManager},
    vendor::discovery::MediaTools,
};

use super::accepted;
use crate::clock::now_millis;

pub(super) fn recover_startup(
    commands: &CommandSender,
    tools: Option<&MediaTools>,
    mut state: DurableState,
) -> DurableState {
    let manager =
        tools.map(|tools| OutputManager::new(MediaArtifactInspector::new(tools.ffprobe.clone())));
    let active: Vec<_> = state
        .queue
        .iter()
        .filter_map(|item| match item.state {
            QueueItemState::Reserved { claim_id, run_id }
            | QueueItemState::Claimed { claim_id, run_id }
            | QueueItemState::Running { claim_id, run_id } => Some((item.id, claim_id, run_id)),
            QueueItemState::Queued | QueueItemState::Finished(_) => None,
        })
        .collect();
    for (item_id, claim_id, run_id) in active {
        let reservation_only = !state.conversion_runs.contains_key(&run_id);
        if reservation_only {
            let at = now_millis();
            if accepted(
                commands.submit(Command::Worker(WorkerCommand::AbandonReservation {
                    item_id,
                    claim_id,
                    run_id,
                    at,
                })),
            ) {
                fold(
                    &mut state,
                    &DurableDelta::ItemFinished {
                        item_id,
                        claim_id,
                        run_id,
                        outcome: ItemOutcome::Stopped,
                        at,
                        phase_spans: Vec::new(),
                    },
                );
            }
            continue;
        }
        while let Some(transaction) = state.outputs.get(&run_id).cloned() {
            if transaction.is_settled() {
                break;
            }
            // Without ffprobe an unsettled transaction cannot be inspected.
            // Settling it blind could retire the ledger path to a possibly
            // complete staging artifact, so leave the item active and the
            // transaction untouched; the next startup with tools completes
            // this recovery identically.
            let Some(manager) = manager.as_ref() else {
                break;
            };
            let delta = match manager.recover_once(&transaction) {
                Ok(Some(delta)) => delta,
                Ok(None) => break,
                Err(error) => OutputDelta::Conflict {
                    run_id,
                    kind: ConflictKind::InspectionFailed,
                    detail: format!("startup output recovery failed: {error}"),
                },
            };
            if !accepted(commands.submit(Command::Worker(WorkerCommand::Output(delta.clone())))) {
                break;
            }
            fold(&mut state, &DurableDelta::Output(delta));
        }
        let output_settled = state
            .outputs
            .get(&run_id)
            .is_none_or(crfty_core::OutputTransaction::is_settled);
        if output_settled {
            let outcome = recovered_outcome(&state, run_id);
            // Honest timestamp: this is when the outcome was decided, which
            // for a crash-recovered run is recovery time, not encode time.
            let at = now_millis();
            if accepted(commands.submit(Command::Worker(WorkerCommand::Terminal {
                item_id,
                claim_id,
                run_id,
                outcome: outcome.clone(),
                at,
                phase_spans: Vec::new(),
                final_telemetry: None,
            }))) {
                fold(
                    &mut state,
                    &DurableDelta::ItemFinished {
                        item_id,
                        claim_id,
                        run_id,
                        outcome,
                        at,
                        phase_spans: Vec::new(),
                    },
                );
            }
        }
    }
    state
}

/// Derives the terminal outcome for a recovered run from its settled output
/// transaction: a promoted-and-settled output is a success even though the
/// process died before acknowledging it, distinguished as Converted or
/// Remuxed by the prepared action; a conflicted settlement is a structured
/// failure; everything else (abandoned staging, no output) stopped cleanly.
fn recovered_outcome(state: &DurableState, run_id: RunId) -> ItemOutcome {
    let Some(transaction) = state.outputs.get(&run_id) else {
        return ItemOutcome::Stopped;
    };
    if transaction.settled_identity().is_some() {
        return match state
            .conversion_runs
            .get(&run_id)
            .map(|run| &run.spec.action)
        {
            Some(JobAction::Remux) => ItemOutcome::Remuxed(CompletionEvidence::RecoveredAtStartup),
            Some(JobAction::Encode { .. }) => {
                ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup)
            }
            // Unreachable: the ledger only accepts output transactions
            // for encode and remux runs.
            _ => ItemOutcome::Stopped,
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
