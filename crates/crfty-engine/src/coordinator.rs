use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crfty_core::{
    AnalysisAttempt, AnalysisGenerationId, AnalysisResult, AppSnapshot, CRF_FIXED_SCALE, ClaimId,
    ClaimedJob, Command, CompletionEvidence, ConflictKind, CorruptionSignature, Crf, DecodeMode,
    DurableDelta, DurableState, DurationMs, Effect, ExecutionSettings, FailureFacts, FailureKind,
    HistoryCommand, ItemOutcome, JobAction, JobPhase, JobProgress, MAX_PERCENT_BASIS_POINTS,
    MAX_VMAF_SCORE, OutputDelta, OutputState, OutputTarget, OutputTransaction,
    PERCENT_BASIS_POINTS_SCALE, PhaseSpan, ProjectionCommand, QueueCommand, QueueItemState,
    Replacement, Reply, RunId, SearchMeasurement, SessionCommand, SettingsCommand, SkipReason,
    StreamByteSizes, SystemCommand, Telemetry, UnixMillis, VMAF_SCORE_FIXED_SCALE, VendorActivity,
    VendorCommand, VideoExtension, VmafScore, VmafTarget, WorkerCommand, fold,
};

const FIRST_RUNTIME_ID: u64 = 1;
/// Depth of the public event channel. A healthy consumer drains continuously,
/// so occupancy stays near zero; the bound only bites once the consumer has
/// stopped, and must comfortably exceed the startup burst (snapshot plus
/// drained ephemerals) that is buffered before any consumer exists. Public so
/// the overflow test can size its event flood relative to it.
pub const PUBLIC_EVENT_CHANNEL_CAPACITY: usize = 1024;
const ADAPTER_REPORT_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How long shutdown waits for the vendor worker to observe cancellation and
/// unwind through its own staging cleanup before abandoning the thread to
/// process exit (#33 §12). Cancellation is observed between download chunks,
/// so anything slower than this is a wedged network read.
const VENDOR_SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const VENDOR_SHUTDOWN_POLL: Duration = Duration::from_millis(25);
const NORMALIZED_PROGRESS_MIN: f32 = 0.0;
const NORMALIZED_PROGRESS_MAX: f32 = 1.0;
const OUTPUT_CONTAINER_EXTENSION: &str = "mkv";

type MediaOutputManager = OutputManager<MediaArtifactInspector>;

struct OutputDestination {
    final_path: PathBuf,
    replacement: Replacement,
}

struct PreparedOutput {
    manager: MediaOutputManager,
    transaction: OutputTransaction,
}

#[derive(Clone, Copy)]
struct JobServices<'a> {
    commands: &'a CommandSender,
    runtime: &'a AbAv1Runtime,
    tools: &'a MediaTools,
    cancellation: &'a ActiveCancellation,
    /// Input media duration from the claim-time preflight probe; the total
    /// the encode/remux output position runs toward, so the ETA's remaining
    /// work is known. `None` (probe failed or reported zero) means no ETA.
    input_duration_ms: Option<u64>,
}

/// What a successful adapter run measured before settlement. The terminal
/// outcome is built only after the output transaction settles, from these
/// facts plus the settled ledger state.
enum SuccessfulJob {
    Encode {
        outcome: EncodeOutcome,
        decode_mode: DecodeMode,
    },
    Remux,
}

/// Wall-clock instant for durable command payloads; core has no clock. A
/// pre-epoch system clock degrades to zero rather than failing the run.
pub(crate) fn now_millis() -> UnixMillis {
    UnixMillis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            }),
    )
}

/// Per-job telemetry sequencing plus monotonic phase-span accumulation. The
/// spans ride the lossless terminal command; telemetry stays the lossy path.
struct PhaseTracker {
    sequence: u64,
    current: Option<(JobPhase, Instant)>,
    spans: Vec<PhaseSpan>,
}

impl PhaseTracker {
    fn new() -> Self {
        Self {
            sequence: 0,
            current: None,
            spans: Vec::new(),
        }
    }

    fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    /// Starts measuring `phase`, closing the previous span. Re-entering the
    /// running phase is a no-op so repeated search attempts accumulate into
    /// one span instead of fragmenting.
    fn enter(&mut self, phase: JobPhase) {
        if self
            .current
            .as_ref()
            .is_some_and(|(active, _)| *active == phase)
        {
            return;
        }
        self.close_current();
        self.current = Some((phase, Instant::now()));
    }

    fn close_current(&mut self) {
        if let Some((phase, entered)) = self.current.take() {
            let elapsed = u64::try_from(entered.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.spans.push(PhaseSpan {
                phase,
                duration: DurationMs(elapsed),
            });
        }
    }

    fn finish(&mut self) -> Vec<PhaseSpan> {
        self.close_current();
        std::mem::take(&mut self.spans)
    }
}

use crate::{
    ab_av1::{
        AbAv1Runtime, CancelMode, CancellationHandle, EncodeOutcome, EncodeRequest, JobFailureKind,
        JobHandle, JobReport, JobTerminal, SearchOutcome, SearchRequest,
        Telemetry as AdapterTelemetry,
    },
    driver::{CommandSender, DriverEvent, DriverHandle, DriverStartError},
    failure::scrub_tail,
    media::{DecodeResolver, MediaInspector},
    output::{MediaArtifactInspector, OutputManager},
    rate::{RateSample, RateTracker},
    remux::{
        self, RemuxCancellationHandle, RemuxHandle, RemuxReport, RemuxRequest, RemuxTelemetry,
        RemuxTerminal,
    },
    vendor::{
        discovery::{self, CurrentTools, DiscoveredTools, DiscoveryReport, MediaTools},
        download::HttpFetch,
        install::{self as vendor_install, InstallError, InstallProgress},
        manifest,
    },
};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub journal_path: PathBuf,
    pub config_path: PathBuf,
    /// Root of the managed vendor tree (`current.json`, `installs/`,
    /// `staging/`); the shell passes `<app data dir>/vendor`.
    pub vendor_root: PathBuf,
    pub tools: ToolsConfig,
    /// Base execution settings. The profile carries no tool revisions — the
    /// session worker composes the discovered revisions in before each claim,
    /// so only [`ExecutionSettings::validate_base`] applies here.
    pub execution: ExecutionSettings,
}

#[derive(Debug, Clone)]
pub enum ToolsConfig {
    /// Run vendor discovery (explicit env paths > managed install > PATH)
    /// against the vendor root at startup.
    Discover,
    /// Injected discovery outcome. Tests and the contract fixture pin tools
    /// and revisions without touching the process environment.
    Fixed(DiscoveredTools),
}

#[derive(Debug)]
pub enum EngineStartError {
    /// Another process holds the data-directory lock: a second instance. The
    /// shell surfaces this distinctly instead of a generic degraded state.
    AlreadyRunning {
        lock_path: PathBuf,
    },
    Failed(String),
}

impl fmt::Display for EngineStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning { lock_path } => write!(
                formatter,
                "another instance holds the data lock at {}",
                lock_path.display()
            ),
            Self::Failed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for EngineStartError {}

pub struct EngineRuntime {
    pub commands: UserCommandSender,
    pub events: mpsc::Receiver<DriverEvent>,
    driver: Option<DriverHandle>,
    supervisor: Option<thread::JoinHandle<()>>,
    event_forwarder: Option<thread::JoinHandle<()>>,
    analysis: Option<Arc<crate::analysis::AnalysisRuntime>>,
    runtime: Option<Arc<AbAv1Runtime>>,
}

