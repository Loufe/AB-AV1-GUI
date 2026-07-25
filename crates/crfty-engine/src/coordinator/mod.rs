//! Engine runtime lifecycle: starting the driver, wiring the tool and
//! vendor services around it, and the command surface callers hold.
//!
//! The work itself lives in the submodules: startup recovery, supervision,
//! vendor tasks, the session and job pipeline, and output settlement.

mod job;
mod output_flow;
mod output_path;
mod recovery;
mod session;
mod supervision;
mod telemetry;
mod vendor_task;

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use crfty_core::{
    AnalysisGenerationId, AppSnapshot, Command, CorruptionSignature, DurableState,
    ExecutionSettings, HistoryCommand, ProjectionCommand, QueueCommand, QueueItemState, Reply,
    SessionCommand, SettingsCommand, SystemCommand, VendorCommand, VideoExtension,
};

use crate::{
    ab_av1::AbAv1Runtime,
    driver::{CommandSender, DriverEvent, DriverHandle, DriverStartError},
    vendor::discovery::{self, DiscoveredTools, DiscoveryReport},
};

use self::recovery::recover_startup;
use self::supervision::supervise;
use crate::clock::now_millis;

const FIRST_RUNTIME_ID: u64 = 1;

/// Depth of the public event channel. A healthy consumer drains continuously,
/// so occupancy stays near zero; the bound only bites once the consumer has
/// stopped, and must comfortably exceed the startup burst (snapshot plus
/// drained ephemerals) that is buffered before any consumer exists. Public so
/// the overflow test can size its event flood relative to it.
pub const PUBLIC_EVENT_CHANNEL_CAPACITY: usize = 1024;

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
