use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    AnalysisAttempt, AnalysisIntent, AnalysisResult, ContentKey, DecodeMode, DurationMs,
    FailureFacts, FileRecord, ImportPath, JobPhase, JobSpec, MediaObservation, Operation,
    OutputDelta, OutputTarget, ParkedRecord, PathBinding, PathHash, ReservedJob, Settings,
    SkipReason, ToolRevisions, UnixMillis, Verdict, VerdictKind,
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
    pub intent: AnalysisIntent,
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
    Converted(CompletionEvidence),
    Remuxed(CompletionEvidence),
    NotWorthwhile { attempts: Vec<AnalysisAttempt> },
    Stopped,
    Skipped { reason: SkipReason },
    Failed(FailureFacts),
}

/// Where the facts backing a successful outcome came from. A live run carries
/// what the adapter measured; a crash-recovered success carries nothing —
/// output size, path, and content key are already durable on the settled
/// transaction, and fabricating adapter fields would be dishonest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum CompletionEvidence {
    LiveEncode {
        #[specta(type = crate::JsNumber)]
        input_size: u64,
        #[specta(type = crate::JsNumber)]
        output_size: u64,
        stream_sizes: StreamByteSizes,
        /// The decode mode the encode actually ran with; diverges from the
        /// analysis profile once the hardware→software retry ladder exists.
        encode_decode: DecodeMode,
    },
    LiveRemux {
        #[specta(type = crate::JsNumber)]
        input_size: u64,
        #[specta(type = crate::JsNumber)]
        output_size: u64,
    },
    RecoveredAtStartup,
}

/// Core-owned mirror of the adapter's per-stream output byte accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct StreamByteSizes {
    #[specta(type = crate::JsNumber)]
    pub video: u64,
    #[specta(type = crate::JsNumber)]
    pub audio: u64,
    #[specta(type = crate::JsNumber)]
    pub subtitle: u64,
    #[specta(type = crate::JsNumber)]
    pub other: u64,
}

/// How long one job phase ran, measured monotonically by the worker and
/// delivered in the lossless terminal command — telemetry is never a durable
/// source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PhaseSpan {
    pub phase: JobPhase,
    pub duration: DurationMs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ConversionRun {
    pub spec: JobSpec,
    pub analysis: Option<AnalysisResult>,
    pub output_content_key: Option<ContentKey>,
    /// Mirrors the owning queue item's `QueueItemState::Finished(outcome)`;
    /// both are written by the single `DurableDelta::ItemFinished` fold arm.
    /// The run keeps its own copy because it outlives the queue item (items
    /// can be removed once finished) and because journal replay uses
    /// `outcome.is_some()` as its "run already terminal" guard. Projections
    /// must not let the two diverge.
    pub outcome: Option<ItemOutcome>,
    pub started_at: Option<UnixMillis>,
    pub finished_at: Option<UnixMillis>,
    pub phase_spans: Vec<PhaseSpan>,
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
        at: UnixMillis,
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
        at: UnixMillis,
        phase_spans: Vec<PhaseSpan>,
    },
    Output(OutputDelta),
    /// An import batch landing in the parked inbox. One delta per accepted
    /// import; the reducer has already dropped keys that are parked or
    /// adopted.
    HistoryImported {
        records: Vec<(ImportPath, ParkedRecord)>,
    },
    /// A parked record matched the observed file: the parked entry leaves
    /// the inbox, the content record gains import provenance, and — when the
    /// resolution decided one — an adopted verdict.
    ParkedAdopted {
        import_path: ImportPath,
        content_key: ContentKey,
        verdict: Option<Verdict>,
    },
    /// A parked record no longer describes the file at its path: stale
    /// content, retired without adoption. Durable so replay converges and the
    /// journal keeps an audit trail of every discarded imported claim.
    ParkedRetired {
        import_path: ImportPath,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DurableState {
    pub queue: Vec<QueueItem>,
    pub paths: BTreeMap<PathHash, PathBinding>,
    pub records: BTreeMap<ContentKey, FileRecord>,
    pub outputs: BTreeMap<RunId, crate::OutputTransaction>,
    pub conversion_runs: BTreeMap<RunId, ConversionRun>,
    /// Imported history records not yet matched to a real file. The inbox
    /// empties itself: prepare-time adoption or retirement removes entries;
    /// nothing else writes here after an import.
    pub parked: BTreeMap<ImportPath, ParkedRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppState {
    pub durable: DurableState,
    pub settings: Settings,
    pub session: SessionState,
    pub telemetry: BTreeMap<RunId, Telemetry>,
    pub tools: ToolsState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub enum MediaTool {
    Ffmpeg,
    Ffprobe,
}

/// Which discovery tier produced the active media tools. Precedence is
/// explicit environment paths, then the managed vendor install, then PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum ToolSource {
    Explicit,
    System,
    Managed,
}

/// Whether external media tools are usable. Ephemeral state: discovery is a
/// filesystem fact reported to the reducer, never journaled. Fail-closed by
/// default so media work stays gated until discovery actually reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum ToolAvailability {
    Available {
        source: ToolSource,
        revisions: ToolRevisions,
    },
    Missing {
        missing: Vec<MediaTool>,
        detail: String,
    },
}

impl Default for ToolAvailability {
    fn default() -> Self {
        Self::Missing {
            missing: vec![MediaTool::Ffmpeg, MediaTool::Ffprobe],
            detail: "media tool discovery has not completed".to_owned(),
        }
    }
}

/// What the vendor subsystem is doing right now. `Downloading` progress is
/// throttled by the engine (core has no clock); a terminal `Failed` stands
/// until the next vendor command replaces it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub enum VendorActivity {
    #[default]
    Idle,
    Checking,
    Downloading {
        #[specta(type = crate::JsNumber)]
        received: u64,
        #[specta(type = Option<crate::JsNumber>)]
        total: Option<u64>,
    },
    Installing,
    Failed {
        detail: String,
    },
}