impl EngineRuntime {
    pub fn start(config: EngineConfig) -> Result<Self, EngineStartError> {
        config.execution.validate_base().map_err(|reason| {
            EngineStartError::Failed(format!("invalid engine execution settings: {reason}"))
        })?;
        let runtime = Arc::new(AbAv1Runtime::start().map_err(|error| {
            EngineStartError::Failed(format!("failed to start encoder: {error}"))
        })?);
        // Unbounded by design: effects form a cycle — the driver emits them,
        // the supervisor turns them into commands submitted back into the
        // driver's bounded command channel — so a bound here could deadlock
        // the driver against its own supervisor. Depth is governed by the
        // reducer, which serializes work through the session and vendor
        // activity states and dedups effects per batch, never by event rate.
        let (effect_tx, effect_rx) = mpsc::channel();
        let mut driver =
            DriverHandle::start_with_effects(&config.journal_path, &config.config_path, effect_tx)
                .map_err(map_driver_start)?;
        let driver_events = driver.take_events().ok_or_else(|| {
            EngineStartError::Failed("driver event receiver is missing".to_owned())
        })?;
        let initial = match driver_events.recv() {
            Ok(DriverEvent::Snapshot(snapshot)) => snapshot,
            Ok(_) => {
                return Err(EngineStartError::Failed(
                    "driver did not emit its snapshot first".to_owned(),
                ));
            }
            Err(error) => {
                return Err(EngineStartError::Failed(format!(
                    "driver disconnected before startup recovery: {error}"
                )));
            }
        };
        let report = match &config.tools {
            ToolsConfig::Discover => discovery::discover(&config.vendor_root),
            ToolsConfig::Fixed(tools) => DiscoveryReport {
                tools: tools.clone(),
                update_available: false,
            },
        };
        // Availability is reported before recovery so the reducer's fail-closed
        // default is replaced by the real discovery result ahead of any
        // recovery events, and the ToolsChanged ephemeral is already queued
        // when the startup drain below forwards non-durable events.
        let discovered = driver
            .commands
            .submit(Command::System(SystemCommand::ToolsDiscovered {
                availability: report.tools.availability(),
                update_available: report.update_available,
            }))
            .map_err(|error| {
                EngineStartError::Failed(format!("failed to report tool availability: {error}"))
            })?;
        if !matches!(discovered, Reply::Accepted) {
            return Err(EngineStartError::Failed(format!(
                "tool availability report was not accepted: {discovered:?}"
            )));
        }
        let current_tools = match report.tools {
            DiscoveredTools::Available(current) => Some(current),
            DiscoveredTools::Missing { .. } => None,
        };
        let recovered = recover_startup(
            &driver.commands,
            current_tools.as_ref().map(|current| &current.media),
            initial.durable,
        );
        let next_runtime_id = next_runtime_id(&recovered)?;
        // Bounded: telemetry can outrun a stalled consumer for hours, and an
        // unbounded buffer would turn that stall into unbounded memory. On
        // overflow the forwarder below severs the stream instead of blocking
        // or dropping individual events — a gap-riddled stream would silently
        // corrupt every downstream fold, while a severed one is observable.
        // The journal, not the stream, holds the truth, so the recovery is a
        // reconnect that folds from a fresh snapshot.
        let (public_event_tx, public_event_rx) = mpsc::sync_channel(PUBLIC_EVENT_CHANNEL_CAPACITY);
        public_event_tx
            .try_send(DriverEvent::Snapshot(AppSnapshot {
                durable: recovered,
                settings: initial.settings,
            }))
            .map_err(|error| {
                EngineStartError::Failed(format!("failed to emit startup snapshot: {error}"))
            })?;
        for pending in driver_events.try_iter() {
            if !matches!(pending, DriverEvent::Durable(_)) {
                public_event_tx.try_send(pending).map_err(|error| {
                    EngineStartError::Failed(format!("failed to emit startup event: {error}"))
                })?;
            }
        }
        let event_forwarder = thread::Builder::new()
            .name("crfty-event-forwarder".to_owned())
            .spawn(move || {
                while let Ok(event) = driver_events.recv() {
                    match public_event_tx.try_send(event) {
                        Ok(()) => {}
                        // The consumer stopped draining. Dropping the sender
                        // keeps the driver unblocked and memory bounded; the
                        // consumer observes the disconnect once it drains the
                        // buffered tail and recovers by reconnecting.
                        Err(mpsc::TrySendError::Full(_)) => {
                            tracing::error!(
                                "public event channel overflowed \
                                 ({PUBLIC_EVENT_CHANNEL_CAPACITY} events buffered, consumer \
                                 not draining); severing the event stream"
                            );
                            break;
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
            })
            .map_err(|error| {
                EngineStartError::Failed(format!("failed to start event bridge: {error}"))
            })?;
        // Written only by the vendor worker on successful activation, which
        // the reducer permits only while the engine is fully idle; sessions
        // snapshot it once at start. That serialization is what makes the
        // shared slot race-free.
        let tools_slot = Arc::new(Mutex::new(current_tools));
        let internal_commands = driver.commands.clone();
        let supervisor_commands = internal_commands.clone();
        let supervisor_runtime = Arc::clone(&runtime);
        let supervisor_tools = Arc::clone(&tools_slot);
        let supervisor = thread::Builder::new()
            .name("crfty-job-supervisor".to_owned())
            .spawn(move || {
                supervise(
                    effect_rx,
                    supervisor_commands,
                    supervisor_runtime,
                    config,
                    supervisor_tools,
                    next_runtime_id,
                );
            })
            .map_err(|error| {
                EngineStartError::Failed(format!("failed to start coordinator: {error}"))
            })?;
        let analysis = crate::analysis::AnalysisRuntime::start(
            internal_commands.clone(),
            Arc::clone(&tools_slot),
        )
        .map_err(|error| {
            EngineStartError::Failed(format!("failed to start Analysis discovery: {error}"))
        })?;
        Ok(Self {
            commands: UserCommandSender {
                inner: internal_commands,
                analysis: Arc::clone(&analysis),
            },
            events: public_event_rx,
            driver: Some(driver),
            supervisor: Some(supervisor),
            event_forwarder: Some(event_forwarder),
            analysis: Some(analysis),
            runtime: Some(runtime),
        })
    }

    pub fn shutdown(mut self) -> Result<(), EngineStartError> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), EngineStartError> {
        if let Some(analysis) = self.analysis.take() {
            analysis.shutdown().map_err(|_| {
                EngineStartError::Failed("Analysis discovery worker panicked".to_owned())
            })?;
        }
        if let Some(driver) = self.driver.take() {
            driver.shutdown().map_err(map_driver_start)?;
        }
        if let Some(supervisor) = self.supervisor.take() {
            supervisor
                .join()
                .map_err(|_| EngineStartError::Failed("job supervisor panicked".to_owned()))?;
        }
        if let Some(forwarder) = self.event_forwarder.take() {
            forwarder
                .join()
                .map_err(|_| EngineStartError::Failed("event forwarder panicked".to_owned()))?;
        }
        if let Some(runtime) = self.runtime.take() {
            let runtime = Arc::try_unwrap(runtime).map_err(|_| {
                EngineStartError::Failed("encoder runtime still has active owners".to_owned())
            })?;
            runtime.shutdown().map_err(|error| {
                EngineStartError::Failed(format!("encoder shutdown failed: {error}"))
            })?;
        }
        Ok(())
    }
}

/// Outcome of a history import: how many records were parked and how many
/// were skipped as duplicates of already-parked or already-adopted paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    pub parked: u32,
    pub skipped: u32,
}

#[derive(Clone)]
pub struct UserCommandSender {
    inner: CommandSender,
    analysis: Arc<crate::analysis::AnalysisRuntime>,
}

impl UserCommandSender {
    pub fn begin_analysis_discovery(
        &self,
        roots: Vec<PathBuf>,
        extensions: BTreeSet<VideoExtension>,
    ) -> Result<AnalysisGenerationId, crate::analysis::AnalysisError> {
        self.analysis.begin(roots, extensions)
    }

    pub fn cancel_analysis(&self) -> Result<(), crate::analysis::AnalysisError> {
        self.analysis.cancel()
    }

    pub fn begin_analysis_basic_scan(
        &self,
        generation: AnalysisGenerationId,
    ) -> Result<(), crate::analysis::AnalysisError> {
        self.analysis.begin_basic_scan(generation)
    }

    pub fn submit_queue(&self, command: QueueCommand) -> Result<Reply, crate::driver::SubmitError> {
        self.inner.submit(Command::Queue(command))
    }

    pub fn submit_session(
        &self,
        command: SessionCommand,
    ) -> Result<Reply, crate::driver::SubmitError> {
        self.inner.submit(Command::Session(command))
    }

    pub fn submit_settings(
        &self,
        command: SettingsCommand,
    ) -> Result<Reply, crate::driver::SubmitError> {
        self.inner.submit(Command::Settings(command))
    }

    pub fn submit_vendor(
        &self,
        command: VendorCommand,
    ) -> Result<Reply, crate::driver::SubmitError> {
        self.inner.submit(Command::Vendor(command))
    }

    pub fn submit_projection(
        &self,
        command: ProjectionCommand,
    ) -> Result<Reply, crate::driver::SubmitError> {
        self.inner.submit(Command::Projection(command))
    }

