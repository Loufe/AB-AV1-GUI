use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    AnalysisAttempt, AnalysisResult, ContentKey, FileRecord, JobPhase, JobSpec, MediaObservation,
    Operation, OutputDelta, OutputTarget, PathBinding, PathHash, ReservedJob, Settings, SkipReason,
};

macro_rules! numeric_id {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            specta::Type,
        )]
        pub struct $name(#[specta(type = crate::JsNumber)] pub u64);
    };
}

numeric_id!(QueueItemId);
numeric_id!(ClaimId);
numeric_id!(RunId);
numeric_id!(JournalSequence);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct QueueItem {
    pub id: QueueItemId,
    pub input: PathBuf,
    pub operation: Operation,
    pub output_target: OutputTarget,
    pub state: QueueItemState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum QueueItemState {
    Queued,
    Reserved { claim_id: ClaimId, run_id: RunId },
    Claimed { claim_id: ClaimId, run_id: RunId },
    Running { claim_id: ClaimId, run_id: RunId },
    Finished(ItemOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum ItemOutcome {
    Analyzed,
    Converted,
    Remuxed,
    NotWorthwhile { attempts: Vec<AnalysisAttempt> },
    Stopped,
    Skipped { reason: SkipReason },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ConversionRun {
    pub spec: JobSpec,
    pub analysis: Option<AnalysisResult>,
    pub output_content_key: Option<ContentKey>,
    pub outcome: Option<ItemOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum DurableDelta {
    QueueAdded {
        item: QueueItem,
    },
    QueueRemoved {
        item_id: QueueItemId,
    },
    QueueMoved {
        item_id: QueueItemId,
        before: Option<QueueItemId>,
    },
    ItemReserved {
        job: Box<ReservedJob>,
    },
    MediaObserved {
        observation: Box<MediaObservation>,
    },
    ItemPrepared {
        spec: Box<JobSpec>,
    },
    ItemRunning {
        item_id: QueueItemId,
        claim_id: ClaimId,
        run_id: RunId,
    },
    AnalysisRecorded {
        run_id: RunId,
        result: Box<AnalysisResult>,
    },
    ItemFinished {
        item_id: QueueItemId,
        claim_id: ClaimId,
        run_id: RunId,
        outcome: ItemOutcome,
    },
    Output(OutputDelta),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DurableState {
    pub queue: Vec<QueueItem>,
    pub paths: BTreeMap<PathHash, PathBinding>,
    pub records: BTreeMap<ContentKey, FileRecord>,
    pub outputs: BTreeMap<RunId, crate::OutputTransaction>,
    pub conversion_runs: BTreeMap<RunId, ConversionRun>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppState {
    pub durable: DurableState,
    pub settings: Settings,
    pub session: SessionState,
    pub telemetry: BTreeMap<RunId, Telemetry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AppSnapshot {
    pub durable: DurableState,
    pub settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum ConfigDelta {
    SettingsChanged { settings: Settings },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub enum SessionState {
    #[default]
    Idle,
    Running,
    StopAfterCurrent,
    ForceStopping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct Telemetry {
    pub run_id: RunId,
    #[specta(type = crate::JsNumber)]
    pub sequence: u64,
    pub phase: JobPhase,
    pub progress: JobProgress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum JobProgress {
    Phase,
    SearchBasisPoints(u32),
    OutputPositionMs(#[specta(type = crate::JsNumber)] u64),
}

pub fn fold_config(state: &mut Settings, delta: &ConfigDelta) {
    match delta {
        ConfigDelta::SettingsChanged { settings } => *state = settings.clone(),
    }
}

pub fn fold(state: &mut DurableState, delta: &DurableDelta) {
    match delta {
        DurableDelta::QueueAdded { item } => state.queue.push(item.clone()),
        DurableDelta::QueueRemoved { item_id } => {
            state.queue.retain(|item| item.id != *item_id);
        }
        DurableDelta::QueueMoved { item_id, before } => {
            move_item(&mut state.queue, *item_id, *before)
        }
        DurableDelta::ItemReserved { job } => {
            set_item_state(
                &mut state.queue,
                job.item_id,
                QueueItemState::Reserved {
                    claim_id: job.claim_id,
                    run_id: job.run_id,
                },
            );
        }
        DurableDelta::MediaObserved { observation } => {
            state
                .paths
                .insert(observation.path_hash.clone(), observation.binding.clone());
            state
                .records
                .entry(observation.binding.content_key.clone())
                .and_modify(|record| record.metadata = observation.metadata.clone())
                .or_insert_with(|| FileRecord::new(observation.metadata.clone()));
        }
        DurableDelta::ItemPrepared { spec } => {
            set_item_state(
                &mut state.queue,
                spec.item_id,
                QueueItemState::Claimed {
                    claim_id: spec.claim_id,
                    run_id: spec.run_id,
                },
            );
            state.conversion_runs.insert(
                spec.run_id,
                ConversionRun {
                    spec: spec.as_ref().clone(),
                    analysis: spec.action.selected_analysis().cloned(),
                    output_content_key: None,
                    outcome: None,
                },
            );
        }
        DurableDelta::ItemRunning {
            item_id,
            claim_id,
            run_id,
        } => set_item_state(
            &mut state.queue,
            *item_id,
            QueueItemState::Running {
                claim_id: *claim_id,
                run_id: *run_id,
            },
        ),
        DurableDelta::AnalysisRecorded { run_id, result } => {
            if let Some(run) = state.conversion_runs.get_mut(run_id) {
                run.analysis = Some(result.as_ref().clone());
                if let Some(content_key) = &run.spec.content_key
                    && let Some(record) = state.records.get_mut(content_key)
                {
                    record.record_analysis(result.as_ref().clone());
                }
            }
        }
        DurableDelta::ItemFinished {
            item_id,
            run_id,
            outcome,
            ..
        } => {
            set_item_state(
                &mut state.queue,
                *item_id,
                QueueItemState::Finished(outcome.clone()),
            );
            if let Some(run) = state.conversion_runs.get_mut(run_id) {
                if matches!(outcome, ItemOutcome::Converted | ItemOutcome::Remuxed)
                    && let Some(transaction) = state.outputs.get(run_id)
                {
                    run.output_content_key = match &transaction.state {
                        crate::OutputState::Committed { final_identity }
                        | crate::OutputState::Retired { final_identity } => {
                            Some(final_identity.content_key.clone())
                        }
                        _ => None,
                    };
                }
                run.outcome = Some(outcome.clone());
            }
        }
        DurableDelta::Output(delta) => delta.fold_into(&mut state.outputs),
    }
}

fn set_item_state(queue: &mut [QueueItem], item_id: QueueItemId, item_state: QueueItemState) {
    if let Some(item) = queue.iter_mut().find(|item| item.id == item_id) {
        item.state = item_state;
    }
}

fn move_item(queue: &mut Vec<QueueItem>, item_id: QueueItemId, before: Option<QueueItemId>) {
    let Some(source) = queue.iter().position(|item| item.id == item_id) else {
        return;
    };
    let item = queue.remove(source);
    let destination = before
        .and_then(|before_id| queue.iter().position(|entry| entry.id == before_id))
        .unwrap_or(queue.len());
    queue.insert(destination, item);
}