/// The full ephemeral tool picture: what is usable, what the vendor pipeline
/// is doing, and whether the compiled-in manifest is newer than the managed
/// install. Never journaled; replayed after each snapshot on subscribe.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub struct ToolsState {
    pub availability: ToolAvailability,
    pub activity: VendorActivity,
    pub update_available: bool,
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
                    started_at: None,
                    finished_at: None,
                    phase_spans: Vec::new(),
                },
            );
        }
        DurableDelta::ItemRunning {
            item_id,
            claim_id,
            run_id,
            at,
        } => {
            set_item_state(
                &mut state.queue,
                *item_id,
                QueueItemState::Running {
                    claim_id: *claim_id,
                    run_id: *run_id,
                },
            );
            if let Some(run) = state.conversion_runs.get_mut(run_id) {
                run.started_at = Some(*at);
            }
        }
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
            at,
            phase_spans,
            ..
        } => {
            set_item_state(
                &mut state.queue,
                *item_id,
                QueueItemState::Finished(outcome.clone()),
            );
            if let Some(run) = state.conversion_runs.get_mut(run_id) {
                if matches!(outcome, ItemOutcome::Converted(_) | ItemOutcome::Remuxed(_))
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
                run.finished_at = Some(*at);
                run.phase_spans = phase_spans.clone();
                // Decisive outcomes upsert the record's verdict; the latest
                // run wins because deltas fold in order. A success with no
                // settled output content key (unsettled transaction) sets no
                // verdict — the produced artifact cannot be named. The
                // verdict absorbs the measured summary here, the single
                // writer for both native and adopted verdicts' shape.
                let kind = match outcome {
                    ItemOutcome::Converted(evidence) => {
                        run.output_content_key.clone().map(|output_content_key| {
                            let (input_size, output_size) = evidence_sizes(evidence);
                            VerdictKind::Converted {
                                output_content_key: Some(output_content_key),
                                input_size,
                                output_size,
                                encoding_time: encoding_duration(phase_spans),
                                crf: run.analysis.as_ref().map(|found| found.measurement.crf),
                                vmaf: run.analysis.as_ref().map(|found| found.measurement.score),
                                target: run.analysis.as_ref().map(|found| found.successful_target),
                            }
                        })
                    }
                    ItemOutcome::Remuxed(evidence) => {
                        run.output_content_key.clone().map(|output_content_key| {
                            let (input_size, output_size) = evidence_sizes(evidence);
                            VerdictKind::Remuxed {
                                output_content_key,
                                input_size,
                                output_size,
                            }
                        })
                    }
                    ItemOutcome::NotWorthwhile { .. } => Some(VerdictKind::NotWorthwhile {
                        requested: run.spec.execution.requested_target,
                        floor: run.spec.execution.fallback_floor,
                    }),
                    ItemOutcome::Analyzed
                    | ItemOutcome::Stopped
                    | ItemOutcome::Skipped { .. }
                    | ItemOutcome::Failed(_) => None,
                };
                if let Some(kind) = kind
                    && let Some(content_key) = &run.spec.content_key
                    && let Some(record) = state.records.get_mut(content_key)
                {
                    record.verdict = Some(Verdict {
                        kind,
                        source_run: Some(*run_id),
                        decided_at: *at,
                    });
                }
            }
        }
        DurableDelta::Output(delta) => delta.fold_into(&mut state.outputs),
        DurableDelta::HistoryImported { records } => {
            for (import_path, parked) in records {
                state.parked.insert(import_path.clone(), parked.clone());
            }
        }
        DurableDelta::ParkedAdopted {
            import_path,
            content_key,
            verdict,
        } => {
            state.parked.remove(import_path);
            if let Some(record) = state.records.get_mut(content_key) {
                record.imported = Some(import_path.clone());
                if let Some(verdict) = verdict {
                    record.verdict = Some(verdict.clone());
                }
            }
        }
        DurableDelta::ParkedRetired { import_path } => {
            state.parked.remove(import_path);
        }
    }
}

/// Measured byte sizes from live evidence; a crash-recovered success carries
/// none, and fabricating them would be dishonest.
fn evidence_sizes(evidence: &CompletionEvidence) -> (Option<u64>, Option<u64>) {
    match evidence {
        CompletionEvidence::LiveEncode {
            input_size,
            output_size,
            ..
        }
        | CompletionEvidence::LiveRemux {
            input_size,
            output_size,
        } => (Some(*input_size), Some(*output_size)),
        CompletionEvidence::RecoveredAtStartup => (None, None),
    }
}

/// Total measured encoding time across the run's phase spans; `None` when no
/// encoding phase was measured (recovered runs, empty spans).
fn encoding_duration(spans: &[PhaseSpan]) -> Option<DurationMs> {
    let mut measured = false;
    let mut total: u64 = 0;
    for span in spans {
        if span.phase == JobPhase::Encoding {
            measured = true;
            total = total.saturating_add(span.duration.0);
        }
    }
    measured.then_some(DurationMs(total))
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