    /// Read, strictly parse, and submit a history import file (see
    /// `docs/HISTORY_IMPORT.md`). Every failure mode — unreadable or
    /// oversized file, schema rejection, reducer rejection, degraded journal
    /// — surfaces as a user-facing message.
    pub fn import_history(&self, path: &Path) -> Result<ImportSummary, String> {
        let records = crate::history_import::load_import_file(path, now_millis())
            .map_err(|error| error.to_string())?;
        let reply = self
            .inner
            .submit(Command::History(HistoryCommand::Import { records }))
            .map_err(|error| format!("import submission failed: {error}"))?;
        match reply {
            Reply::Imported { parked, skipped } => Ok(ImportSummary { parked, skipped }),
            Reply::Rejected { reason } | Reply::DurabilityUnknown { reason } => Err(reason),
            Reply::Accepted
            | Reply::AnalysisStarted { .. }
            | Reply::BasicScan(_)
            | Reply::Reserved(_)
            | Reply::Claimed(_) => Err("import command returned an invalid reply".to_owned()),
        }
    }

    /// Operator consent to discard the corrupt journal tail identified by
    /// `signature`. The only `System` command a user surface may submit; the
    /// driver intercepts it, so it never reaches the reducer.
    pub fn acknowledge_corruption(
        &self,
        signature: CorruptionSignature,
    ) -> Result<Reply, crate::driver::SubmitError> {
        self.inner
            .submit(Command::System(SystemCommand::AcknowledgeCorruption {
                signature,
            }))
    }
}

impl Drop for EngineRuntime {
    fn drop(&mut self) {
        let _result = self.stop_and_join();
    }
}

fn map_driver_start(error: DriverStartError) -> EngineStartError {
    match error {
        DriverStartError::AlreadyRunning { lock_path } => {
            EngineStartError::AlreadyRunning { lock_path }
        }
        DriverStartError::Failed(message) => {
            EngineStartError::Failed(format!("driver error: {message}"))
        }
    }
}

fn next_runtime_id(state: &DurableState) -> Result<u64, EngineStartError> {
    let maximum = state
        .queue
        .iter()
        .filter_map(|item| match item.state {
            QueueItemState::Reserved { claim_id, run_id }
            | QueueItemState::Claimed { claim_id, run_id }
            | QueueItemState::Running { claim_id, run_id } => Some(claim_id.0.max(run_id.0)),
            QueueItemState::Queued | QueueItemState::Finished(_) => None,
        })
        .chain(state.conversion_runs.keys().map(|run_id| run_id.0))
        .chain(state.outputs.keys().map(|run_id| run_id.0))
        .max()
        .unwrap_or(FIRST_RUNTIME_ID.saturating_sub(1));
    maximum
        .checked_add(1)
        .ok_or_else(|| EngineStartError::Failed("runtime id space is exhausted".to_owned()))
}

fn recover_startup(
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
    match (&transaction.replacement, &transaction.state) {
        (Replacement::KeepOriginal, OutputState::Committed { .. })
        | (Replacement::RetireOriginal, OutputState::Retired { .. }) => {
            match state
                .conversion_runs
                .get(&run_id)
                .map(|run| &run.spec.action)
            {
                Some(JobAction::Remux) => {
                    ItemOutcome::Remuxed(CompletionEvidence::RecoveredAtStartup)
                }
                Some(JobAction::Encode { .. }) => {
                    ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup)
                }
                // Unreachable: the ledger only accepts output transactions
                // for encode and remux runs.
                _ => ItemOutcome::Stopped,
            }
        }
        (_, OutputState::Conflict { .. }) => ItemOutcome::Failed(FailureFacts::new(
            FailureKind::OutputConflict,
            "output transaction settled as a conflict",
        )),
        _ => ItemOutcome::Stopped,
    }
}

#[derive(Clone)]
struct ActiveCancellation {
    state: Arc<Mutex<CancellationState>>,
}

struct CancellationState {
    force_stopping: bool,
    slot: Option<(RunId, ActiveJobCancellation)>,
}

#[derive(Clone)]
enum ActiveJobCancellation {
    AbAv1(CancellationHandle),
    Remux(RemuxCancellationHandle),
}

impl ActiveJobCancellation {
    fn cancel(&self) {
        match self {
            Self::AbAv1(handle) => handle.cancel(CancelMode::Force),
            Self::Remux(handle) => handle.cancel(),
        }
    }
}

impl ActiveCancellation {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(CancellationState {
                force_stopping: false,
                slot: None,
            })),
        }
    }

    fn register(
        &self,
        run_id: RunId,
        handle: ActiveJobCancellation,
    ) -> CancellationRegistration<'_> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.slot = Some((run_id, handle.clone()));
        if state.force_stopping {
            handle.cancel();
        }
        CancellationRegistration {
            cancellation: self,
            run_id,
        }
    }

    fn clear(&self, run_id: RunId) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state
            .slot
            .as_ref()
            .is_some_and(|(active, _)| *active == run_id)
        {
            state.slot = None;
        }
    }

    fn force(&self, run_id: Option<RunId>) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.force_stopping = true;
        if let Some((active, handle)) = state.slot.as_ref()
            && run_id.is_none_or(|expected| expected == *active)
        {
            handle.cancel();
        }
    }

    fn reset(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.force_stopping = false;
        state.slot = None;
    }

    fn is_force_stopping(&self) -> bool {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.force_stopping
    }
}

struct CancellationRegistration<'a> {
    cancellation: &'a ActiveCancellation,
    run_id: RunId,
}

impl Drop for CancellationRegistration<'_> {
    fn drop(&mut self) {
        self.cancellation.clear(self.run_id);
    }
}

fn supervise(
    effects: mpsc::Receiver<Effect>,
    commands: CommandSender,
    runtime: Arc<AbAv1Runtime>,
    config: EngineConfig,
    tools_slot: Arc<Mutex<Option<CurrentTools>>>,
    next_runtime_id: u64,
) {
    let cancellation = ActiveCancellation::new();
    let vendor_cancelled = Arc::new(AtomicBool::new(false));
    let next_id = Arc::new(AtomicU64::new(next_runtime_id));
    let mut worker: Option<thread::JoinHandle<()>> = None;
    let mut vendor: Option<thread::JoinHandle<()>> = None;
    while let Ok(effect) = effects.recv() {
        match effect {
            Effect::StartWorker => {
                if let Some(previous) = worker.take() {
                    // A previous worker that is still winding down (e.g. the
                    // session was force-stopped and restarted immediately)
                    // would otherwise block this join for as long as its
                    // current job keeps running. Force-cancel it first, the
                    // same way StopDriver does; reset() below clears the
                    // latch before the new worker starts.
                    cancellation.force(None);
                    if previous.join().is_err() {
                        report_worker_crash(&commands, "previous session worker panicked");
                        break;
                    }
                }
                cancellation.reset();
                let worker_commands = commands.clone();
                let worker_runtime = Arc::clone(&runtime);
                let worker_config = config.clone();
                let worker_tools = Arc::clone(&tools_slot);
                let worker_cancellation = cancellation.clone();
                let worker_ids = Arc::clone(&next_id);
                let spawned = thread::Builder::new()
                    .name("crfty-session-worker".to_owned())
                    .spawn(move || {
                        // Held for the whole session and released on every
                        // exit path, including caught panics: the guard sits
                        // outside catch_unwind on this thread's stack.
                        let _sleep = crate::power::inhibit_sleep();
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            run_session(
                                &worker_commands,
                                &worker_runtime,
                                &worker_config,
                                &worker_tools,
                                &worker_cancellation,
                                &worker_ids,
                            )
                        }));
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(message)) if !worker_cancellation.is_force_stopping() => {
                                report_worker_crash(&worker_commands, &message);
                            }
                            Ok(Err(_)) => {}
                            Err(_) => {
                                report_worker_crash(&worker_commands, "session worker panicked");
                            }
                        }
                    });
                match spawned {
                    Ok(handle) => worker = Some(handle),
                    Err(error) => {
                        report_worker_crash(
                            &commands,
                            &format!("failed to spawn session worker: {error}"),
                        );
                        break;
                    }
                }
            }
            Effect::KillActiveRun { run_id } => cancellation.force(Some(run_id)),
            // The reducer serializes vendor work through the activity state,
            // so at most one runs; status flows back as commands, and the
            // handle is kept so shutdown can join it with a bounded wait.
            Effect::VendorInstall => {
                reap_finished_vendor(&mut vendor);
                vendor = spawn_vendor_worker(
                    &commands,
                    &config,
                    &tools_slot,
                    &vendor_cancelled,
                    VendorTask::Install,
                );
            }
            Effect::VendorCheck => {
                reap_finished_vendor(&mut vendor);
                vendor = spawn_vendor_worker(
                    &commands,
                    &config,
                    &tools_slot,
                    &vendor_cancelled,
                    VendorTask::Check,
                );
            }
            Effect::WriteSettings { .. } => {
                report_worker_crash(
                    &commands,
                    "driver leaked a settings effect to the supervisor",
                );
                break;
            }
            Effect::StopDriver => {
                cancellation.force(None);
                // Flag first so a mid-download vendor worker starts unwinding
                // through its own cleanup while the session worker is joined;
                // the bounded join below reclaims it.
                vendor_cancelled.store(true, Ordering::Relaxed);
                break;
            }
        }
    }
    // Reached on StopDriver (both flags already set) or when the driver died
    // and the effect channel disconnected. Force-flag both workers again so
    // the joins below are winding-down waits, never an hours-long encode.
    cancellation.force(None);
    vendor_cancelled.store(true, Ordering::Relaxed);
    if let Some(worker) = worker
        && worker.join().is_err()
    {
        report_worker_crash(&commands, "session worker panicked during shutdown");
    }
    if let Some(vendor) = vendor
        && !join_within(vendor, VENDOR_SHUTDOWN_WAIT, VENDOR_SHUTDOWN_POLL)
    {
        tracing::warn!(
            "vendor worker still running after {VENDOR_SHUTDOWN_WAIT:?}; abandoning it to \
             process exit"
        );
    }
}

