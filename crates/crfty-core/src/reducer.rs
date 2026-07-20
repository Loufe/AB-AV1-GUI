use std::path::PathBuf;

use serde::Serialize;

use crate::state::ConversionRun;
use crate::{
    AnalysisIntent, AnalysisResult, AppState, ClaimId, ClaimedJob, CompletionEvidence, ConfigDelta,
    CorruptionSignature, DecodeMode, DecodePreference, DurableDelta, ExecutionSettings,
    ItemOutcome, JobAction, JobSpec, MediaObservation, Operation, OutputDelta, OutputTarget,
    PhaseSpan, QueueItem, QueueItemId, QueueItemState, ReservedJob, RunId, SessionState, Settings,
    Telemetry, ToolAvailability, ToolsState, UnixMillis, VendorActivity, fold, fold_config,
    select_job_action,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Queue(QueueCommand),
    Session(SessionCommand),
    Settings(SettingsCommand),
    Worker(WorkerCommand),
    Vendor(VendorCommand),
    System(SystemCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCommand {
    Set { settings: Settings },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueCommand {
    Add {
        item_id: QueueItemId,
        input: PathBuf,
        operation: Operation,
        intent: AnalysisIntent,
        output_target: OutputTarget,
    },
    Remove {
        item_id: QueueItemId,
    },
    Move {
        item_id: QueueItemId,
        before: Option<QueueItemId>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum EphemeralDelta {
    SessionChanged(SessionState),
    Telemetry(Telemetry),
    TelemetryCleared { run_id: RunId },
    ToolsChanged(ToolsState),
    WorkerCrashed { message: String },
    CommandRejected { reason: String },
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
    Reserved(Option<Box<ReservedJob>>),
    Claimed(Option<Box<ClaimedJob>>),
    Rejected { reason: String },
    DurabilityUnknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
        Command::Queue(command) => apply_queue(state, command),
        Command::Session(command) => apply_session(state, command),
        Command::Settings(command) => apply_settings(state, command),
        Command::Worker(command) => apply_worker(state, command),
        Command::Vendor(command) => apply_vendor(state, command),
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
            EphemeralDelta::SessionChanged(session) => state.session = session.clone(),
            EphemeralDelta::Telemetry(telemetry) => {
                state.telemetry.insert(telemetry.run_id, telemetry.clone());
            }
            EphemeralDelta::TelemetryCleared { run_id } => {
                state.telemetry.remove(run_id);
            }
            EphemeralDelta::ToolsChanged(tools) => state.tools = tools.clone(),
            EphemeralDelta::WorkerCrashed { .. } | EphemeralDelta::CommandRejected { .. } => {}
        }
    }
    applied
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
        QueueCommand::Add {
            item_id,
            input,
            operation,
            intent,
            output_target,
        } => {
            if state.durable.queue.iter().any(|item| item.id == item_id) {
                return Applied::rejected("queue item id already exists");
            }
            let mut applied = Applied::accepted();
            applied.durable.push(DurableDelta::QueueAdded {
                item: QueueItem {
                    id: item_id,
                    input,
                    operation,
                    intent,
                    output_target,
                    state: QueueItemState::Queued,
                },
            });
            applied
        }
        QueueCommand::Remove { item_id } => {
            let Some(item) = find_item(state, item_id) else {
                return Applied::rejected("queue item does not exist");
            };
            if !matches!(
                item.state,
                QueueItemState::Queued | QueueItemState::Finished(_)
            ) {
                return Applied::rejected("active queue item cannot be removed");
            }
            let mut applied = Applied::accepted();
            applied.durable.push(DurableDelta::QueueRemoved { item_id });
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
            mut execution,
        } => {
            execution.overwrite_existing = state.settings.output.overwrite_existing;
            execution.decode_preference = if state.settings.hardware_decode {
                DecodePreference::HardwarePreferred
            } else {
                execution.profile.decode_mode = DecodeMode::Software;
                DecodePreference::SoftwareOnly
            };
            if let Err(reason) = execution.validate() {
                return Applied::rejected(reason);
            }
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
            let content_key = observation
                .as_ref()
                .map(|observed| observed.binding.content_key.clone());
            let record = content_key
                .as_ref()
                .and_then(|key| state.durable.records.get(key));
            let metadata = observation
                .as_ref()
                .map(|observed| &observed.metadata)
                .or_else(|| record.map(|known| &known.metadata));
            let action =
                select_job_action(metadata, record, item.operation, item.intent, &execution);
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
            crate::SkipReason::LowResolution { .. } | crate::SkipReason::AlreadyAv1Matroska => {
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
