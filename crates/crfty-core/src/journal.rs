use serde::{Deserialize, Serialize};

use crate::{
    AppState, DurableDelta, DurableState, JournalSequence, QueueItemState, fold,
    reducer::{validate_output_delta, validate_terminal},
};

pub const JOURNAL_SCHEMA_VERSION: u32 = 11;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
        DurableDelta::ItemReserved { job } => {
            let matches_item = state.queue.iter().any(|item| {
                item.id == job.item_id
                    && item.input == job.input
                    && item.operation == job.operation
                    && item.intent == job.intent
                    && item.output_target == job.output_target
                    && matches!(item.state, QueueItemState::Queued)
            });
            let active = state.queue.iter().any(|item| {
                matches!(
                    item.state,
                    QueueItemState::Reserved { .. }
                        | QueueItemState::Claimed { .. }
                        | QueueItemState::Running { .. }
                )
            });
            if !matches_item || active {
                return Err("reservation does not match an available queue item");
            }
        }
        DurableDelta::MediaObserved { observation } => {
            if observation.path_hash.0.is_empty()
                || observation.binding.content_key.0.is_empty()
                || observation.binding.stamp.size == 0
                || observation.metadata.duration_ms == 0
                || observation.metadata.width == 0
                || observation.metadata.height == 0
            {
                return Err("media observation is incomplete");
            }
        }
        DurableDelta::ItemPrepared { spec } => {
            spec.execution.validate()?;
            let matches_item = state.queue.iter().any(|item| {
                item.id == spec.item_id
                    && item.input == spec.input
                    && item.operation == spec.operation
                    && item.intent == spec.intent
                    && item.output_target == spec.output_target
                    && matches!(
                        item.state,
                        QueueItemState::Reserved {
                            claim_id,
                            run_id,
                        } if claim_id == spec.claim_id && run_id == spec.run_id
                    )
            });
            let record = spec
                .content_key
                .as_ref()
                .and_then(|key| state.records.get(key));
            let content_exists = spec.content_key.is_none() || record.is_some();
            let expected_action = crate::select_job_action(
                record.map(|known| &known.metadata),
                record,
                spec.operation,
                spec.intent,
                &spec.execution,
            );
            if !matches_item
                || !content_exists
                || spec.action != expected_action
                || state.conversion_runs.contains_key(&spec.run_id)
            {
                return Err("prepared job does not match its reservation or media record");
            }
        }
        DurableDelta::ItemRunning {
            item_id,
            claim_id,
            run_id,
            ..
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
            if run
                .spec
                .content_key
                .as_ref()
                .is_some_and(|key| !state.records.contains_key(key))
            {
                return Err("analysis content record is missing");
            }
            result.validate_for(&run.spec.execution)?;
        }
        DurableDelta::ItemFinished {
            item_id,
            claim_id,
            run_id,
            outcome,
            ..
        } => {
            let reserved = state.queue.iter().any(|item| {
                item.id == *item_id
                    && matches!(
                        item.state,
                        QueueItemState::Reserved {
                            claim_id: current_claim,
                            run_id: current_run,
                        } if current_claim == *claim_id && current_run == *run_id
                    )
            });
            if reserved {
                if !matches!(outcome, crate::ItemOutcome::Stopped)
                    || state.conversion_runs.contains_key(run_id)
                {
                    return Err("reservation terminal is not a clean stop");
                }
                return Ok(());
            }
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