/// The reducer only schedules new vendor work after the previous worker
/// reported a terminal activity — its last act before exiting — so this join
/// reclaims a thread that is already unwinding.
fn reap_finished_vendor(vendor: &mut Option<thread::JoinHandle<()>>) {
    if let Some(previous) = vendor.take()
        && previous.join().is_err()
    {
        tracing::error!("previous vendor worker panicked");
    }
}

/// Bounded join for shutdown: std has no timed join, so completion is polled.
/// Returns false when the thread outlived `wait` and was left detached —
/// blocking shutdown on a wedged network read would be worse than abandoning
/// the thread to process exit.
fn join_within(handle: thread::JoinHandle<()>, wait: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(poll);
    }
    if handle.join().is_err() {
        tracing::error!("thread panicked while being joined during shutdown");
    }
    true
}

fn report_worker_crash(commands: &CommandSender, message: &str) {
    match commands.submit(Command::Worker(WorkerCommand::Crashed {
        message: message.to_owned(),
    })) {
        Ok(Reply::Accepted) => {}
        Ok(reply) => tracing::error!("failed to report worker crash ({message}): {reply:?}"),
        Err(error) => tracing::error!("failed to report worker crash ({message}): {error}"),
    }
}

#[derive(Clone, Copy)]
enum VendorTask {
    Install,
    Check,
}

/// Download progress is re-reported only every this many bytes so the
/// ordered stream carries a handful of updates per archive, not one per
/// 128 KiB chunk.
const VENDOR_PROGRESS_STEP_BYTES: u64 = 8 * 1024 * 1024;

fn spawn_vendor_worker(
    commands: &CommandSender,
    config: &EngineConfig,
    tools_slot: &Arc<Mutex<Option<CurrentTools>>>,
    cancelled: &Arc<AtomicBool>,
    task: VendorTask,
) -> Option<thread::JoinHandle<()>> {
    cancelled.store(false, Ordering::Relaxed);
    let worker_commands = commands.clone();
    let worker_config = config.clone();
    let worker_tools = Arc::clone(tools_slot);
    let worker_cancelled = Arc::clone(cancelled);
    let spawned = thread::Builder::new()
        .name("crfty-vendor-worker".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_vendor_task(
                    &worker_commands,
                    &worker_config,
                    &worker_tools,
                    &worker_cancelled,
                    task,
                );
            }));
            if result.is_err() {
                submit_vendor_activity(
                    &worker_commands,
                    VendorActivity::Failed {
                        detail: "the vendor worker panicked".to_owned(),
                    },
                );
            }
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(error) => {
            submit_vendor_activity(
                commands,
                VendorActivity::Failed {
                    detail: format!("failed to start the vendor worker: {error}"),
                },
            );
            None
        }
    }
}

fn run_vendor_task(
    commands: &CommandSender,
    config: &EngineConfig,
    tools_slot: &Mutex<Option<CurrentTools>>,
    cancelled: &AtomicBool,
    task: VendorTask,
) {
    match task {
        VendorTask::Install => match run_vendor_install(commands, config, cancelled) {
            Ok(()) => {
                refresh_discovered_tools(commands, config, tools_slot);
                submit_vendor_activity(commands, VendorActivity::Idle);
            }
            // A cancelled install is not a failure: the previous tools are
            // untouched, so the activity simply returns to rest.
            Err(InstallError::Cancelled) => submit_vendor_activity(commands, VendorActivity::Idle),
            Err(InstallError::Failed(detail)) => {
                submit_vendor_activity(commands, VendorActivity::Failed { detail });
            }
        },
        VendorTask::Check => {
            refresh_discovered_tools(commands, config, tools_slot);
            submit_vendor_activity(commands, VendorActivity::Idle);
        }
    }
}

fn run_vendor_install(
    commands: &CommandSender,
    config: &EngineConfig,
    cancelled: &AtomicBool,
) -> Result<(), InstallError> {
    if matches!(config.tools, ToolsConfig::Fixed(_)) {
        return Err(InstallError::Failed(
            "this engine instance runs with a fixed tool set".to_owned(),
        ));
    }
    let Some(manifest) = manifest::current() else {
        return Err(InstallError::Failed(
            "no pinned FFmpeg build exists for this platform".to_owned(),
        ));
    };
    let fetch = HttpFetch::new().map_err(InstallError::Failed)?;
    let mut last_reported: u64 = 0;
    let mut progress = |event: InstallProgress| match event {
        InstallProgress::Downloading { received, total } => {
            let complete = total.is_some_and(|total| received >= total);
            if received.saturating_sub(last_reported) >= VENDOR_PROGRESS_STEP_BYTES || complete {
                last_reported = received;
                submit_vendor_activity(commands, VendorActivity::Downloading { received, total });
            }
        }
        InstallProgress::Installing => {
            submit_vendor_activity(commands, VendorActivity::Installing);
        }
    };
    vendor_install::install(
        &config.vendor_root,
        manifest,
        &fetch,
        &mut progress,
        cancelled,
    )
    .map(|_metadata| ())
}

/// Re-runs discovery, publishes the result, and updates the shared slot the
/// next session snapshots. Discovery reads the freshly written
/// `current.json`, so a successful install becomes active tools without
/// trusting anything but the on-disk record — and explicit env overrides
/// still win.
fn refresh_discovered_tools(
    commands: &CommandSender,
    config: &EngineConfig,
    tools_slot: &Mutex<Option<CurrentTools>>,
) {
    let report = match &config.tools {
        ToolsConfig::Discover => discovery::discover(&config.vendor_root),
        ToolsConfig::Fixed(tools) => DiscoveryReport {
            tools: tools.clone(),
            update_available: false,
        },
    };
    let current = match &report.tools {
        DiscoveredTools::Available(current) => Some(current.clone()),
        DiscoveredTools::Missing { .. } => None,
    };
    {
        let mut slot = match tools_slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = current;
    }
    match commands.submit(Command::System(SystemCommand::ToolsDiscovered {
        availability: report.tools.availability(),
        update_available: report.update_available,
    })) {
        Ok(Reply::Accepted) => {}
        Ok(reply) => tracing::warn!("tool rediscovery report was not accepted: {reply:?}"),
        Err(error) => tracing::warn!("failed to report rediscovered tools: {error}"),
    }
}

fn submit_vendor_activity(commands: &CommandSender, activity: VendorActivity) {
    match commands.submit(Command::System(SystemCommand::VendorProgress { activity })) {
        Ok(Reply::Accepted) => {}
        Ok(reply) => tracing::warn!("vendor progress report was not accepted: {reply:?}"),
        Err(error) => tracing::warn!("failed to report vendor progress: {error}"),
    }
}

