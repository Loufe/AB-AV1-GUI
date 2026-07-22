use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::history::prepare_import_adoptions;
use crate::state::ConversionRun;
use crate::{
    AnalysisActivity, AnalysisCommand, AnalysisDelta, AnalysisFileScan, AnalysisIntent,
    AnalysisMutationError, AnalysisResult, AnalysisRow, AnalysisRowEntry, AppState,
    BasicScanDisposition, ClaimId, ClaimedJob, CompletionEvidence, ConfigDelta, ContentKey,
    CorruptionSignature, CurrentFileIdentity, DecodeMode, DecodePreference, DurableDelta,
    ExecutionSettings, FreshnessDecision, ImportPath, ImportedHistoryRecord, ItemOutcome,
    JobAction, JobSpec, MediaObservation, Operation, OutputDelta, OutputState, OutputTarget,
    OverwriteDecision, PathHash, PhaseSpan, QueueItem, QueueItemId, QueueItemState, ReservedJob,
    RunId, SessionAggregates, SessionState, Settings, SkipReason, StatisticsPayload, Telemetry,
    ToolAvailability, ToolsState, UnixMillis, VendorActivity, apply_analysis_mutation,
    begin_analysis_generation, decide_freshness, evaluate_enqueue, fold, fold_config,
    select_job_action, statistics,
};

/// Sanity bound for a requester-supplied UTC offset: one day in minutes.
/// Real offsets stay within ±14 hours; anything past a day is a caller bug.
const MAX_UTC_OFFSET_MINUTES: i32 = 1_440;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Analysis(AnalysisCommand),
    Queue(QueueCommand),
    Session(SessionCommand),
    Settings(SettingsCommand),
    Worker(WorkerCommand),
    Vendor(VendorCommand),
    Projection(ProjectionCommand),
    History(HistoryCommand),
    System(SystemCommand),
}

/// Durable history-surface operations. Deliberately NOT a [`SystemCommand`]:
/// system commands emit no durable deltas and stay usable over a corrupt
/// journal, while history operations must be refused by the driver's
/// degraded gate like any other durable write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryCommand {
    /// Park a parsed import batch. Keys already parked or already adopted
    /// are skipped; an all-known batch is still accepted (a re-import is a
    /// counted no-op, not an error).
    Import {
        records: Vec<(ImportPath, ImportedHistoryRecord)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCommand {
    Set { settings: Settings },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueCommand {
    /// A batch of add requests judged individually: ineligible files never
    /// become queue items (ADR-013 filter-at-add), and every disposition is
    /// counted into one [`EphemeralDelta::QueueAddSummary`]. A single add is
    /// a one-element batch. Adds are append-only and allowed in every
    /// session state.
    AddMany { requests: Vec<QueueAddRequest> },
    /// Atomically removes a non-empty, distinct set of pending or finished
    /// items. The whole request rejects if any id is duplicate, unknown, or
    /// active; a one-item removal uses this same surface.
    RemoveMany { item_ids: Vec<QueueItemId> },
    Move {
        item_id: QueueItemId,
        before: Option<QueueItemId>,
    },
    /// Atomically replaces the order of every currently queued item. The
    /// submitted ids must be an exact permutation of the pending tail, so a
    /// stale grouped move is rejected rather than partially applied.
    ReorderPending { pending_order: Vec<QueueItemId> },
    /// Removes every `Queued` and `Finished` item. Idle-only — while a
    /// session runs, the queue's pending tail is the worker's feed. An empty
    /// result is an accepted no-op.
    Clear,
    /// Removes `Finished` items except `Failed(_)` — failures stay visible
    /// until addressed (V2/HandBrake parity). Allowed while running.
    ClearCompleted,
    /// Sends a finished item around again: state resets to `Queued` in place
    /// (no re-add) and the item moves to the end of the queue. A plain retry
    /// is allowed while running; a patched retry is idle-only and applies its
    /// full resolved tuple in the same durable transition as the requeue.
    Retry {
        item_id: QueueItemId,
        /// `None` is a plain retry and stays available during a running
        /// session. `Some`, including an empty patch, is an edit request and
        /// therefore idle-only; the reducer resolves it before journaling.
        patch: Option<QueueItemEdit>,
    },
    /// Rewrites a pending item's job parameters. Valid only on `Queued`
    /// items while the session is idle: a running session's rules are frozen
    /// (#33 §11). Bulk edits are frontend loops over this command.
    Edit {
        item_id: QueueItemId,
        patch: QueueItemEdit,
    },
}

/// A partial edit of a queued item; `None` keeps the current value. The
/// reducer resolves the full tuple before journaling, so the fold and replay
/// never see patch semantics.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct QueueItemEdit {
    pub operation: Option<Operation>,
    pub intent: Option<AnalysisIntent>,
    pub output_target: Option<OutputTarget>,
    pub overwrite: Option<OverwriteDecision>,
}

/// One add request inside [`QueueCommand::AddMany`]. The enqueue facts
/// (`path_hash`, `identity`, and timestamp reliability) are I/O results
/// gathered by the caller — core cannot stat, hash paths, or consult a clock.
/// Either identity fact may be absent (unreadable or vanished path), in which
/// case reliability is `Unknown`; absence fails open so any real problem
/// surfaces at claim time, where content identity is authoritative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueAddRequest {
    pub item_id: QueueItemId,
    pub input: PathBuf,
    pub path_hash: Option<PathHash>,
    pub identity: Option<crate::DestructiveIdentity>,
    pub timestamp_reliability: crate::TimestampReliability,
    pub operation: Operation,
    pub intent: AnalysisIntent,
    pub output_target: OutputTarget,
    pub overwrite: OverwriteDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCommand {
    Start,
    StopAfterCurrent,
    ForceStop,
}

/// User-initiated vendor operations. `Install` downloads and atomically
/// activates the manifest-pinned FFmpeg build; `Check` re-runs discovery and
/// the local update comparison. Both are serialized through
/// [`VendorActivity`]: at most one vendor worker exists at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorCommand {
    Install,
    Check,
}

