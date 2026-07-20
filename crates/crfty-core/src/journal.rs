use serde::{Deserialize, Serialize};

use crate::{
    AppState, DurableDelta, DurableState, JournalSequence, QueueItemState, SessionState,
    UnixMillis, fold, output::validate_output_delta, reducer::validate_terminal,
};

pub const JOURNAL_SCHEMA_VERSION: u32 = 12;

/// Compaction fires at an idle writer barrier when the journal is both large
/// in absolute terms and dominated by dead upserts (#33 §10). The floor keeps
/// healthy small journals untouched; the ratio mirrors Redis AOF's
/// grown-relative-to-live-state rewrite trigger.
pub const COMPACTION_IDLE_MIN_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
pub const COMPACTION_IDLE_MIN_RATIO: u64 = 4;
/// Hard ceiling: bounds startup replay cost even when the live state itself is
/// large enough that the ratio rule never trips.
pub const COMPACTION_HARD_LIMIT_BYTES: u64 = 256 * 1024 * 1024;

#[must_use]
pub fn compaction_due(journal_bytes: u64, live_state_bytes: u64) -> bool {
    if journal_bytes >= COMPACTION_HARD_LIMIT_BYTES {
        return true;
    }
    journal_bytes >= COMPACTION_IDLE_MIN_JOURNAL_BYTES
        && journal_bytes >= live_state_bytes.saturating_mul(COMPACTION_IDLE_MIN_RATIO)
}

/// Compaction is a writer barrier, never an interruption: it runs only while
/// no session is active and no queue item holds a reservation or claim
/// (#33 §10 — "a conversion already running is never interrupted").
#[must_use]
pub fn compaction_quiescent(state: &AppState) -> bool {
    state.session == SessionState::Idle
        && state.durable.queue.iter().all(|item| {
            matches!(
                item.state,
                QueueItemState::Queued | QueueItemState::Finished(_)
            )
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEnvelope {
    pub sequence: JournalSequence,
    pub deltas: Vec<DurableDelta>,
}

/// The folded state a compaction wrote as the new journal's head line,
/// stamped so the surviving file records which schema and application
/// produced it (#33 §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalSnapshot {
    pub app_version: String,
    pub compacted_at: UnixMillis,
    /// The sequence the first delta record after this snapshot must carry;
    /// numbering continues across compactions so recovery identity and
    /// runtime-id derivation never reset.
    pub base_sequence: JournalSequence,
    pub state: DurableState,
}

/// One journal line, decoded after [`JournalLineVersion`] has already
/// verified the schema version at line level.
#[derive(Debug, Deserialize)]
struct JournalLine {
    record: JournalRecord,
}

/// First-stage decode: only the version, ignoring the record body. A line
/// from a different schema version has an unknown record shape, so the
/// version must be readable without decoding the record — that is what turns
/// a future format change into "unsupported journal schema" rather than a
/// parse error.
#[derive(Debug, Deserialize)]
struct JournalLineVersion {
    schema_version: u32,
}

#[derive(Debug, Deserialize)]
enum JournalRecord {
    Snapshot(Box<JournalSnapshot>),
    Deltas(JournalEnvelope),
}

/// Borrowing mirror of [`JournalLine`]/[`JournalRecord`] so encoding never
/// clones the folded state. Variant names must match the owned decoder.
#[derive(Serialize)]
struct JournalLineRef<'a> {
    schema_version: u32,
    record: JournalRecordRef<'a>,
}

#[derive(Serialize)]
enum JournalRecordRef<'a> {
    Snapshot(JournalSnapshotRef<'a>),
    Deltas(&'a JournalEnvelope),
}

/// Field-borrowing mirror of [`JournalSnapshot`] so compaction serializes the
/// live folded state without cloning it.
#[derive(Serialize)]
struct JournalSnapshotRef<'a> {
    app_version: &'a str,
    compacted_at: UnixMillis,
    base_sequence: JournalSequence,
    state: &'a DurableState,
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
    /// Byte length of the journal prefix that folded into `state`. On a torn
    /// tail this is where the writer must truncate before appending again;
    /// otherwise the partial record and the next append would merge into one
    /// unparseable line and load as corruption on the following start.
    pub valid_prefix_len: usize,
}

pub fn encode_record(envelope: &JournalEnvelope) -> Result<Vec<u8>, serde_json::Error> {
    encode_line(&JournalLineRef {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record: JournalRecordRef::Deltas(envelope),
    })
}

/// Encode the head line of a compacted journal. Sequence numbering continues:
/// the first delta record appended after this snapshot must carry
/// `base_sequence`.
pub fn encode_snapshot(
    app_version: &str,
    compacted_at: UnixMillis,
    base_sequence: JournalSequence,
    state: &DurableState,
) -> Result<Vec<u8>, serde_json::Error> {
    encode_line(&JournalLineRef {
        schema_version: JOURNAL_SCHEMA_VERSION,
        record: JournalRecordRef::Snapshot(JournalSnapshotRef {
            app_version,
            compacted_at,
            base_sequence,
            state,
        }),
    })
}

fn encode_line(line: &JournalLineRef<'_>) -> Result<Vec<u8>, serde_json::Error> {
    let mut encoded = serde_json::to_vec(line)?;
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
            corruption = Some(JournalCorruption {
                offset,
                reason: "empty journal record".to_owned(),
            });
            break;
        }
        match serde_json::from_slice::<JournalLineVersion>(line) {
            Ok(probe) if probe.schema_version != JOURNAL_SCHEMA_VERSION => {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: format!("unsupported journal schema {}", probe.schema_version),
                });
                break;
            }
            Ok(_current) => {}
            Err(error) => {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: format!("invalid journal record: {error}"),
                });
                break;
            }
        }
        let parsed = match serde_json::from_slice::<JournalLine>(line) {
            Ok(parsed) => parsed,
            Err(error) => {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: format!("invalid journal record: {error}"),
                });
                break;
            }
        };
        match parsed.record {
            JournalRecord::Snapshot(snapshot) => {
                // A snapshot is only ever written as the head of a freshly
                // compacted journal; one appearing later means the file was
                // spliced or overwritten mid-stream.
                if offset != 0 {
                    corruption = Some(JournalCorruption {
                        offset,
                        reason: "snapshot record after journal head".to_owned(),
                    });
                    break;
                }
                state = snapshot.state;
                expected = snapshot.base_sequence.0;
            }
            JournalRecord::Deltas(envelope) => {
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
            }
        }
        offset += segment.len();
    }

    JournalReplay {
        state,
        next_sequence: JournalSequence(expected),
        corruption,
        ignored_torn_tail,
        valid_prefix_len: offset,
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
            validate_output_delta(state, output)?;
        }
    }
    Ok(())
}