fn run_session(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    config: &EngineConfig,
    tools_slot: &Mutex<Option<CurrentTools>>,
    cancellation: &ActiveCancellation,
    ids: &AtomicU64,
) -> Result<(), String> {
    // Snapshot the slot once: every claim in this session executes with the
    // same binaries and revisions, and the reducer refuses vendor installs
    // while a session runs, so the snapshot cannot go stale mid-session.
    let current = {
        let slot = match tools_slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.clone()
    };
    let Some(current) = current else {
        // Unreachable past the reducer's session-start gate; finish the
        // session gracefully rather than reporting a worker crash.
        return require_accepted(
            "finish tool-less worker session",
            commands.submit(Command::Worker(WorkerCommand::Finished)),
        );
    };
    let tools = &current.media;
    let inspector = MediaInspector::new(tools.ffprobe.clone());
    let mut decoder_resolver = DecodeResolver::new(tools.ffmpeg.clone());
    // Claim-time revision immutability: the composed revisions freeze into
    // each JobSpec at PrepareReserved and survive any later tool swap.
    let mut base_execution = config.execution.clone();
    base_execution.profile.ab_av1_revision = current.revisions.ab_av1.clone();
    base_execution.profile.ffmpeg_revision = current.revisions.ffmpeg.clone();
    base_execution.profile.encoder_revision = current.revisions.encoder.clone();
    loop {
        let claim_id = ClaimId(ids.fetch_add(1, Ordering::Relaxed));
        let run_id = RunId(ids.fetch_add(1, Ordering::Relaxed));
        let reservation = commands.submit(Command::Worker(WorkerCommand::ReserveNext {
            claim_id,
            run_id,
        }));
        let reserved = match reservation {
            Ok(Reply::Reserved(Some(job))) => job,
            Ok(Reply::Reserved(None) | Reply::Rejected { .. }) => break,
            Ok(Reply::DurabilityUnknown { reason }) => return Err(reason),
            Err(error) => return Err(format!("worker reservation failed: {error}")),
            Ok(
                Reply::Accepted
                | Reply::AnalysisStarted { .. }
                | Reply::BasicScan(_)
                | Reply::Claimed(_)
                | Reply::Imported { .. },
            ) => {
                return Err("reservation command returned an invalid reply".to_owned());
            }
        };
        let observation = match inspector.observe(&reserved.input) {
            Ok(observation) => Some(Box::new(observation)),
            Err(error) => {
                tracing::warn!(
                    "media preflight failed; continuing without reusable facts: {error}"
                );
                None
            }
        };
        let input_duration_ms = observation
            .as_ref()
            .map(|observed| observed.metadata.duration_ms)
            .filter(|duration| *duration > 0);
        let mut execution = base_execution.clone();
        execution.profile.decode_mode =
            observation
                .as_ref()
                .map_or(crfty_core::DecodeMode::Software, |observed| {
                    decoder_resolver.resolve(execution.decode_preference, &observed.metadata.codec)
                });
        // The observed file's normalized spellings, matched against the
        // parked import inbox by the reducer during preparation.
        let import_paths = crate::history_import::import_path_candidates(&reserved.input);
        let prepared = commands.submit(Command::Worker(WorkerCommand::PrepareReserved {
            item_id: reserved.item_id,
            claim_id,
            run_id,
            observation,
            import_paths,
            execution,
        }));
        let job = match prepared {
            Ok(Reply::Claimed(Some(job))) => job,
            Ok(Reply::Rejected { reason } | Reply::DurabilityUnknown { reason }) => {
                let _reply = commands.submit(Command::Worker(WorkerCommand::AbandonReservation {
                    item_id: reserved.item_id,
                    claim_id,
                    run_id,
                    at: now_millis(),
                }));
                return Err(reason);
            }
            Err(error) => {
                let _reply = commands.submit(Command::Worker(WorkerCommand::AbandonReservation {
                    item_id: reserved.item_id,
                    claim_id,
                    run_id,
                    at: now_millis(),
                }));
                return Err(format!("worker preparation failed: {error}"));
            }
            Ok(
                Reply::Accepted
                | Reply::AnalysisStarted { .. }
                | Reply::BasicScan(_)
                | Reply::Claimed(None)
                | Reply::Reserved(_)
                | Reply::Imported { .. },
            ) => {
                return Err("preparation command returned an invalid reply".to_owned());
            }
        };
        require_accepted(
            "mark worker item started",
            commands.submit(Command::Worker(WorkerCommand::Started {
                item_id: job.spec.item_id,
                claim_id,
                run_id,
                at: now_millis(),
            })),
        )?;
        process_job(
            commands,
            runtime,
            tools,
            cancellation,
            &job,
            input_duration_ms,
        )?;
    }
    require_accepted(
        "finish worker session",
        commands.submit(Command::Worker(WorkerCommand::Finished)),
    )
}

fn process_job(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    cancellation: &ActiveCancellation,
    job: &ClaimedJob,
    input_duration_ms: Option<u64>,
) -> Result<(), String> {
    let mut tracker = PhaseTracker::new();
    let services = JobServices {
        commands,
        runtime,
        tools,
        cancellation,
        input_duration_ms,
    };
    publish_phase(commands, job.spec.run_id, &mut tracker, JobPhase::Preparing);
    match &job.spec.action {
        JobAction::Skip { reason } => {
            terminal(
                commands,
                job,
                &mut tracker,
                ItemOutcome::Skipped {
                    reason: reason.clone(),
                },
                None,
            )?;
            return Ok(());
        }
        JobAction::Remux => {
            let Some(destination) = resolve_output_destination(commands, job, &mut tracker)? else {
                return Ok(());
            };
            let Some(output) = begin_output(commands, tools, job, destination, &mut tracker)?
            else {
                return Ok(());
            };
            return run_remux(services, job, output, &mut tracker);
        }
        JobAction::Analyze { .. } | JobAction::Encode { .. } => {}
    }

    let destination = if matches!(job.spec.action, JobAction::Encode { .. }) {
        let Some(destination) = resolve_output_destination(commands, job, &mut tracker)? else {
            return Ok(());
        };
        Some(destination)
    } else {
        None
    };
    let analysis = if let Some(selected) = job.spec.action.selected_analysis() {
        selected.clone()
    } else {
        let searched =
            match search_with_fallback(commands, runtime, tools, cancellation, job, &mut tracker) {
                Ok(result) => result,
                Err(outcome) => {
                    terminal(commands, job, &mut tracker, outcome, None)?;
                    return Ok(());
                }
            };
        require_accepted(
            "record analysis",
            commands.submit(Command::Worker(WorkerCommand::RecordAnalysis {
                item_id: job.spec.item_id,
                claim_id: job.spec.claim_id,
                run_id: job.spec.run_id,
                result: Box::new(searched.clone()),
            })),
        )?;
        searched
    };
    if matches!(job.spec.action, JobAction::Analyze { .. }) {
        publish_phase(
            commands,
            job.spec.run_id,
            &mut tracker,
            JobPhase::Finalizing,
        );
        terminal(commands, job, &mut tracker, ItemOutcome::Analyzed, None)?;
        return Ok(());
    }

    let Some(destination) = destination else {
        return Err("encode job has no resolved output destination".to_owned());
    };
    let Some(output) = begin_output(commands, tools, job, destination, &mut tracker)? else {
        return Ok(());
    };
    run_encode(services, job, output, analysis, &mut tracker)
}