/// Read-model requests answered synchronously from durable state. The reply
/// is a plain `Accepted`; the computed payload travels as a sequenced
/// ephemeral delta on the one stream (ADR-006), never journaled and never
/// replayed to late subscribers — a stale-aware UI simply re-requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionCommand {
    RequestStatistics { utc_offset_minutes: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerCommand {
    ReserveNext {
        claim_id: ClaimId,
        run_id: RunId,
    },
    PrepareReserved {
        item_id: QueueItemId,
        claim_id: ClaimId,
        run_id: RunId,
        observation: Option<Box<MediaObservation>>,
        /// Normalized path spellings of the observed file (at most two:
        /// canonical and merely-absolute), computed by the engine with the
        /// same rule the import uses. Any that are parked resolve to
        /// adoption or retirement alongside the observation.
        import_paths: Vec<ImportPath>,
        execution: ExecutionSettings,
    },
    AbandonReservation {
        item_id: QueueItemId,
        claim_id: ClaimId,
        run_id: RunId,
        at: UnixMillis,
    },
    Started {
        item_id: QueueItemId,
        claim_id: ClaimId,
        run_id: RunId,
        at: UnixMillis,
    },
    Output(OutputDelta),
    RecordAnalysis {
        item_id: QueueItemId,
        claim_id: ClaimId,
        run_id: RunId,
        result: Box<AnalysisResult>,
    },
    Terminal {
        item_id: QueueItemId,
        claim_id: ClaimId,
        run_id: RunId,
        outcome: ItemOutcome,
        at: UnixMillis,
        phase_spans: Vec<PhaseSpan>,
        final_telemetry: Option<Telemetry>,
    },
    Crashed {
        message: String,
    },
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemCommand {
    Shutdown,
    /// Discovery facts from the engine: what is usable and whether the
    /// compiled-in manifest is newer than the managed install. Never touches
    /// the vendor activity — progress travels via `VendorProgress`.
    ToolsDiscovered {
        availability: ToolAvailability,
        update_available: bool,
    },
    /// Vendor worker progress. The engine throttles emission; core has no
    /// clock and applies whatever it is told.
    VendorProgress {
        activity: VendorActivity,
    },
    /// Operator consent to discard a corrupt journal tail whose identity is
    /// `signature`. The driver intercepts this before `apply` — degraded
    /// state deliberately lives outside `AppState` — so the reducer only ever
    /// sees it on a routing bug and rejects it.
    AcknowledgeCorruption {
        signature: CorruptionSignature,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub enum EphemeralDelta {
    /// Incremental or replacement Analysis read-model state. Standing: the
    /// shell folds it and replays one complete Reset on every subscription.
    Analysis(AnalysisDelta),
    SessionChanged(SessionState),
    /// The per-session aggregates after an item finished (zeroed at session
    /// start). The one post-durable ephemeral: on the stream it follows the
    /// `ItemFinished` it summarizes, so a consumer never sees counts for a
    /// finish it has not observed.
    SessionAggregates(SessionAggregates),
    Telemetry(Telemetry),
    TelemetryCleared {
        run_id: RunId,
    },
    ToolsChanged(ToolsState),
    /// Answer to [`ProjectionCommand::RequestStatistics`]. Fire-and-forget:
    /// not part of the read model and never replayed on subscribe.
    Statistics(Box<StatisticsPayload>),
    WorkerCrashed {
        message: String,
    },
    CommandRejected {
        reason: String,
    },
    /// Disposition counts for one [`QueueCommand::AddMany`] batch. Reasons
    /// carry their payloads, so distinct payloads (different source runs)
    /// count as separate entries — consumers sum across entries.
    QueueAddSummary {
        added: u32,
        skipped: Vec<(SkipReason, u32)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    StartWorker,
    KillActiveRun { run_id: RunId },
    WriteSettings { settings: Settings },
    VendorInstall,
    VendorCheck,
    StopDriver,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Accepted,
    AnalysisStarted {
        generation: crate::AnalysisGenerationId,
    },
    BasicScan(BasicScanDisposition),
    Reserved(Option<Box<ReservedJob>>),
    Claimed(Option<Box<ClaimedJob>>),
    Imported {
        parked: u32,
        skipped: u32,
    },
    Rejected {
        reason: String,
    },
    DurabilityUnknown {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub durable: Vec<DurableDelta>,
    pub config: Vec<ConfigDelta>,
    pub ephemeral: Vec<EphemeralDelta>,
    pub effects: Vec<Effect>,
    pub reply: Reply,
}

impl Applied {
    fn accepted() -> Self {
        Self {
            durable: Vec::new(),
            config: Vec::new(),
            ephemeral: Vec::new(),
            effects: Vec::new(),
            reply: Reply::Accepted,
        }
    }

    fn rejected(reason: impl Into<String>) -> Self {
        let mut applied = Self::accepted();
        applied.reject(reason);
        applied
    }

    /// Every rejection surfaces the same two ways: the caller's `Reply` and a
    /// `CommandRejected` ephemeral on the stream. Keep them in lockstep here.
    fn reject(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.ephemeral.push(EphemeralDelta::CommandRejected {
            reason: reason.clone(),
        });
        self.reply = Reply::Rejected { reason };
    }
}

pub fn apply(state: &mut AppState, command: Command) -> Applied {
    let applied = match command {
        Command::Analysis(command) => apply_analysis_command(state, command),
        Command::Queue(command) => apply_queue(state, command),
        Command::Session(command) => apply_session(state, command),
        Command::Settings(command) => apply_settings(state, command),
        Command::Worker(command) => apply_worker(state, command),
        Command::Vendor(command) => apply_vendor(state, command),
        Command::Projection(ProjectionCommand::RequestStatistics { utc_offset_minutes }) => {
            if utc_offset_minutes.abs() > MAX_UTC_OFFSET_MINUTES {
                return Applied::rejected("UTC offset is outside a plausible range");
            }
            let mut applied = Applied::accepted();
            applied
                .ephemeral
                .push(EphemeralDelta::Statistics(Box::new(statistics(
                    &state.durable,
                    utc_offset_minutes,
                ))));
            applied
        }
        Command::History(command) => apply_history(state, command),
        Command::System(SystemCommand::Shutdown) => {
            let mut applied = Applied::accepted();
            applied.effects.push(Effect::StopDriver);
            applied
        }
        Command::System(SystemCommand::ToolsDiscovered {
            availability,
            update_available,
        }) => tools_transition(
            state,
            ToolsState {
                availability,
                activity: state.tools.activity.clone(),
                update_available,
            },
        ),
        Command::System(SystemCommand::VendorProgress { activity }) => tools_transition(
            state,
            ToolsState {
                activity,
                ..state.tools.clone()
            },
        ),
        Command::System(SystemCommand::AcknowledgeCorruption { .. }) => {
            Applied::rejected("corruption acknowledgement is handled by the driver")
        }
    };
    for delta in &applied.durable {
        fold(&mut state.durable, delta);
    }
    for delta in &applied.config {
        fold_config(&mut state.settings, delta);
    }
    for delta in &applied.ephemeral {
        match delta {
            EphemeralDelta::Analysis(delta) => {
                let result = apply_analysis_mutation(&mut state.analysis, delta);
                debug_assert!(result.is_ok());
            }
            EphemeralDelta::SessionChanged(session) => state.session = session.clone(),
            EphemeralDelta::SessionAggregates(aggregates) => state.aggregates = *aggregates,
            EphemeralDelta::Telemetry(telemetry) => {
                state.telemetry.insert(telemetry.run_id, telemetry.clone());
            }
            EphemeralDelta::TelemetryCleared { run_id } => {
                state.telemetry.remove(run_id);
            }
            EphemeralDelta::ToolsChanged(tools) => state.tools = tools.clone(),
            EphemeralDelta::Statistics(_)
            | EphemeralDelta::WorkerCrashed { .. }
            | EphemeralDelta::CommandRejected { .. }
            | EphemeralDelta::QueueAddSummary { .. } => {}
        }
    }
    applied
}

fn apply_analysis_command(state: &AppState, command: AnalysisCommand) -> Applied {
    match command {
        AnalysisCommand::InspectFile {
            generation,
            row_id,
            path_hash,
            current,
            timestamp_reliability,
            import_paths,
        } => inspect_analysis_file(
            state,
            generation,
            row_id,
            path_hash,
            current,
            timestamp_reliability,
            &import_paths,
        ),
        AnalysisCommand::ObserveFile {
            generation,
            row_id,
            observation,
            import_paths,
        } => observe_analysis_file(state, generation, row_id, *observation, &import_paths),
        other => apply_analysis_state_command(state, other),
    }
}

fn apply_analysis_state_command(state: &AppState, command: AnalysisCommand) -> Applied {
    let mut applied = Applied::accepted();
    let delta = match command {
        AnalysisCommand::Begin { roots } => {
            let delta = match begin_analysis_generation(&state.analysis, roots) {
                Ok(delta) => delta,
                Err(AnalysisMutationError::EmptyRoots) => {
                    return Applied::rejected("Analysis discovery requires at least one root");
                }
                Err(_) => return Applied::rejected("Analysis generation id space is exhausted"),
            };
            let generation = match &delta {
                AnalysisDelta::Reset { snapshot } => match &snapshot.current {
                    Some(generation) => generation.id,
                    None => return Applied::rejected("Analysis generation Reset is empty"),
                },
                AnalysisDelta::RowsUpserted { .. } | AnalysisDelta::ActivityChanged { .. } => {
                    return Applied::rejected(
                        "Analysis generation allocator returned a live delta",
                    );
                }
            };
            applied.reply = Reply::AnalysisStarted { generation };
            delta
        }
        AnalysisCommand::UpsertRows { generation, rows } => {
            AnalysisDelta::RowsUpserted { generation, rows }
        }
        AnalysisCommand::SetActivity {
            generation,
            activity,
        } => AnalysisDelta::ActivityChanged {
            generation,
            activity,
        },
        AnalysisCommand::BeginBasicScan { generation } => {
            if let ToolAvailability::Missing { detail, .. } = &state.tools.availability {
                return Applied::rejected(format!("media tools are unavailable: {detail}"));
            }
            if vendor_worker_active(&state.tools.activity) {
                return Applied::rejected("a vendor operation is in progress");
            }
            AnalysisDelta::ActivityChanged {
                generation,
                activity: AnalysisActivity::BasicScanning,
            }
        }
        AnalysisCommand::FinishBasicScan { generation } => AnalysisDelta::ActivityChanged {
            generation,
            activity: AnalysisActivity::Ready,
        },
        AnalysisCommand::FailFile {
            generation,
            row_id,
            failure,
        } => {
            let row = match analysis_file_with_failure(state, generation, row_id, failure) {
                Ok(row) => row,
                Err(reason) => return Applied::rejected(reason),
            };
            AnalysisDelta::RowsUpserted {
                generation,
                rows: vec![row],
            }
        }
        AnalysisCommand::InspectFile { .. } | AnalysisCommand::ObserveFile { .. } => {
            return Applied::rejected("Analysis scan command reached the state-only path");
        }
    };
    let mut candidate = state.analysis.clone();
    if let Err(error) = apply_analysis_mutation(&mut candidate, &delta) {
        return Applied::rejected(format!("Analysis mutation rejected: {error:?}"));
    }
    applied.ephemeral.push(EphemeralDelta::Analysis(delta));
    applied
}

fn inspect_analysis_file(
    state: &AppState,
    generation: crate::AnalysisGenerationId,
    row_id: crate::AnalysisRowId,
    path_hash: PathHash,
    current: CurrentFileIdentity,
    timestamp_reliability: crate::TimestampReliability,
    import_paths: &[ImportPath],
) -> Applied {
    if let Err(reason) = validate_analysis_file(state, generation, row_id) {
        return Applied::rejected(reason);
    }
    let settled = match &current {
        CurrentFileIdentity::Present(identity) => {
            settled_output_match(state, |artifact| artifact.destructive == *identity)
        }
        CurrentFileIdentity::Missing | CurrentFileIdentity::Unavailable => None,
    };
    let binding = state.durable.paths.get(&path_hash);
    let decision = decide_freshness(
        &current,
        binding.map(|known| &known.identity),
        settled
            .as_ref()
            .map(|(_, _, artifact)| &artifact.destructive),
        timestamp_reliability,
    );
    match decision {
        FreshnessDecision::Missing => basic_scan_reply(BasicScanDisposition::Missing),
        FreshnessDecision::Unavailable => basic_scan_reply(BasicScanDisposition::Unavailable),
        FreshnessDecision::Reobserve(_) => basic_scan_reply(BasicScanDisposition::Observe),
        FreshnessDecision::RecognizeSettledOutput => {
            // Imported stamps need real output metadata before they can be
            // adopted honestly. Do not manufacture it from the source row.
            if import_paths
                .iter()
                .any(|path| state.durable.parked.contains_key(path))
            {
                return basic_scan_reply(BasicScanDisposition::Observe);
            }
            let Some((source_content_key, output_content_key, _)) = settled else {
                return Applied::rejected("settled output recognition lost its matching output");
            };
            let metadata = state
                .durable
                .records
                .get(&output_content_key)
                .map(|record| record.metadata.clone());
            upsert_scanned_row(
                state,
                generation,
                row_id,
                AnalysisFileScan::SettledOutput {
                    source_content_key,
                    output_content_key,
                    metadata,
                    refresh_failure: None,
                },
                Vec::new(),
            )
        }
        FreshnessDecision::ReuseObservation => {
            let Some(binding) = binding else {
                return Applied::rejected("fresh Analysis binding is missing");
            };
            let Some(record) = state.durable.records.get(&binding.content_key) else {
                return basic_scan_reply(BasicScanDisposition::Observe);
            };
            let observation = MediaObservation {
                path_hash,
                binding: binding.clone(),
                metadata: record.metadata.clone(),
            };
            let prepared = prepare_import_adoptions(
                &state.durable,
                Some(&observation),
                import_paths,
                Some(&binding.content_key),
                Some(record),
            );
            upsert_scanned_row(
                state,
                generation,
                row_id,
                AnalysisFileScan::Scanned {
                    content_key: binding.content_key.clone(),
                    metadata: record.metadata.clone(),
                    refresh_failure: None,
                },
                prepared.deltas,
            )
        }
    }
}

fn observe_analysis_file(
    state: &AppState,
    generation: crate::AnalysisGenerationId,
    row_id: crate::AnalysisRowId,
    observation: MediaObservation,
    import_paths: &[ImportPath],
) -> Applied {
    if let Err(reason) = validate_analysis_file(state, generation, row_id) {
        return Applied::rejected(reason);
    }
    let settled = settled_output_match(state, |artifact| {
        artifact.content_key == observation.binding.content_key
            && artifact.destructive == observation.binding.identity
    });
    let adoption_content_key = settled
        .as_ref()
        .map_or(&observation.binding.content_key, |(source, _, _)| source);
    let record = state.durable.records.get(adoption_content_key);
    let prepared = prepare_import_adoptions(
        &state.durable,
        Some(&observation),
        import_paths,
        Some(adoption_content_key),
        record,
    );
    let path_changed = state
        .durable
        .paths
        .get(&observation.path_hash)
        .is_none_or(|binding| binding != &observation.binding);
    let record_changed = state
        .durable
        .records
        .get(&observation.binding.content_key)
        .is_none_or(|record| record.metadata != observation.metadata);
    let mut durable = Vec::new();
    if path_changed || record_changed {
        durable.push(DurableDelta::MediaObserved {
            observation: Box::new(observation.clone()),
        });
    }
    durable.extend(prepared.deltas);
    let scan = settled.map_or_else(
        || AnalysisFileScan::Scanned {
            content_key: observation.binding.content_key.clone(),
            metadata: observation.metadata.clone(),
            refresh_failure: None,
        },
        |(source_content_key, output_content_key, _)| AnalysisFileScan::SettledOutput {
            source_content_key,
            output_content_key,
            metadata: Some(observation.metadata.clone()),
            refresh_failure: None,
        },
    );
    upsert_scanned_row(state, generation, row_id, scan, durable)
}

fn basic_scan_reply(disposition: BasicScanDisposition) -> Applied {
    let mut applied = Applied::accepted();
    applied.reply = Reply::BasicScan(disposition);
    applied
}

fn upsert_scanned_row(
    state: &AppState,
    generation: crate::AnalysisGenerationId,
    row_id: crate::AnalysisRowId,
    scan: AnalysisFileScan,
    durable: Vec<DurableDelta>,
) -> Applied {
    let row = match analysis_file_with_scan(state, generation, row_id, scan) {
        Ok(row) => row,
        Err(reason) => return Applied::rejected(reason),
    };
    let delta = AnalysisDelta::RowsUpserted {
        generation,
        rows: vec![row],
    };
    let mut candidate = state.analysis.clone();
    if let Err(error) = apply_analysis_mutation(&mut candidate, &delta) {
        return Applied::rejected(format!("Analysis mutation rejected: {error:?}"));
    }
    let mut applied = basic_scan_reply(BasicScanDisposition::Complete);
    applied.durable = durable;
    applied.ephemeral.push(EphemeralDelta::Analysis(delta));
    applied
}

fn validate_analysis_file(
    state: &AppState,
    generation: crate::AnalysisGenerationId,
    row_id: crate::AnalysisRowId,
) -> Result<(), &'static str> {
    let current = state
        .analysis
        .current
        .as_ref()
        .ok_or("no Analysis generation exists")?;
    if current.id != generation {
        return Err("Basic Scan names a stale Analysis generation");
    }
    if current.activity != AnalysisActivity::BasicScanning {
        return Err("Analysis generation is not Basic Scanning");
    }
    let row = current
        .rows
        .get(&row_id)
        .ok_or("Basic Scan row does not exist")?;
    if !matches!(row.entry, AnalysisRowEntry::File { .. }) {
        return Err("Basic Scan row is not a file");
    }
    Ok(())
}

fn analysis_file_with_scan(
    state: &AppState,
    generation: crate::AnalysisGenerationId,
    row_id: crate::AnalysisRowId,
    scan: AnalysisFileScan,
) -> Result<AnalysisRow, &'static str> {
    validate_analysis_file(state, generation, row_id)?;
    let current = state
        .analysis
        .current
        .as_ref()
        .ok_or("no Analysis generation exists")?;
    let mut row = current
        .rows
        .get(&row_id)
        .cloned()
        .ok_or("Basic Scan row does not exist")?;
    row.entry = AnalysisRowEntry::File { scan };
    Ok(row)
}

fn analysis_file_with_failure(
    state: &AppState,
    generation: crate::AnalysisGenerationId,
    row_id: crate::AnalysisRowId,
    failure: crate::AnalysisScanFailure,
) -> Result<AnalysisRow, &'static str> {
    validate_analysis_file(state, generation, row_id)?;
    let current = state
        .analysis
        .current
        .as_ref()
        .ok_or("no Analysis generation exists")?;
    let mut row = current
        .rows
        .get(&row_id)
        .cloned()
        .ok_or("Basic Scan row does not exist")?;
    let scan = match row.entry {
        AnalysisRowEntry::File {
            scan:
                AnalysisFileScan::Scanned {
                    content_key,
                    metadata,
                    ..
                },
        } => AnalysisFileScan::Scanned {
            content_key,
            metadata,
            refresh_failure: Some(Box::new(failure)),
        },
        AnalysisRowEntry::File {
            scan:
                AnalysisFileScan::SettledOutput {
                    source_content_key,
                    output_content_key,
                    metadata,
                    ..
                },
        } => AnalysisFileScan::SettledOutput {
            source_content_key,
            output_content_key,
            metadata,
            refresh_failure: Some(Box::new(failure)),
        },
        AnalysisRowEntry::File {
            scan: AnalysisFileScan::Discovered | AnalysisFileScan::Failed { .. },
        } => AnalysisFileScan::Failed { failure },
        AnalysisRowEntry::Folder { .. } => return Err("Basic Scan row is not a file"),
    };
    row.entry = AnalysisRowEntry::File { scan };
    Ok(row)
}

fn settled_output_match(
    state: &AppState,
    mut predicate: impl FnMut(&crate::ArtifactIdentity) -> bool,
) -> Option<(ContentKey, ContentKey, crate::ArtifactIdentity)> {
    state
        .durable
        .outputs
        .iter()
        .rev()
        .find_map(|(run_id, transaction)| {
            let artifact = match &transaction.state {
                OutputState::Committed { final_identity }
                | OutputState::Retired { final_identity } => final_identity,
                _ => return None,
            };
            if !predicate(artifact) {
                return None;
            }
            let source = state
                .durable
                .conversion_runs
                .get(run_id)?
                .spec
                .content_key
                .clone()?;
            Some((source, artifact.content_key.clone(), artifact.clone()))
        })
}

fn apply_settings(state: &AppState, command: SettingsCommand) -> Applied {
    match command {
        SettingsCommand::Set { settings } => {
            if let Err(reason) = settings.validate() {
                return Applied::rejected(reason);
            }
            let mut applied = Applied::accepted();
            if settings != state.settings {
                applied.config.push(ConfigDelta::SettingsChanged {
                    settings: settings.clone(),
                });
            }
            applied.effects.push(Effect::WriteSettings { settings });
            applied
        }
    }
}

fn apply_queue(state: &AppState, command: QueueCommand) -> Applied {
    match command {
        QueueCommand::AddMany { requests } => {
            let mut accepted: Vec<QueueItem> = Vec::new();
            let mut skipped: Vec<(SkipReason, u32)> = Vec::new();
            for request in requests {
                // Item ids are caller-allocated and unique by contract; a
                // collision is a wiring bug that rejects the whole batch.
                if state
                    .durable
                    .queue
                    .iter()
                    .any(|item| item.id == request.item_id)
                    || accepted.iter().any(|item| item.id == request.item_id)
                {
                    return Applied::rejected("queue item id already exists");
                }
                // One item per path: a path re-add is only meaningful once
                // the standing item is finished. Changing an item's
                // operation is Edit's job, not a second add's.
                let already_queued = state
                    .durable
                    .queue
                    .iter()
                    .filter(|item| !matches!(item.state, QueueItemState::Finished(_)))
                    .chain(accepted.iter())
                    .any(|item| item.input == request.input);
                let skip = if already_queued {
                    Some(SkipReason::AlreadyQueued)
                } else {
                    request.path_hash.as_ref().and_then(|path_hash| {
                        evaluate_enqueue(
                            &state.durable,
                            path_hash,
                            request.identity.as_ref(),
                            request.timestamp_reliability,
                            request.operation,
                            request.intent,
                        )
                    })
                };
                match skip {
                    Some(reason) => count_skip(&mut skipped, reason),
                    None => accepted.push(QueueItem {
                        id: request.item_id,
                        input: request.input,
                        operation: request.operation,
                        intent: request.intent,
                        output_target: request.output_target,
                        overwrite: request.overwrite,
                        state: QueueItemState::Queued,
                    }),
                }
            }
            let mut applied = Applied::accepted();
            let added = u32::try_from(accepted.len()).unwrap_or(u32::MAX);
            for item in accepted {
                applied.durable.push(DurableDelta::QueueAdded { item });
            }
            applied
                .ephemeral
                .push(EphemeralDelta::QueueAddSummary { added, skipped });
            applied
        }
        QueueCommand::RemoveMany { item_ids } => {
            if item_ids.is_empty() {
                return Applied::rejected("at least one queue item must be removed");
            }
            let requested = item_ids.iter().copied().collect::<BTreeSet<_>>();
            if requested.len() != item_ids.len() {
                return Applied::rejected("queue removal contains duplicate item ids");
            }
            for item_id in &requested {
                let Some(item) = find_item(state, *item_id) else {
                    return Applied::rejected("queue item does not exist");
                };
                if !matches!(
                    item.state,
                    QueueItemState::Queued | QueueItemState::Finished(_)
                ) {
                    return Applied::rejected("active queue item cannot be removed");
                }
            }
            // Canonical queue order makes the durable set deterministic while
            // retaining every unselected item's relative position.
            let item_ids = state
                .durable
                .queue
                .iter()
                .filter(|item| requested.contains(&item.id))
                .map(|item| item.id)
                .collect();
            let mut applied = Applied::accepted();
            applied
                .durable
                .push(DurableDelta::QueueItemsRemoved { item_ids });
            applied
        }
        QueueCommand::Move { item_id, before } => {
            let Some(item) = find_item(state, item_id) else {
                return Applied::rejected("queue item does not exist");
            };
            if !matches!(item.state, QueueItemState::Queued) {
                return Applied::rejected("only queued items can be reordered");
            }
            // The journal records where the item actually lands, so the fold
            // stays positional: a destination above the frozen finished/active
            // prefix resolves to the first pending slot before it is written.
            let resolved = match before {
                None => None,
                Some(before_id) => {
                    let Some(before_item) = find_item(state, before_id) else {
                        return Applied::rejected("queue destination does not exist");
                    };
                    if matches!(before_item.state, QueueItemState::Queued) {
                        Some(before_id)
                    } else {
                        state
                            .durable
                            .queue
                            .iter()
                            .find(|entry| matches!(entry.state, QueueItemState::Queued))
                            .map(|entry| entry.id)
                    }
                }
            };
            if resolved == Some(item_id) {
                return Applied::accepted();
            }
            let mut applied = Applied::accepted();
            applied.durable.push(DurableDelta::QueueMoved {
                item_id,
                before: resolved,
            });
            applied
        }
        QueueCommand::ReorderPending { pending_order } => {
            if let Err(reason) =
                crate::state::validate_pending_order(&state.durable.queue, &pending_order)
            {
                return Applied::rejected(reason);
            }
            let current_order = state
                .durable
                .queue
                .iter()
                .filter(|item| matches!(item.state, QueueItemState::Queued))
                .map(|item| item.id)
                .collect::<Vec<_>>();
            if current_order == pending_order {
                return Applied::accepted();
            }
            let mut applied = Applied::accepted();
            applied
                .durable
                .push(DurableDelta::QueueReordered { pending_order });
            applied
        }
        QueueCommand::Clear => {
            if state.session != SessionState::Idle {
                return Applied::rejected("queue can only be cleared while idle");
            }
            let item_ids = state
                .durable
                .queue
                .iter()
                .filter(|item| {
                    matches!(
                        item.state,
                        QueueItemState::Queued | QueueItemState::Finished(_)
                    )
                })
                .map(|item| item.id)
                .collect::<Vec<_>>();
            let mut applied = Applied::accepted();
            if !item_ids.is_empty() {
                applied
                    .durable
                    .push(DurableDelta::QueueItemsRemoved { item_ids });
            }
            applied
        }
        QueueCommand::ClearCompleted => {
            let item_ids = state
                .durable
                .queue
                .iter()
                .filter(|item| {
                    matches!(&item.state, QueueItemState::Finished(outcome)
                        if !matches!(outcome, ItemOutcome::Failed(_)))
                })
                .map(|item| item.id)
                .collect::<Vec<_>>();
            let mut applied = Applied::accepted();
            if !item_ids.is_empty() {
                applied
                    .durable
                    .push(DurableDelta::QueueItemsRemoved { item_ids });
            }
            applied
        }
        QueueCommand::Retry { item_id, patch } => {
            if patch.is_some() && state.session != SessionState::Idle {
                return Applied::rejected("queue items can only be edited while idle");
            }
            let Some(item) = find_item(state, item_id) else {
                return Applied::rejected("queue item does not exist");
            };
            if !matches!(item.state, QueueItemState::Finished(_)) {
                return Applied::rejected("only finished items can be retried");
            }
            let patch = patch.unwrap_or_default();
            let operation = patch.operation.unwrap_or(item.operation);
            let intent = patch.intent.unwrap_or(item.intent);
            let output_target = patch
                .output_target
                .unwrap_or_else(|| item.output_target.clone());
            let overwrite = patch.overwrite.unwrap_or(item.overwrite);
            let mut applied = Applied::accepted();
            applied.durable.push(DurableDelta::QueueRetried {
                item_id,
                operation,
                intent,
                output_target,
                overwrite,
            });
            applied
        }
        QueueCommand::Edit { item_id, patch } => {
            if state.session != SessionState::Idle {
                return Applied::rejected("queue items can only be edited while idle");
            }
            let Some(item) = find_item(state, item_id) else {
                return Applied::rejected("queue item does not exist");
            };
            if !matches!(item.state, QueueItemState::Queued) {
                return Applied::rejected("only queued items can be edited");
            }
            let operation = patch.operation.unwrap_or(item.operation);
            let intent = patch.intent.unwrap_or(item.intent);
            let output_target = patch
                .output_target
                .unwrap_or_else(|| item.output_target.clone());
            let overwrite = patch.overwrite.unwrap_or(item.overwrite);
            if operation == item.operation
                && intent == item.intent
                && output_target == item.output_target
                && overwrite == item.overwrite
            {
                return Applied::accepted();
            }
            let mut applied = Applied::accepted();
            applied.durable.push(DurableDelta::QueueEdited {
                item_id,
                operation,
                intent,
                output_target,
                overwrite,
            });
            applied
        }
    }
}

fn count_skip(skipped: &mut Vec<(SkipReason, u32)>, reason: SkipReason) {
    if let Some((_, count)) = skipped.iter_mut().find(|(existing, _)| *existing == reason) {
        *count = count.saturating_add(1);
    } else {
        skipped.push((reason, 1));
    }
}

fn tools_transition(state: &AppState, next: ToolsState) -> Applied {
    let mut applied = Applied::accepted();
    if state.tools != next {
        applied.ephemeral.push(EphemeralDelta::ToolsChanged(next));
    }
    applied
}

/// Whether a vendor worker is (or is about to be) running. `Failed` and
/// `Idle` are the only restable states.
fn vendor_worker_active(activity: &VendorActivity) -> bool {
    matches!(
        activity,
        VendorActivity::Checking | VendorActivity::Downloading { .. } | VendorActivity::Installing
    )
}

/// Whether the installed tool binaries may currently be swapped out from
/// under a starting session.
fn vendor_swapping_tools(activity: &VendorActivity) -> bool {
    matches!(
        activity,
        VendorActivity::Downloading { .. } | VendorActivity::Installing
    )
}

fn apply_vendor(state: &AppState, command: VendorCommand) -> Applied {
    if vendor_worker_active(&state.tools.activity) {
        return Applied::rejected("a vendor operation is already in progress");
    }
    if state
        .analysis
        .current
        .as_ref()
        .is_some_and(|generation| generation.activity == AnalysisActivity::BasicScanning)
    {
        return Applied::rejected("vendor operations cannot run during Basic Scan");
    }
    match command {
        VendorCommand::Install => {
            // Idle-only swap: an install replaces the binaries a session
            // worker would execute, so it is refused whenever a session or
            // claimed item exists — including crash-recovered actives.
            if state.session != SessionState::Idle {
                return Applied::rejected("vendor install requires an idle session");
            }
            if active_run(state).is_some() {
                return Applied::rejected(
                    "vendor install cannot start while a queue item is active",
                );
            }
            let mut applied = tools_transition(
                state,
                ToolsState {
                    activity: VendorActivity::Downloading {
                        received: 0,
                        total: None,
                    },
                    ..state.tools.clone()
                },
            );
            applied.effects.push(Effect::VendorInstall);
            applied
        }
        VendorCommand::Check => {
            let mut applied = tools_transition(
                state,
                ToolsState {
                    activity: VendorActivity::Checking,
                    ..state.tools.clone()
                },
            );
            applied.effects.push(Effect::VendorCheck);
            applied
        }
    }
}

fn apply_history(state: &AppState, command: HistoryCommand) -> Applied {
    match command {
        HistoryCommand::Import { records } => {
            let mut applied = Applied::accepted();
            let mut known: std::collections::BTreeSet<ImportPath> = state
                .durable
                .parked
                .keys()
                .cloned()
                .chain(state.durable.adopted_imports.iter().cloned())
                .collect();
            let mut fresh: Vec<(ImportPath, ImportedHistoryRecord)> = Vec::new();
            let mut skipped: u32 = 0;
            for (import_path, parked) in records {
                if known.contains(&import_path) {
                    skipped = skipped.saturating_add(1);
                } else {
                    known.insert(import_path.clone());
                    fresh.push((import_path, parked));
                }
            }
            let parked = u32::try_from(fresh.len()).unwrap_or(u32::MAX);
            if !fresh.is_empty() {
                applied
                    .durable
                    .push(DurableDelta::HistoryImported { records: fresh });
            }
            applied.reply = Reply::Imported { parked, skipped };
            applied
        }
    }
}

fn apply_session(state: &AppState, command: SessionCommand) -> Applied {
    match command {
        SessionCommand::Start if state.session == SessionState::Idle => {
            if let ToolAvailability::Missing { detail, .. } = &state.tools.availability {
                return Applied::rejected(format!("media tools are unavailable: {detail}"));
            }
            if vendor_swapping_tools(&state.tools.activity) {
                return Applied::rejected("a vendor install is in progress");
            }
            let mut applied = Applied::accepted();
            applied
                .ephemeral
                .push(EphemeralDelta::SessionChanged(SessionState::Running));
            // A new session starts its aggregates from zero; the previous
            // session's totals are display state, not history.
            applied.ephemeral.push(EphemeralDelta::SessionAggregates(
                SessionAggregates::default(),
            ));
            applied.effects.push(Effect::StartWorker);
            applied
        }
        SessionCommand::Start => Applied::rejected("session is already active"),
        SessionCommand::StopAfterCurrent if state.session == SessionState::Running => {
            let mut applied = Applied::accepted();
            let next = if active_run(state).is_some() {
                SessionState::StopAfterCurrent
            } else {
                SessionState::Idle
            };
            applied.ephemeral.push(EphemeralDelta::SessionChanged(next));
            applied
        }
        SessionCommand::StopAfterCurrent => Applied::rejected("session is not running"),
        SessionCommand::ForceStop
            if matches!(
                state.session,
                SessionState::Running | SessionState::StopAfterCurrent
            ) =>
        {
            let mut applied = Applied::accepted();
            if let Some(run_id) = active_run(state) {
                applied
                    .ephemeral
                    .push(EphemeralDelta::SessionChanged(SessionState::ForceStopping));
                applied.effects.push(Effect::KillActiveRun { run_id });
            } else {
                applied
                    .ephemeral
                    .push(EphemeralDelta::SessionChanged(SessionState::Idle));
            }
            applied
        }
        SessionCommand::ForceStop => Applied::rejected("session is not running"),
    }
}

fn apply_worker(state: &AppState, command: WorkerCommand) -> Applied {
    match command {
        WorkerCommand::ReserveNext { claim_id, run_id } => {
            if state.session != SessionState::Running {
                return Applied::rejected("session is not accepting another claim");
            }
            if active_run(state).is_some() {
                return Applied::rejected("another queue item is active");
            }
            let Some(item) = state
                .durable
                .queue
                .iter()
                .find(|item| matches!(item.state, QueueItemState::Queued))
            else {
                let mut applied = Applied::accepted();
                applied.reply = Reply::Reserved(None);
                return applied;
            };
            let job = ReservedJob {
                item_id: item.id,
                claim_id,
                run_id,
                input: item.input.clone(),
                operation: item.operation,
                intent: item.intent,
                output_target: item.output_target.clone(),
            };
            let mut applied = Applied::accepted();
            applied.durable.push(DurableDelta::ItemReserved {
                job: Box::new(job.clone()),
            });
            applied.reply = Reply::Reserved(Some(Box::new(job)));
            applied
        }
        WorkerCommand::PrepareReserved {
            item_id,
            claim_id,
            run_id,
            observation,
            import_paths,
            mut execution,
        } => {
            let Some(item) = find_item(state, item_id) else {
                return Applied::rejected("queue item does not exist");
            };
            if !matches!(
                item.state,
                QueueItemState::Reserved {
                    claim_id: current_claim,
                    run_id: current_run,
                } if current_claim == claim_id && current_run == run_id
            ) {
                return Applied::rejected("worker preparation has a stale reservation");
            }
            execution.overwrite_existing = match item.overwrite {
                OverwriteDecision::FollowSettings => state.settings.output.overwrite_existing,
                OverwriteDecision::Allow => true,
                OverwriteDecision::Deny => false,
            };
            execution.decode_preference = if state.settings.hardware_decode {
                DecodePreference::HardwarePreferred
            } else {
                execution.profile.decode_mode = DecodeMode::Software;
                DecodePreference::SoftwareOnly
            };
            if let Err(reason) = execution.validate() {
                return Applied::rejected(reason);
            }
            let content_key = observation
                .as_ref()
                .map(|observed| observed.binding.content_key.clone());
            let record = content_key
                .as_ref()
                .and_then(|key| state.durable.records.get(key));
            // Compute import effects before selecting the action: replay
            // validation recomputes that action after these deltas fold, so
            // the live decision must observe the same post-adoption record.
            let prepared_imports = prepare_import_adoptions(
                &state.durable,
                observation.as_deref(),
                &import_paths,
                content_key.as_ref(),
                record,
            );
            let record_for_action = prepared_imports.effective_record.as_ref().or(record);
            let metadata = observation
                .as_ref()
                .map(|observed| &observed.metadata)
                .or_else(|| record_for_action.map(|known| &known.metadata));
            let action = select_job_action(
                metadata,
                record_for_action,
                item.operation,
                item.intent,
                &execution,
            );
            if let Some(selected) = action.selected_analysis()
                && let Err(reason) = selected.validate_reusable_for(&execution)
            {
                return Applied::rejected(reason);
            }
            let spec = JobSpec {
                item_id,
                claim_id,
                run_id,
                input: item.input.clone(),
                content_key,
                operation: item.operation,
                intent: item.intent,
                output_target: item.output_target.clone(),
                execution,
                action,
            };
            let mut applied = Applied::accepted();
            if let Some(observation) = observation {
                let path_changed = state
                    .durable
                    .paths
                    .get(&observation.path_hash)
                    .is_none_or(|binding| binding != &observation.binding);
                let record_changed = state
                    .durable
                    .records
                    .get(&observation.binding.content_key)
                    .is_none_or(|record| record.metadata != observation.metadata);
                if path_changed || record_changed {
                    applied
                        .durable
                        .push(DurableDelta::MediaObserved { observation });
                }
                // The adoption deltas land AFTER MediaObserved so the
                // content record exists when an adoption folds.
                applied.durable.extend(prepared_imports.deltas);
            }
            applied.durable.push(DurableDelta::ItemPrepared {
                spec: Box::new(spec.clone()),
            });
            applied.reply = Reply::Claimed(Some(Box::new(ClaimedJob { spec })));
            applied
        }
        WorkerCommand::AbandonReservation {
            item_id,
            claim_id,
            run_id,
            at,
        } => {
            let Some(item) = find_item(state, item_id) else {
                return Applied::rejected("queue item does not exist");
            };
            if !matches!(
                item.state,
                QueueItemState::Reserved {
                    claim_id: current_claim,
                    run_id: current_run,
                } if current_claim == claim_id && current_run == run_id
            ) || state.durable.conversion_runs.contains_key(&run_id)
            {
                return Applied::rejected("reservation cannot be abandoned from its current state");
            }
            let mut applied = Applied::accepted();
            let mut aggregates = state.aggregates;
            aggregates.absorb(&ItemOutcome::Stopped, &[]);
            applied
                .ephemeral
                .push(EphemeralDelta::SessionAggregates(aggregates));
            applied.durable.push(DurableDelta::ItemFinished {
                item_id,
                claim_id,
                run_id,
                outcome: ItemOutcome::Stopped,
                at,
                phase_spans: Vec::new(),
            });
            applied
        }
        WorkerCommand::Started {
            item_id,
            claim_id,
            run_id,
            at,
        } => transition_active(state, item_id, claim_id, run_id, |applied| {
            applied.durable.push(DurableDelta::ItemRunning {
                item_id,
                claim_id,
                run_id,
                at,
            });
        }),
        WorkerCommand::Output(output_delta) => {
            let run_id = output_delta.run_id();
            if active_run(state) != Some(run_id) {
                return Applied::rejected("output event does not belong to the active run");
            }
            if let Err(reason) = crate::output::validate_output_delta(&state.durable, &output_delta)
            {
                return Applied::rejected(reason);
            }
            let mut applied = Applied::accepted();
            applied.durable.push(DurableDelta::Output(output_delta));
            applied
        }
        WorkerCommand::RecordAnalysis {
            item_id,
            claim_id,
            run_id,
            result,
        } => transition_active(state, item_id, claim_id, run_id, |applied| {
            let Some(run) = state.durable.conversion_runs.get(&run_id) else {
                applied.reject("conversion run does not exist");
                return;
            };
            if run.analysis.is_some() {
                applied.reject("analysis is already recorded");
                return;
            }
            if let Err(reason) = result.validate_for(&run.spec.execution) {
                applied.reject(reason);
                return;
            }
            applied
                .durable
                .push(DurableDelta::AnalysisRecorded { run_id, result });
        }),
        WorkerCommand::Terminal {
            item_id,
            claim_id,
            run_id,
            outcome,
            at,
            phase_spans,
            final_telemetry,
        } => transition_active(state, item_id, claim_id, run_id, |applied| {
            let unsettled = state
                .durable
                .outputs
                .get(&run_id)
                .is_some_and(|transaction| !transaction.is_settled());
            if unsettled {
                applied.reject("output transaction is not settled");
                return;
            }
            let Some(run) = state.durable.conversion_runs.get(&run_id) else {
                applied.reject("conversion run does not exist");
                return;
            };
            if let Err(reason) =
                validate_terminal(run, state.durable.outputs.get(&run_id), &outcome)
            {
                applied.reject(reason);
                return;
            }
            if let Some(telemetry) = final_telemetry {
                applied.ephemeral.push(EphemeralDelta::Telemetry(telemetry));
            }
            applied
                .ephemeral
                .push(EphemeralDelta::TelemetryCleared { run_id });
            let mut aggregates = state.aggregates;
            aggregates.absorb(&outcome, &phase_spans);
            applied
                .ephemeral
                .push(EphemeralDelta::SessionAggregates(aggregates));
            applied.durable.push(DurableDelta::ItemFinished {
                item_id,
                claim_id,
                run_id,
                outcome,
                at,
                phase_spans,
            });
        }),
        WorkerCommand::Finished => {
            if active_run(state).is_some() {
                return Applied::rejected("worker cannot finish while an item is active");
            }
            if state.session == SessionState::Idle {
                return Applied::accepted();
            }
            let mut applied = Applied::accepted();
            applied
                .ephemeral
                .push(EphemeralDelta::SessionChanged(SessionState::Idle));
            applied
        }
        WorkerCommand::Crashed { message } => {
            let mut applied = Applied::accepted();
            applied
                .ephemeral
                .push(EphemeralDelta::WorkerCrashed { message });
            applied.effects.push(Effect::StopDriver);
            applied
        }
    }
}

pub(crate) fn validate_terminal(
    run: &ConversionRun,
    output: Option<&crate::OutputTransaction>,
    outcome: &ItemOutcome,
) -> Result<(), &'static str> {
    match outcome {
        ItemOutcome::Analyzed => {
            if !matches!(run.spec.action, JobAction::Analyze { .. })
                || run.analysis.is_none()
                || output.is_some()
            {
                return Err("analyzed outcome is incompatible with the run state");
            }
        }
        ItemOutcome::Converted(evidence) => {
            if !matches!(run.spec.action, JobAction::Encode { .. }) || run.analysis.is_none() {
                return Err("converted outcome requires a converted run with durable analysis");
            }
            if !has_successful_output(output) {
                return Err("converted outcome requires a successfully settled output");
            }
            if run.started_at.is_none() {
                return Err("successful outcome requires a started run");
            }
            if matches!(evidence, CompletionEvidence::LiveRemux { .. }) {
                return Err("converted outcome cannot carry remux evidence");
            }
        }
        ItemOutcome::Remuxed(evidence) => {
            if !matches!(run.spec.action, JobAction::Remux) || run.analysis.is_some() {
                return Err("remuxed outcome requires a remux run without analysis");
            }
            if !has_successful_output(output) {
                return Err("remuxed outcome requires a successfully settled output");
            }
            if run.started_at.is_none() {
                return Err("successful outcome requires a started run");
            }
            if matches!(evidence, CompletionEvidence::LiveEncode { .. }) {
                return Err("remuxed outcome cannot carry encode evidence");
            }
        }
        ItemOutcome::NotWorthwhile { attempts } => {
            if run.analysis.is_some() || output.is_some() {
                return Err("not-worthwhile outcome cannot retain analysis or output state");
            }
            if attempts.is_empty()
                || attempts.iter().any(|attempt| {
                    attempt.target > run.spec.execution.requested_target
                        || attempt.target < run.spec.execution.fallback_floor
                        || attempt
                            .last_measurement
                            .as_ref()
                            .is_some_and(|measurement| measurement.validate().is_err())
                })
            {
                return Err("not-worthwhile attempts are inconsistent with the claimed job");
            }
        }
        ItemOutcome::Skipped { reason } => match reason {
            crate::SkipReason::LowResolution { .. }
            | crate::SkipReason::AlreadyAv1Matroska
            | crate::SkipReason::NotWorthwhile { .. }
            | crate::SkipReason::ProbableDuplicate { .. } => {
                if !matches!(&run.spec.action, JobAction::Skip { reason: expected } if expected == reason)
                    || run.analysis.is_some()
                    || output.is_some()
                {
                    return Err("policy skip does not match the prepared job");
                }
            }
            crate::SkipReason::OutputExists => {
                if !run.spec.action.produces_output() || output.is_some() {
                    return Err("output-exists skip is incompatible with the run state");
                }
            }
            crate::SkipReason::AlreadyConverted { .. } | crate::SkipReason::AlreadyQueued => {
                return Err("enqueue-time skip reasons are never terminal outcomes");
            }
        },
        ItemOutcome::Failed(facts) => {
            facts.diagnostic.validate()?;
            // An output-conflict failure asserts the transaction really did
            // settle as a conflict. The converse is deliberately NOT an
            // invariant: a conflicted settlement followed by a Stopped
            // terminal is legal (cancellation racing a settlement failure).
            if matches!(facts.kind, crate::FailureKind::OutputConflict)
                && !output.is_some_and(|transaction| {
                    matches!(transaction.state, crate::OutputState::Conflict { .. })
                })
            {
                return Err("output-conflict failure requires a conflicted output transaction");
            }
        }
        ItemOutcome::Stopped => {}
    }
    Ok(())
}

fn has_successful_output(output: Option<&crate::OutputTransaction>) -> bool {
    output.is_some_and(|transaction| {
        matches!(
            (&transaction.replacement, &transaction.state),
            (
                crate::Replacement::KeepOriginal,
                crate::OutputState::Committed { .. }
            ) | (
                crate::Replacement::RetireOriginal,
                crate::OutputState::Retired { .. }
            )
        )
    })
}

fn transition_active(
    state: &AppState,
    item_id: QueueItemId,
    claim_id: ClaimId,
    run_id: RunId,
    update: impl FnOnce(&mut Applied),
) -> Applied {
    let Some(item) = find_item(state, item_id) else {
        return Applied::rejected("queue item does not exist");
    };
    let matching = matches!(
        item.state,
        QueueItemState::Reserved {
            claim_id: current_claim,
            run_id: current_run,
        } | QueueItemState::Claimed {
            claim_id: current_claim,
            run_id: current_run,
        } | QueueItemState::Running {
            claim_id: current_claim,
            run_id: current_run,
        } if current_claim == claim_id && current_run == run_id
    );
    if !matching {
        return Applied::rejected("worker event has a stale claim or run id");
    }
    let mut applied = Applied::accepted();
    update(&mut applied);
    applied
}

fn find_item(state: &AppState, item_id: QueueItemId) -> Option<&QueueItem> {
    state.durable.queue.iter().find(|item| item.id == item_id)
}

fn active_run(state: &AppState) -> Option<RunId> {
    state
        .durable
        .queue
        .iter()
        .find_map(|item| match item.state {
            QueueItemState::Claimed { run_id, .. } | QueueItemState::Running { run_id, .. } => {
                Some(run_id)
            }
            QueueItemState::Reserved { run_id, .. } => Some(run_id),
            QueueItemState::Queued | QueueItemState::Finished(_) => None,
        })
}
