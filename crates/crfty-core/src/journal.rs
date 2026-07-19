use serde::{Deserialize, Serialize};

use crate::{
    AppState, DurableDelta, DurableState, JournalSequence, QueueItemState, fold,
    reducer::{validate_output_delta, validate_terminal},
};

pub const JOURNAL_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEnvelope {
    pub schema_version: u32,
    pub sequence: JournalSequence,
    pub deltas: Vec<DurableDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCorruption {
    pub offset: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalReplay {
    pub state: DurableState,
    pub next_sequence: JournalSequence,
    pub corruption: Option<JournalCorruption>,
    pub ignored_torn_tail: bool,
}

pub fn encode_record(envelope: &JournalEnvelope) -> Result<Vec<u8>, serde_json::Error> {
    let mut encoded = serde_json::to_vec(envelope)?;
    encoded.push(b'\n');
    Ok(encoded)
}

#[must_use]
pub fn replay(bytes: &[u8]) -> JournalReplay {
    let mut state = DurableState::default();
    let mut expected = 0_u64;
    let mut offset = 0_usize;
    let mut corruption = None;
    let mut ignored_torn_tail = false;

    for segment in bytes.split_inclusive(|byte| *byte == b'\n') {
        let complete = segment.last() == Some(&b'\n');
        if !complete {
            ignored_torn_tail = true;
            break;
        }
        let line = segment.strip_suffix(b"\n").unwrap_or(segment);
        if line.is_empty() {
            if complete {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: "empty journal record".to_owned(),
                });
            } else {
                ignored_torn_tail = true;
            }
            break;
        }
        let envelope = match serde_json::from_slice::<JournalEnvelope>(line) {
            Ok(envelope) => envelope,
            Err(error) => {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: format!("invalid journal record: {error}"),
                });
                break;
            }
        };
        if envelope.schema_version != JOURNAL_SCHEMA_VERSION {
            corruption = Some(JournalCorruption {
                offset,
                reason: format!("unsupported journal schema {}", envelope.schema_version),
            });
            break;
        }
        if envelope.sequence != JournalSequence(expected) {
            corruption = Some(JournalCorruption {
                offset,
                reason: format!(
                    "expected journal sequence {expected}, found {}",
                    envelope.sequence.0
                ),
            });
            break;
        }
        if envelope.deltas.is_empty() {
            corruption = Some(JournalCorruption {
                offset,
                reason: "empty journal batch".to_owned(),
            });
            break;
        }
        let mut candidate = state.clone();
        for delta in &envelope.deltas {
            if let Err(reason) = validate_replayed_delta(&candidate, delta) {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: format!("invalid durable transition: {reason}"),
                });
                break;
            }
            fold(&mut candidate, delta);
        }
        if corruption.is_some() {
            break;
        }
        state = candidate;
        expected = match expected.checked_add(1) {
            Some(next) => next,
            None => {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: "journal sequence overflow".to_owned(),
                });
                break;
            }
        };
        offset += segment.len();
    }

    JournalReplay {
        state,
        next_sequence: JournalSequence(expected),
        corruption,
        ignored_torn_tail,
    }
}

fn validate_replayed_delta(state: &DurableState, delta: &DurableDelta) -> Result<(), &'static str> {
    match delta {
        DurableDelta::QueueAdded { item } => {
            if state.queue.iter().any(|current| current.id == item.id) {
                return Err("queue item id already exists");
            }
            if !matches!(item.state, QueueItemState::Queued) {
                return Err("new queue item is not queued");
            }
        }
        DurableDelta::QueueRemoved { item_id } => {
            let removable = state.queue.iter().any(|item| {
                item.id == *item_id
                    && matches!(
                        item.state,
                        QueueItemState::Queued | QueueItemState::Finished(_)
                    )
            });
            if !removable {
                return Err("removed queue item does not exist or is active");
            }
        }
        DurableDelta::QueueMoved { item_id, before } => {
            let movable = state
                .queue
                .iter()
                .any(|item| item.id == *item_id && matches!(item.state, QueueItemState::Queued));
            let destination = before.is_none_or(|before_id| {
                state.queue.iter().any(|item| {
                    item.id == before_id && matches!(item.state, QueueItemState::Queued)
                })
            });
            if !movable || !destination {
                return Err("queue move references an unavailable item");
            }
        }
        DurableDelta::ItemClaimed { spec } => {
            spec.execution.validate()?;
            let matches_item = state.queue.iter().any(|item| {
                item.id == spec.item_id
                    && item.input == spec.input
                    && item.operation == spec.operation
                    && item.output_target == spec.output_target
                    && matches!(item.state, QueueItemState::Queued)
            });
            let active = state.queue.iter().any(|item| {
                matches!(
                    item.state,
                    QueueItemState::Claimed { .. } | QueueItemState::Running { .. }
                )
            });
            if !matches_item || active || state.conversion_runs.contains_key(&spec.run_id) {
                return Err("claim does not match an available queue item");
            }
        }
        DurableDelta::ItemRunning {
            item_id,
            claim_id,
            run_id,
        } => {
            let matches_claim = state.queue.iter().any(|item| {
                item.id == *item_id
                    && matches!(
                        item.state,
                        QueueItemState::Claimed {
                            claim_id: current_claim,
                            run_id: current_run,
                        } if current_claim == *claim_id && current_run == *run_id
                    )
            });
            if !matches_claim {
                return Err("running transition has a stale claim");
            }
        }
        DurableDelta::AnalysisRecorded { run_id, result } => {
            let Some(run) = state.conversion_runs.get(run_id) else {
                return Err("analysis references a missing run");
            };
            if run.analysis.is_some() {
                return Err("analysis is already recorded");
            }
            result.validate_for(&run.spec.execution)?;
        }
        DurableDelta::ItemFinished {
            item_id,
            claim_id,
            run_id,
            outcome,
        } => {
            let Some(run) = state.conversion_runs.get(run_id) else {
                return Err("terminal transition references a missing run");
            };
            let active = state.queue.iter().any(|item| {
                item.id == *item_id
                    && matches!(
                        item.state,
                        QueueItemState::Claimed {
                            claim_id: current_claim,
                            run_id: current_run,
                        } | QueueItemState::Running {
                            claim_id: current_claim,
                            run_id: current_run,
                        } if current_claim == *claim_id && current_run == *run_id
                    )
            });
            if !active || run.outcome.is_some() {
                return Err("terminal transition has a stale claim");
            }
            validate_terminal(run, state.outputs.get(run_id), outcome)?;
        }
        DurableDelta::Output(output) => {
            let app = AppState {
                durable: state.clone(),
                ..AppState::default()
            };
            validate_output_delta(&app, output)?;
        }
    }
    Ok(())
}