fn run_encode(
    services: JobServices<'_>,
    job: &ClaimedJob,
    output: PreparedOutput,
    analysis: AnalysisResult,
    tracker: &mut PhaseTracker,
) -> Result<(), String> {
    let PreparedOutput {
        manager,
        mut transaction,
    } = output;
    let mut decode_mode = analysis.profile.decode_mode;
    loop {
        let request = EncodeRequest {
            input: job.spec.input.clone(),
            output: transaction.staging.clone(),
            crf: crf_to_f32(analysis.measurement.crf),
            preset: analysis.profile.preset,
            decode_mode,
        };
        let handle = match services
            .runtime
            .start_encode(services.tools.clone(), request)
        {
            Ok(handle) => handle,
            Err(error) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(FailureFacts::new(
                        FailureKind::EncodeStart,
                        error.to_string(),
                    )),
                    None,
                    tracker,
                );
            }
        };
        let report = wait_for_report(
            services.commands,
            job.spec.run_id,
            handle,
            services.cancellation,
            JobPhase::Encoding,
            tracker,
            services.input_duration_ms.map(|duration| duration as f64),
        );
        match report {
            Ok(JobReport {
                terminal: JobTerminal::Completed(outcome),
                final_telemetry,
            }) => {
                return finish_successful_output(
                    services.commands,
                    job,
                    manager,
                    transaction,
                    SuccessfulJob::Encode {
                        outcome,
                        decode_mode,
                    },
                    final_telemetry.as_ref().map(telemetry_progress),
                    tracker,
                );
            }
            Ok(JobReport {
                terminal: JobTerminal::Cancelled,
                final_telemetry,
            }) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Stopped,
                    map_progress(
                        job.spec.run_id,
                        tracker,
                        JobPhase::Encoding,
                        final_telemetry.as_ref().map(telemetry_progress),
                    ),
                    tracker,
                );
            }
            Ok(JobReport {
                terminal: JobTerminal::Failed(failure),
                final_telemetry,
            }) => {
                // Hardware→software retry: hook BEFORE any abandonment so the
                // still-unsettled transaction is reused. The failed attempt's
                // adapter cleanup deleted the staging file, so restaging moves
                // the journaled pin to a recreated one; once a transaction is
                // abandoned the ledger refuses to restage, which is what makes
                // retry-after-abandonment unrepresentable. The requested
                // JobSpec is never rewritten — the divergence is recorded in
                // the terminal evidence's `encode_decode`.
                if matches!(decode_mode, DecodeMode::Hardware(_)) {
                    match restage_for_retry(&manager, services.commands, &mut transaction) {
                        Ok(()) => {
                            tracing::warn!(
                                "hardware-decode encode failed ({}); retrying once with software decode",
                                failure.message
                            );
                            decode_mode = DecodeMode::Software;
                            continue;
                        }
                        Err(restage_error) => {
                            tracing::warn!(
                                "staging could not be recreated for the software retry: {restage_error}"
                            );
                        }
                    }
                }
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(FailureFacts::new(FailureKind::EncodeRun, failure.message)),
                    map_progress(
                        job.spec.run_id,
                        tracker,
                        JobPhase::Encoding,
                        final_telemetry.as_ref().map(telemetry_progress),
                    ),
                    tracker,
                );
            }
            Ok(JobReport {
                terminal: JobTerminal::Panicked { cleanup_failure },
                final_telemetry,
            }) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(panicked_facts(
                        "encode adapter panicked",
                        cleanup_failure,
                        job,
                        &transaction,
                    )),
                    map_progress(
                        job.spec.run_id,
                        tracker,
                        JobPhase::Encoding,
                        final_telemetry.as_ref().map(telemetry_progress),
                    ),
                    tracker,
                );
            }
            Err(message) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, message)),
                    None,
                    tracker,
                );
            }
        }
    }
}

/// Moves the journaled staging pin to a freshly recreated empty staging file
/// so a software-decode retry can reuse the still-unsettled transaction: the
/// recreated identity is journaled as a repeated `StagingCreated`.
fn restage_for_retry(
    manager: &MediaOutputManager,
    commands: &CommandSender,
    transaction: &mut OutputTransaction,
) -> Result<(), String> {
    let initial = manager
        .restage(transaction)
        .map_err(|error| error.to_string())?;
    let created = OutputDelta::StagingCreated {
        run_id: transaction.run_id,
        initial,
    };
    submit_output(commands, created.clone())?;
    fold_transaction(transaction, created);
    Ok(())
}

/// Facts for an adapter panic: the cleanup error, if any, may embed run paths,
/// so it travels as a scrubbed diagnostic rather than message prose.
fn panicked_facts(
    message: &str,
    cleanup_failure: Option<String>,
    job: &ClaimedJob,
    transaction: &OutputTransaction,
) -> FailureFacts {
    let facts = FailureFacts::new(
        FailureKind::AdapterPanicked {
            cleanup_failed: cleanup_failure.is_some(),
        },
        message,
    );
    match cleanup_failure {
        Some(cleanup) => facts.with_diagnostic(scrub_tail(
            &cleanup,
            &[
                (job.spec.input.as_path(), "<input>"),
                (transaction.staging.as_path(), "<staging>"),
                (transaction.final_path.as_path(), "<output>"),
            ],
        )),
        None => facts,
    }
}

fn run_remux(
    services: JobServices<'_>,
    job: &ClaimedJob,
    output: PreparedOutput,
    tracker: &mut PhaseTracker,
) -> Result<(), String> {
    let PreparedOutput {
        manager,
        transaction,
    } = output;
    let handle = match remux::start(RemuxRequest {
        ffmpeg: services.tools.ffmpeg.clone(),
        input: job.spec.input.clone(),
        output: transaction.staging.clone(),
    }) {
        Ok(handle) => handle,
        Err(error) => {
            abandon_output(
                &manager,
                services.commands,
                job,
                &transaction,
                ItemOutcome::Failed(FailureFacts::new(
                    FailureKind::RemuxStart,
                    error.to_string(),
                )),
                None,
                tracker,
            )?;
            return Ok(());
        }
    };
    let report = wait_for_remux_report(
        services.commands,
        job.spec.run_id,
        handle,
        services.cancellation,
        tracker,
        services.input_duration_ms.map(|duration| duration as f64),
    );
    match report {
        Ok(RemuxReport {
            terminal: RemuxTerminal::Completed(_),
            final_telemetry,
        }) => finish_successful_output(
            services.commands,
            job,
            manager,
            transaction,
            SuccessfulJob::Remux,
            final_telemetry.map(remux_progress),
            tracker,
        )?,
        Ok(RemuxReport {
            terminal: RemuxTerminal::Cancelled,
            final_telemetry,
        }) => abandon_output(
            &manager,
            services.commands,
            job,
            &transaction,
            ItemOutcome::Stopped,
            map_progress(
                job.spec.run_id,
                tracker,
                JobPhase::Remuxing,
                final_telemetry.map(remux_progress),
            ),
            tracker,
        )?,
        Ok(RemuxReport {
            terminal: RemuxTerminal::Failed(failure),
            final_telemetry,
        }) => {
            let facts = FailureFacts::new(FailureKind::RemuxRun, failure.message).with_diagnostic(
                scrub_tail(
                    failure.stderr_tail.trim(),
                    &[
                        (job.spec.input.as_path(), "<input>"),
                        (transaction.staging.as_path(), "<staging>"),
                        (transaction.final_path.as_path(), "<output>"),
                    ],
                ),
            );
            abandon_output(
                &manager,
                services.commands,
                job,
                &transaction,
                ItemOutcome::Failed(facts),
                map_progress(
                    job.spec.run_id,
                    tracker,
                    JobPhase::Remuxing,
                    final_telemetry.map(remux_progress),
                ),
                tracker,
            )?;
        }
        Err(message) => abandon_output(
            &manager,
            services.commands,
            job,
            &transaction,
            ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, message)),
            None,
            tracker,
        )?,
    }
    Ok(())
}

fn abandon_output(
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

fn resolve_output_destination(
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

fn begin_output(
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

fn finish_successful_output(
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
    match (&transaction.replacement, &transaction.state) {
        (Replacement::KeepOriginal, OutputState::Committed { final_identity })
        | (Replacement::RetireOriginal, OutputState::Retired { final_identity }) => match success {
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
        },
        (_, OutputState::Conflict { .. }) => ItemOutcome::Failed(FailureFacts::new(
            FailureKind::OutputConflict,
            "output transaction settled as a conflict",
        )),
        _ => ItemOutcome::Stopped,
    }
}

fn search_with_fallback(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    cancellation: &ActiveCancellation,
    job: &ClaimedJob,
    tracker: &mut PhaseTracker,
) -> Result<AnalysisResult, ItemOutcome> {
    let execution = &job.spec.execution;
    let mut profile = execution.profile.clone();
    let mut target = execution.requested_target.0;
    let mut failed_attempts = Vec::new();
    loop {
        let request = SearchRequest {
            input: job.spec.input.clone(),
            target_vmaf: f32::from(target),
            max_encoded_percent: profile.max_encoded_percent_basis_points as f32
                / PERCENT_BASIS_POINTS_SCALE as f32,
            preset: profile.preset,
            samples: profile.samples,
            sample_duration: Duration::from_millis(profile.sample_duration_ms),
            thorough: profile.thorough,
            decode_mode: profile.decode_mode,
        };
        let handle = runtime
            .start_search(tools.clone(), request)
            .map_err(|error| {
                ItemOutcome::Failed(FailureFacts::new(
                    FailureKind::SearchStart,
                    error.to_string(),
                ))
            })?;
        let report = wait_for_report(
            commands,
            job.spec.run_id,
            handle,
            cancellation,
            JobPhase::Analyzing,
            tracker,
            Some(f64::from(NORMALIZED_PROGRESS_MAX)),
        )
        .map_err(|message| {
            ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, message))
        })?;
        match report.terminal {
            JobTerminal::Completed(outcome) => {
                let measured = measurement(outcome).map_err(|message| {
                    ItemOutcome::Failed(FailureFacts::new(FailureKind::SearchRun, message))
                })?;
                return Ok(AnalysisResult {
                    requested_target: execution.requested_target,
                    successful_target: VmafTarget(target),
                    fallback_floor: execution.fallback_floor,
                    fallback_step: execution.fallback_step,
                    failed_attempts,
                    measurement: measured,
                    profile: profile.clone(),
                });
            }
            JobTerminal::Failed(failure) => match failure.kind {
                JobFailureKind::NoGoodCrf { last } => {
                    let measured = measurement(last).map_err(|message| {
                        ItemOutcome::Failed(FailureFacts::new(FailureKind::SearchRun, message))
                    })?;
                    failed_attempts.push(AnalysisAttempt {
                        target: VmafTarget(target),
                        last_measurement: Some(measured),
                    });
                }
                JobFailureKind::Other => {
                    // Hardware→software retry (parse-free trigger): any
                    // non-NoGoodCrf search failure under hardware decode
                    // restarts the whole VMAF ladder once with the software
                    // profile. Attempts measured under hardware are discarded
                    // so the recorded result is honest about the profile it
                    // ran with; the widened `permitted_profiles` gate accepts
                    // the divergent profile while the JobSpec stays as
                    // requested.
                    if matches!(profile.decode_mode, DecodeMode::Hardware(_)) {
                        tracing::warn!(
                            "hardware-decode search failed ({}); retrying with software decode",
                            failure.message
                        );
                        profile.decode_mode = DecodeMode::Software;
                        target = execution.requested_target.0;
                        failed_attempts.clear();
                        continue;
                    }
                    return Err(ItemOutcome::Failed(FailureFacts::new(
                        FailureKind::SearchRun,
                        failure.message,
                    )));
                }
            },
            JobTerminal::Cancelled => return Err(ItemOutcome::Stopped),
            JobTerminal::Panicked { cleanup_failure } => {
                let facts = FailureFacts::new(
                    FailureKind::AdapterPanicked {
                        cleanup_failed: cleanup_failure.is_some(),
                    },
                    "analysis adapter panicked",
                );
                let facts = match cleanup_failure {
                    Some(cleanup) => facts.with_diagnostic(scrub_tail(
                        &cleanup,
                        &[(job.spec.input.as_path(), "<input>")],
                    )),
                    None => facts,
                };
                return Err(ItemOutcome::Failed(facts));
            }
        }
        if target <= execution.fallback_floor.0
            || execution.fallback_step == 0
            || target.saturating_sub(execution.fallback_step) < execution.fallback_floor.0
        {
            return Err(ItemOutcome::NotWorthwhile {
                attempts: failed_attempts,
            });
        }
        target = target.saturating_sub(execution.fallback_step);
    }
}

fn wait_for_report<T>(
    commands: &CommandSender,
    run_id: RunId,
    mut handle: JobHandle<T>,
    cancellation: &ActiveCancellation,
    phase: JobPhase,
    tracker: &mut PhaseTracker,
    total_work: Option<f64>,
) -> Result<JobReport<T>, String> {
    tracker.enter(phase);
    let _registration = cancellation.register(
        run_id,
        ActiveJobCancellation::AbAv1(handle.cancellation_handle()),
    );
    let started = Instant::now();
    let mut rates = RateTracker::new(total_work);
    let mut last_update = None;
    let mut last_published = None;
    loop {
        match handle
            .recv_report(ADAPTER_REPORT_POLL_INTERVAL)
            .map_err(|error| error.to_string())?
        {
            Some(report) => {
                return Ok(report);
            }
            None => {
                // Only a changed adapter value becomes a rate sample: the
                // poll re-reads the latest value every interval, and feeding
                // repeats would drag the window's slope down between the
                // adapter's real updates.
                if let Some(update) = handle.latest_telemetry()
                    && last_update.as_ref() != Some(&update)
                {
                    let elapsed = started.elapsed();
                    rates.record(elapsed, &rate_sample(&update));
                    let progress = telemetry_progress(&update);
                    last_update = Some(update);
                    let published = (progress, rates.fps_centi(), rates.eta_ms(elapsed));
                    if last_published.as_ref() != Some(&published) {
                        commands.publish_telemetry(Telemetry {
                            run_id,
                            sequence: tracker.next_sequence(),
                            phase,
                            progress: published.0.clone(),
                            fps_centi: published.1,
                            eta_ms: published.2,
                        });
                        last_published = Some(published);
                    }
                }
            }
        }
    }
}

fn wait_for_remux_report(
    commands: &CommandSender,
    run_id: RunId,
    mut handle: RemuxHandle,
    cancellation: &ActiveCancellation,
    tracker: &mut PhaseTracker,
    total_work: Option<f64>,
) -> Result<RemuxReport, String> {
    tracker.enter(JobPhase::Remuxing);
    let _registration = cancellation.register(
        run_id,
        ActiveJobCancellation::Remux(handle.cancellation_handle()),
    );
    let started = Instant::now();
    let mut rates = RateTracker::new(total_work);
    let mut last_update = None;
    let mut last_published = None;
    loop {
        match handle
            .recv_report(ADAPTER_REPORT_POLL_INTERVAL)
            .map_err(|error| error.to_string())?
        {
            Some(report) => return Ok(report),
            None => {
                if let Some(update) = handle.latest_telemetry()
                    && last_update != Some(update)
                {
                    let elapsed = started.elapsed();
                    // A remux reports no frame rate; only the position feeds
                    // the window, so fps stays absent and the ETA works.
                    rates.record(
                        elapsed,
                        &RateSample {
                            frames: None,
                            fps_gauge: None,
                            work_done: update.position_ms as f64,
                        },
                    );
                    last_update = Some(update);
                    let published = (
                        remux_progress(update),
                        rates.fps_centi(),
                        rates.eta_ms(elapsed),
                    );
                    if last_published.as_ref() != Some(&published) {
                        commands.publish_telemetry(Telemetry {
                            run_id,
                            sequence: tracker.next_sequence(),
                            phase: JobPhase::Remuxing,
                            progress: published.0.clone(),
                            fps_centi: published.1,
                            eta_ms: published.2,
                        });
                        last_published = Some(published);
                    }
                }
            }
        }
    }
}

/// Normalizes one adapter update for the rate window: search progress runs
/// toward [`NORMALIZED_PROGRESS_MAX`] with a reported fps gauge, an encode's
/// output position runs toward the input duration with a frame counter (the
/// reported gauge covers the window's first sample).
fn rate_sample(update: &AdapterTelemetry) -> RateSample {
    match update {
        AdapterTelemetry::Search(search) => RateSample {
            frames: None,
            fps_gauge: Some(search.fps),
            work_done: f64::from(
                search
                    .progress
                    .clamp(NORMALIZED_PROGRESS_MIN, NORMALIZED_PROGRESS_MAX),
            ),
        },
        AdapterTelemetry::Encode(encode) => RateSample {
            frames: Some(encode.frame),
            fps_gauge: Some(encode.fps),
            work_done: encode.position.as_millis() as f64,
        },
    }
}

fn telemetry_progress(telemetry: &AdapterTelemetry) -> JobProgress {
    match telemetry {
        AdapterTelemetry::Search(search) => JobProgress::SearchBasisPoints(
            (search
                .progress
                .clamp(NORMALIZED_PROGRESS_MIN, NORMALIZED_PROGRESS_MAX)
                * MAX_PERCENT_BASIS_POINTS as f32) as u32,
        ),
        AdapterTelemetry::Encode(encode) => JobProgress::OutputPositionMs(
            encode.position.as_millis().try_into().unwrap_or(u64::MAX),
        ),
    }
}

fn remux_progress(telemetry: RemuxTelemetry) -> JobProgress {
    JobProgress::OutputPositionMs(telemetry.position_ms)
}

fn measurement(outcome: SearchOutcome) -> Result<SearchMeasurement, String> {
    if !outcome.crf.is_finite()
        || outcome.crf < 0.0
        || !outcome.vmaf.is_finite()
        || !(0.0..=f32::from(MAX_VMAF_SCORE)).contains(&outcome.vmaf)
        || !outcome.predicted_percent.is_finite()
        || outcome.predicted_percent < 0.0
    {
        return Err("ab-av1 returned a non-finite or out-of-range analysis value".to_owned());
    }
    let scaled_crf = outcome.crf * CRF_FIXED_SCALE as f32;
    let scaled_percent = outcome.predicted_percent * f64::from(PERCENT_BASIS_POINTS_SCALE);
    if scaled_crf > u32::MAX as f32 || scaled_percent > f64::from(u32::MAX) {
        return Err("ab-av1 returned an analysis value too large to persist".to_owned());
    }
    Ok(SearchMeasurement {
        crf: Crf((outcome.crf * CRF_FIXED_SCALE as f32).round().max(0.0) as u32),
        score: VmafScore(
            (outcome.vmaf * f32::from(VMAF_SCORE_FIXED_SCALE))
                .round()
                .clamp(
                    0.0,
                    f32::from(VMAF_SCORE_FIXED_SCALE) * f32::from(MAX_VMAF_SCORE),
                ) as u16,
        ),
        predicted_size: outcome.predicted_size,
        predicted_percent_basis_points: scaled_percent.round() as u32,
        predicted_duration_ms: outcome.predicted_duration.as_millis() as u64,
        from_cache: outcome.from_cache,
    })
}

fn crf_to_f32(crf: Crf) -> f32 {
    crf.0 as f32 / CRF_FIXED_SCALE as f32
}

fn resolve_output(job: &ClaimedJob) -> Result<(PathBuf, crfty_core::Replacement), String> {
    resolve_output_for(&job.spec.input, &job.spec.output_target)
}

fn resolve_output_for(
    input: &std::path::Path,
    output_target: &OutputTarget,
) -> Result<(PathBuf, crfty_core::Replacement), String> {
    output_target.validate().map_err(str::to_owned)?;
    let stem = input
        .file_stem()
        .ok_or_else(|| "input has no file stem".to_owned())?;
    match output_target {
        OutputTarget::Replace => {
            let is_matroska = input.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case(OUTPUT_CONTAINER_EXTENSION)
            });
            Ok((
                if is_matroska {
                    input.to_path_buf()
                } else {
                    input.with_extension(OUTPUT_CONTAINER_EXTENSION)
                },
                if is_matroska {
                    crfty_core::Replacement::KeepOriginal
                } else {
                    crfty_core::Replacement::RetireOriginal
                },
            ))
        }
        OutputTarget::Suffix { suffix } => {
            let mut name = stem.to_os_string();
            name.push(suffix);
            name.push(".");
            name.push(OUTPUT_CONTAINER_EXTENSION);
            validate_windows_file_name(&name)?;
            Ok((
                input.with_file_name(name),
                crfty_core::Replacement::KeepOriginal,
            ))
        }
        OutputTarget::SeparateFolder {
            directory,
            source_root,
        } => {
            let relative_parent = match source_root {
                Some(root) => input
                    .parent()
                    .ok_or_else(|| "input has no parent directory".to_owned())?
                    .strip_prefix(root)
                    .map_err(|_| "input is outside the configured source root".to_owned())?,
                None => std::path::Path::new(""),
            };
            if relative_parent
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err("source-relative output path is not safely contained".to_owned());
            }
            let parent = directory.join(relative_parent);
            Ok((
                parent.join(stem).with_extension(OUTPUT_CONTAINER_EXTENSION),
                crfty_core::Replacement::KeepOriginal,
            ))
        }
    }
}

fn validate_windows_file_name(name: &std::ffi::OsStr) -> Result<(), String> {
    let Some(name) = name.to_str() else {
        return Ok(());
    };
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                number.len() == 1 && matches!(number.as_bytes().first(), Some(b'1'..=b'9'))
            });
    if reserved {
        Err("output filename is reserved by Windows".to_owned())
    } else {
        Ok(())
    }
}

fn submit_output(commands: &CommandSender, delta: OutputDelta) -> Result<(), String> {
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

fn fold_transaction(transaction: &mut crfty_core::OutputTransaction, delta: OutputDelta) {
    let mut state = DurableState::default();
    state
        .outputs
        .insert(transaction.run_id, transaction.clone());
    fold(&mut state, &DurableDelta::Output(delta));
    if let Some(updated) = state.outputs.remove(&transaction.run_id) {
        *transaction = updated;
    }
}

fn terminal(
    commands: &CommandSender,
    job: &ClaimedJob,
    tracker: &mut PhaseTracker,
    outcome: ItemOutcome,
    final_telemetry: Option<Telemetry>,
) -> Result<(), String> {
    require_accepted(
        "record terminal outcome",
        commands.submit(Command::Worker(WorkerCommand::Terminal {
            item_id: job.spec.item_id,
            claim_id: job.spec.claim_id,
            run_id: job.spec.run_id,
            outcome,
            at: now_millis(),
            phase_spans: tracker.finish(),
            final_telemetry,
        })),
    )
}

/// Wraps a final adapter progress value for the lossless terminal command.
/// Rate is live display state, meaningless once the run is over, so the
/// terminal record never carries one.
fn map_progress(
    run_id: RunId,
    tracker: &mut PhaseTracker,
    phase: JobPhase,
    progress: Option<JobProgress>,
) -> Option<Telemetry> {
    progress.map(|progress| Telemetry {
        run_id,
        sequence: tracker.next_sequence(),
        phase,
        progress,
        fps_centi: None,
        eta_ms: None,
    })
}

fn publish_phase(
    commands: &CommandSender,
    run_id: RunId,
    tracker: &mut PhaseTracker,
    phase: JobPhase,
) {
    tracker.enter(phase);
    commands.publish_telemetry(Telemetry {
        run_id,
        sequence: tracker.next_sequence(),
        phase,
        progress: JobProgress::Phase,
        fps_centi: None,
        eta_ms: None,
    });
}

fn accepted(reply: Result<Reply, crate::driver::SubmitError>) -> bool {
    matches!(reply, Ok(Reply::Accepted))
}

fn require_accepted(
    context: &str,
    reply: Result<Reply, crate::driver::SubmitError>,
) -> Result<(), String> {
    match reply {
        Ok(Reply::Accepted) => Ok(()),
        Ok(Reply::Rejected { reason } | Reply::DurabilityUnknown { reason }) => {
            Err(format!("{context}: {reason}"))
        }
        Ok(other) => Err(format!("{context}: invalid driver reply {other:?}")),
        Err(error) => Err(format!("{context}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crfty_core::{OutputTarget, Replacement};

    use std::time::Duration;

    use super::{
        ActiveCancellation, ActiveJobCancellation, join_within, resolve_output_for,
        validate_windows_file_name,
    };
    use crate::ab_av1::{CancelMode, CancellationHandle};
    use crfty_core::RunId;

    #[test]
    fn suffix_is_a_filename_fragment_not_a_path() {
        for invalid in [
            "",
            "../escape",
            "\\escape",
            "bad:name",
            "bad?name",
            "trailing.",
            "space ",
        ] {
            assert!(
                OutputTarget::Suffix {
                    suffix: invalid.to_owned(),
                }
                .validate()
                .is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(
            OutputTarget::Suffix {
                suffix: "_av1".to_owned(),
            }
            .validate()
            .is_ok()
        );
        assert!(validate_windows_file_name(std::ffi::OsStr::new("CON.mkv")).is_err());
        assert!(validate_windows_file_name(std::ffi::OsStr::new("LPT9.mkv")).is_err());
    }

    #[test]
    fn replace_preserves_case_equivalent_mkv_path() {
        let input = Path::new("Movie.MKV");
        let (output, replacement) =
            resolve_output_for(input, &OutputTarget::Replace).expect("replace path");
        assert_eq!(output, input);
        assert_eq!(replacement, Replacement::KeepOriginal);
    }

    #[test]
    fn separate_output_rejects_input_outside_source_root() {
        let target = OutputTarget::SeparateFolder {
            directory: PathBuf::from("output"),
            source_root: Some(PathBuf::from("library")),
        };
        assert!(resolve_output_for(Path::new("elsewhere/movie.mkv"), &target).is_err());
    }

    #[test]
    fn join_within_reclaims_a_prompt_thread_and_abandons_a_wedged_one() {
        let prompt = std::thread::spawn(|| {});
        assert!(join_within(
            prompt,
            Duration::from_secs(1),
            Duration::from_millis(1),
        ));

        // Stands in for a wedged network read: never observes cancellation.
        let wedged = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(2)));
        assert!(!join_within(
            wedged,
            Duration::from_millis(20),
            Duration::from_millis(1),
        ));
    }

    #[test]
    fn force_before_registration_cannot_miss_the_child() {
        let cancellation = ActiveCancellation::new();
        cancellation.force(None);
        let (handle, receiver) = CancellationHandle::fixture();
        let _registration = cancellation.register(RunId(7), ActiveJobCancellation::AbAv1(handle));
        assert_eq!(*receiver.borrow(), Some(CancelMode::Force));
    }
}
