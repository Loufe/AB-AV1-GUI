//! Bridges the engine's event stream and command surface to the webview.
//!
//! One forwarder thread drains `EngineRuntime::events`, folds durable and
//! config deltas into a read model, and pushes wire events into the subscribed
//! channel. Every send happens under the same lock as the fold, so the stream
//! order the frontend observes is exactly the fold order (ADR-006). The engine
//! channel keeps draining when no webview is subscribed.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

use crfty_core::{
    AnalysisIntent, AnalysisProfile, AppSnapshot, ConfigDelta, CorruptionReport,
    CorruptionSignature, DurableDelta, DurableState, EphemeralDelta, ExecutionSettings, Operation,
    OutputTarget, OverwriteDecision, ProjectionCommand, QueueAddRequest, QueueCommand, QueueItemId,
    Reply, RunId, SessionAggregates, SessionCommand, SessionState, Settings, SettingsCommand,
    Telemetry, ToolsState, VendorCommand, fold, fold_config,
};
use crfty_engine::{
    coordinator::{EngineConfig, EngineRuntime, ToolsConfig, UserCommandSender},
    driver::DriverEvent,
    os_actions::OsActionError,
};
use serde::Serialize;
use tauri::{Manager, ipc::Channel};

const JOURNAL_FILE_NAME: &str = "journal.jsonl";
const CONFIG_FILE_NAME: &str = "config.json";
const VENDOR_DIR_NAME: &str = "vendor";
const LOG_DIR_NAME: &str = "logs";

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ShellEvent {
    pub seq: u32,
    pub payload: StreamPayload,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub enum StreamPayload {
    Snapshot(AppSnapshot),
    Durable(DurableDelta),
    Config(ConfigDelta),
    Ephemeral(EphemeralDelta),
    /// The journal failed replay validation. Reads keep working over the
    /// valid prefix; mutation is rejected until the operator acknowledges
    /// discarding the unreadable tail identified by the report's signature.
    Degraded(CorruptionReport),
    /// An acknowledged corruption was archived and compacted away; the
    /// journal is a fresh healthy generation and mutation is accepted again.
    Recovered,
    /// The engine never started (no data directory, engine start failure);
    /// there is nothing to acknowledge and no commands can run.
    EngineUnavailable {
        reason: String,
    },
    EngineFatal {
        message: String,
    },
    /// Another process holds the engine's data-directory lock: this is a
    /// second instance and the engine refused to start (ADR-008).
    SecondInstance {
        lock_path: String,
    },
}

/// Outcome of a history import: how many records were parked and how many
/// were skipped as duplicates of already-parked or already-adopted paths.
#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
pub struct ImportSummary {
    pub parked: u32,
    pub skipped: u32,
}

/// Outcome of a retroactive log scrub: how many log files were examined, how
/// many were rewritten with anonymized content, and how many failed.
#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
pub struct ScrubSummary {
    pub total: u32,
    pub modified: u32,
    pub failed: u32,
}

/// Outcome of a manual update check against the GitHub releases API.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct ReleaseSummary {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    fn engine_unavailable(message: impl Into<String>) -> Self {
        Self::new("engine_unavailable", message)
    }
}

/// A missing path is the caller's mistake (a stale row) — `rejected`; a
/// desktop that refused or failed the action is ours to surface — `internal`.
impl From<OsActionError> for CommandError {
    fn from(error: OsActionError) -> Self {
        match error {
            OsActionError::Missing { path } => {
                Self::new("rejected", format!("{} does not exist", path.display()))
            }
            OsActionError::Failed { message } => Self::new("internal", message),
        }
    }
}

#[derive(Debug, Clone)]
enum Health {
    Ok,
    /// The engine never started; no command channel exists.
    Unavailable {
        reason: String,
    },
    /// The engine is running over a corrupt journal; recovery is one
    /// matching acknowledgement away.
    Degraded {
        report: CorruptionReport,
    },
    Fatal {
        message: String,
    },
    SecondInstance {
        lock_path: String,
    },
}

struct StreamState {
    model: AppSnapshot,
    session: SessionState,
    tools: ToolsState,
    /// Latest session aggregates, mirroring `AppState::aggregates` in the
    /// reducer. Aggregates are never journaled, so the subscribe replay is
    /// the only way a mid-session reconnect learns the running totals.
    aggregates: SessionAggregates,
    /// Latest telemetry per active run, mirroring `AppState::telemetry` in
    /// the reducer so a reconnecting webview sees live progress immediately
    /// instead of waiting for the next update.
    telemetry: BTreeMap<RunId, Telemetry>,
    health: Health,
    subscriber: Option<Channel<ShellEvent>>,
    seq: u32,
}

impl StreamState {
    fn new(health: Health) -> Self {
        Self {
            model: AppSnapshot::default(),
            session: SessionState::Idle,
            tools: ToolsState::default(),
            aggregates: SessionAggregates::default(),
            telemetry: BTreeMap::new(),
            health,
            subscriber: None,
            seq: 0,
        }
    }

    fn emit(&mut self, payload: StreamPayload) {
        let Some(channel) = &self.subscriber else {
            return;
        };
        let event = ShellEvent {
            seq: self.seq,
            payload,
        };
        self.seq = self.seq.wrapping_add(1);
        if let Err(error) = channel.send(event) {
            tracing::warn!("dropping stream subscriber after failed send: {error}");
            self.subscriber = None;
        }
    }
}

pub struct Bridge {
    stream: Arc<Mutex<StreamState>>,
    commands: Option<UserCommandSender>,
    next_item_id: Arc<AtomicU64>,
    /// Release page from the last successful update check. The URL never
    /// crosses IPC: the frontend asks to open it, the shell opens what the
    /// engine actually fetched.
    release_url: Arc<Mutex<Option<String>>>,
    // Kept alive for the process lifetime: dropping the runtime shuts the
    // engine down. The Mutex only exists to make the receiver-bearing runtime
    // Sync for managed state.
    _engine: Mutex<Option<EngineRuntime>>,
}

impl Bridge {
    pub fn start(app: &tauri::AppHandle) -> Self {
        match Self::try_start(app) {
            Ok(bridge) => bridge,
            Err(health) => {
                match &health {
                    Health::SecondInstance { lock_path } => {
                        tracing::error!("another instance holds the data lock at {lock_path}");
                    }
                    Health::Unavailable { reason } => {
                        tracing::error!("engine unavailable: {reason}")
                    }
                    Health::Ok | Health::Degraded { .. } | Health::Fatal { .. } => {}
                }
                Self {
                    stream: Arc::new(Mutex::new(StreamState::new(health))),
                    commands: None,
                    next_item_id: Arc::new(AtomicU64::new(1)),
                    release_url: Arc::new(Mutex::new(None)),
                    _engine: Mutex::new(None),
                }
            }
        }
    }

    fn try_start(app: &tauri::AppHandle) -> Result<Self, Health> {
        let unavailable = |reason: String| Health::Unavailable { reason };
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| unavailable(format!("no application data directory: {error}")))?;
        std::fs::create_dir_all(&data_dir).map_err(|error| {
            unavailable(format!(
                "failed to create application data directory: {error}"
            ))
        })?;
        // Tracing comes up before the engine so lock contention and journal
        // recovery are captured; the privacy scrubber inside the sink is
        // configured from a read-only settings peek, so no line is written
        // unfiltered.
        crfty_engine::logging::init(
            &data_dir.join(LOG_DIR_NAME),
            &data_dir.join(CONFIG_FILE_NAME),
        );
        // Discovery runs inside the engine and is infallible: missing tools
        // become typed availability state on the stream, never a startup
        // failure that would hide the queue, history, or settings.
        let config = EngineConfig {
            journal_path: data_dir.join(JOURNAL_FILE_NAME),
            config_path: data_dir.join(CONFIG_FILE_NAME),
            vendor_root: data_dir.join(VENDOR_DIR_NAME),
            tools: ToolsConfig::Discover,
            execution: ExecutionSettings::production(AnalysisProfile::production(), false),
        };
        let mut runtime = EngineRuntime::start(config).map_err(|error| match error {
            crfty_engine::coordinator::EngineStartError::AlreadyRunning { lock_path } => {
                Health::SecondInstance {
                    lock_path: lock_path.display().to_string(),
                }
            }
            other => Health::Unavailable {
                reason: other.to_string(),
            },
        })?;
        let events = std::mem::replace(&mut runtime.events, mpsc::channel().1);
        let stream = Arc::new(Mutex::new(StreamState::new(Health::Ok)));
        let next_item_id = Arc::new(AtomicU64::new(1));
        let forwarder_stream = Arc::clone(&stream);
        let forwarder_ids = Arc::clone(&next_item_id);
        std::thread::Builder::new()
            .name("crfty-shell-forwarder".to_owned())
            .spawn(move || forward(events, &forwarder_stream, &forwarder_ids))
            .map_err(|error| unavailable(format!("failed to start stream forwarder: {error}")))?;
        Ok(Self {
            stream,
            commands: Some(runtime.commands.clone()),
            next_item_id,
            release_url: Arc::new(Mutex::new(None)),
            _engine: Mutex::new(Some(runtime)),
        })
    }

    /// Installs the webview's channel and replays current state into it:
    /// snapshot first, then the session state, tool availability, and session
    /// aggregates (which a fresh connection would otherwise only learn on
    /// their next transition), then any standing degradation. All under the
    /// stream lock, so no delta interleaves.
    pub fn subscribe(&self, channel: Channel<ShellEvent>) {
        let mut stream = lock_stream(&self.stream);
        stream.subscriber = Some(channel);
        stream.seq = 0;
        let snapshot = stream.model.clone();
        stream.emit(StreamPayload::Snapshot(snapshot));
        let session = stream.session.clone();
        stream.emit(StreamPayload::Ephemeral(EphemeralDelta::SessionChanged(
            session,
        )));
        let tools = stream.tools.clone();
        stream.emit(StreamPayload::Ephemeral(EphemeralDelta::ToolsChanged(
            tools,
        )));
        let aggregates = stream.aggregates;
        stream.emit(StreamPayload::Ephemeral(EphemeralDelta::SessionAggregates(
            aggregates,
        )));
        let telemetry: Vec<Telemetry> = stream.telemetry.values().cloned().collect();
        for value in telemetry {
            stream.emit(StreamPayload::Ephemeral(EphemeralDelta::Telemetry(value)));
        }
        match stream.health.clone() {
            Health::Ok => {}
            Health::Unavailable { reason } => {
                stream.emit(StreamPayload::EngineUnavailable { reason });
            }
            Health::Degraded { report } => stream.emit(StreamPayload::Degraded(report)),
            Health::Fatal { message } => stream.emit(StreamPayload::EngineFatal { message }),
            Health::SecondInstance { lock_path } => {
                stream.emit(StreamPayload::SecondInstance { lock_path });
            }
        }
    }

    pub fn allocate_item_id(&self) -> QueueItemId {
        QueueItemId(self.next_item_id.fetch_add(1, Ordering::Relaxed))
    }

    pub fn submit_queue(&self, command: QueueCommand) -> Result<(), CommandError> {
        let commands = self.commands()?;
        map_reply(commands.submit_queue(command))
    }

    /// Expands files and folders into one `AddMany` batch carrying real
    /// enqueue facts (path hash + stamp, best-effort). The scan-extension
    /// filter comes from the settings read model; for `SeparateFolder`
    /// targets, each folder-discovered file gets its originating folder as
    /// `source_root` so the output tree mirrors the source tree.
    pub fn queue_add_paths(
        &self,
        inputs: Vec<PathBuf>,
        operation: Operation,
        intent: AnalysisIntent,
        output_target: OutputTarget,
    ) -> Result<(), CommandError> {
        let commands = self.commands()?;
        let extensions = lock_stream(&self.stream)
            .model
            .settings
            .scan_extensions
            .clone();
        let requests = crfty_engine::scan::expand_inputs(&inputs, &extensions)
            .into_iter()
            .map(|file| {
                let output_target = match (&output_target, file.source_root) {
                    (OutputTarget::SeparateFolder { directory, .. }, Some(root)) => {
                        OutputTarget::SeparateFolder {
                            directory: directory.clone(),
                            source_root: Some(root),
                        }
                    }
                    _ => output_target.clone(),
                };
                QueueAddRequest {
                    item_id: self.allocate_item_id(),
                    input: file.path,
                    path_hash: file.path_hash,
                    stamp: file.stamp,
                    operation,
                    intent,
                    output_target,
                    overwrite: OverwriteDecision::FollowSettings,
                }
            })
            .collect();
        map_reply(commands.submit_queue(QueueCommand::AddMany { requests }))
    }

    pub fn submit_session(&self, command: SessionCommand) -> Result<(), CommandError> {
        let commands = self.commands()?;
        map_reply(commands.submit_session(command))
    }

    pub fn submit_settings(&self, settings: Settings) -> Result<(), CommandError> {
        let commands = self.commands()?;
        map_reply(commands.submit_settings(SettingsCommand::Set { settings }))
    }

    pub fn submit_vendor(&self, command: VendorCommand) -> Result<(), CommandError> {
        let commands = self.commands()?;
        map_reply(commands.submit_vendor(command))
    }

    pub fn submit_projection(&self, command: ProjectionCommand) -> Result<(), CommandError> {
        let commands = self.commands()?;
        map_reply(commands.submit_projection(command))
    }

    /// Reads and parses the import file in the engine, then submits it for
    /// durable parking. Failures (unreadable/malformed file, degraded
    /// journal) come back as one user-facing message.
    pub fn import_history(&self, path: &std::path::Path) -> Result<ImportSummary, CommandError> {
        let commands = self.commands()?;
        commands
            .import_history(path)
            .map(|summary| ImportSummary {
                parked: summary.parked,
                skipped: summary.skipped,
            })
            .map_err(|message| CommandError::new("import_failed", message))
    }

    /// Anonymizes every log file under the active log directory in place,
    /// including rolled files from earlier launches. Runs regardless of the
    /// anonymize-logs toggle and is irreversible. Gated on a healthy engine
    /// so a second instance can never rewrite files the lock holder is
    /// actively writing.
    pub fn scrub_logs(&self) -> Result<ScrubSummary, CommandError> {
        self.commands()?;
        crfty_engine::logging::scrub_log_files()
            .map(|outcome| ScrubSummary {
                total: outcome.total,
                modified: outcome.modified,
                failed: outcome.failed,
            })
            .map_err(|message| CommandError::new("scrub_failed", message))
    }

    /// The slot the blocking update check writes its release page into.
    /// Cloned out so the check can run on a worker thread without borrowing
    /// the bridge.
    pub fn release_url_slot(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.release_url)
    }

    /// Runs the one-shot update check and records the release page for
    /// [`Bridge::open_release_page`]. Blocks on the network — callers run it
    /// off the UI thread.
    pub fn check_for_update(slot: &Mutex<Option<String>>) -> Result<ReleaseSummary, CommandError> {
        let check = crfty_engine::release::check_latest_release(env!("CARGO_PKG_VERSION"))
            .map_err(|message| CommandError::new("update_check_failed", message))?;
        *lock_slot(slot) = Some(check.html_url);
        Ok(ReleaseSummary {
            current: check.current,
            latest: check.latest,
            update_available: check.update_available,
        })
    }

    /// Opens the release page recorded by the last successful update check.
    pub fn open_release_page(&self) -> Result<(), CommandError> {
        let url = lock_slot(&self.release_url).clone().ok_or_else(|| {
            CommandError::new(
                "no_release_page",
                "no release page is known; run an update check first",
            )
        })?;
        Ok(crfty_engine::os_actions::open_url(&url)?)
    }

    /// Passes through while degraded by design: acknowledgement is the one
    /// mutation a corrupt journal accepts, and the driver verifies the
    /// signature itself.
    pub fn acknowledge_corruption(
        &self,
        signature: CorruptionSignature,
    ) -> Result<(), CommandError> {
        let commands = self.commands()?;
        map_reply(commands.acknowledge_corruption(signature))
    }

    fn commands(&self) -> Result<&UserCommandSender, CommandError> {
        self.commands.as_ref().ok_or_else(|| {
            let stream = lock_stream(&self.stream);
            match &stream.health {
                Health::Unavailable { reason } => CommandError::engine_unavailable(reason.clone()),
                Health::Degraded { report } => {
                    CommandError::engine_unavailable(report.reason.clone())
                }
                Health::Fatal { message } => CommandError::engine_unavailable(message.clone()),
                Health::SecondInstance { lock_path } => CommandError::new(
                    "second_instance",
                    format!("another instance holds the data lock at {lock_path}"),
                ),
                Health::Ok => CommandError::engine_unavailable("engine is not running"),
            }
        })
    }
}

fn forward(
    events: mpsc::Receiver<DriverEvent>,
    stream: &Arc<Mutex<StreamState>>,
    next_item_id: &AtomicU64,
) {
    while let Ok(event) = events.recv() {
        let mut state = lock_stream(stream);
        absorb(&mut state, event, next_item_id);
    }
    // The engine severs this stream only on public-channel overflow (its
    // subscriber-drop contract) — the engine itself keeps running, but this
    // mirror can no longer observe it and every later snapshot replay would
    // be silently stale. Surface the cut instead of freezing quietly.
    eprintln!("engine event stream disconnected; marking the bridge fatal");
    let message = "lost the engine event stream; restart CRFty to reconnect".to_owned();
    let mut state = lock_stream(stream);
    state.health = Health::Fatal {
        message: message.clone(),
    };
    state.emit(StreamPayload::EngineFatal { message });
}

/// Folds one driver event into the read model and emits its wire payload.
/// Separated from the forwarder loop so tests can drive it directly (with no
/// subscriber installed, `emit` is a no-op).
fn absorb(state: &mut StreamState, event: DriverEvent, next_item_id: &AtomicU64) {
    match event {
        DriverEvent::Snapshot(model) => {
            seed_item_ids(next_item_id, &model.durable);
            state.model = model.clone();
            state.emit(StreamPayload::Snapshot(model));
        }
        DriverEvent::Durable(delta) => {
            fold(&mut state.model.durable, &delta);
            if let DurableDelta::QueueAdded { item } = &delta {
                next_item_id.fetch_max(item.id.0.saturating_add(1), Ordering::Relaxed);
            }
            state.emit(StreamPayload::Durable(delta));
        }
        DriverEvent::Config(delta) => {
            fold_config(&mut state.model.settings, &delta);
            state.emit(StreamPayload::Config(delta));
        }
        DriverEvent::Ephemeral(delta) => {
            match &delta {
                EphemeralDelta::SessionChanged(session) => state.session = session.clone(),
                EphemeralDelta::SessionAggregates(aggregates) => state.aggregates = *aggregates,
                EphemeralDelta::ToolsChanged(tools) => state.tools = tools.clone(),
                EphemeralDelta::Telemetry(telemetry) => {
                    state.telemetry.insert(telemetry.run_id, telemetry.clone());
                }
                EphemeralDelta::TelemetryCleared { run_id } => {
                    state.telemetry.remove(run_id);
                }
                // Statistics answers are fire-and-forget: not part of the
                // read model, so a late subscriber never gets a replay and
                // re-requests instead.
                EphemeralDelta::Statistics(_)
                | EphemeralDelta::WorkerCrashed { .. }
                | EphemeralDelta::CommandRejected { .. }
                | EphemeralDelta::QueueAddSummary { .. } => {}
            }
            state.emit(StreamPayload::Ephemeral(delta));
        }
        // Effects are instructions to the engine's own supervisor and
        // never cross the IPC boundary (ADR-006).
        DriverEvent::Effect(_) => {}
        DriverEvent::Degraded(report) => {
            state.health = Health::Degraded {
                report: report.clone(),
            };
            state.emit(StreamPayload::Degraded(report));
        }
        // The stored health must clear too, or a webview subscribing after
        // the recovery would replay a degraded banner over a healthy journal.
        DriverEvent::Recovered => {
            state.health = Health::Ok;
            state.emit(StreamPayload::Recovered);
        }
        DriverEvent::Fatal { message } => {
            state.health = Health::Fatal {
                message: message.clone(),
            };
            state.emit(StreamPayload::EngineFatal { message });
        }
    }
}

fn seed_item_ids(next_item_id: &AtomicU64, model: &DurableState) {
    let maximum = model.queue.iter().map(|item| item.id.0).max().unwrap_or(0);
    next_item_id.fetch_max(maximum.saturating_add(1), Ordering::Relaxed);
}

fn lock_stream<'a>(stream: &'a Arc<Mutex<StreamState>>) -> MutexGuard<'a, StreamState> {
    match stream.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock_slot(slot: &Mutex<Option<String>>) -> MutexGuard<'_, Option<String>> {
    match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn map_reply(reply: Result<Reply, crfty_engine::driver::SubmitError>) -> Result<(), CommandError> {
    match reply {
        Ok(Reply::Accepted) => Ok(()),
        Ok(Reply::Rejected { reason }) => Err(CommandError::new("rejected", reason)),
        Ok(Reply::DurabilityUnknown { reason }) => {
            Err(CommandError::new("durability_unknown", reason))
        }
        Ok(Reply::Reserved(_) | Reply::Claimed(_) | Reply::Imported { .. }) => {
            Err(CommandError::new(
                "internal",
                "driver returned a worker reply to a user command",
            ))
        }
        Err(error) => Err(CommandError::engine_unavailable(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use crfty_core::{JobPhase, JobProgress};

    use super::*;

    fn telemetry(run: u64, sequence: u64, position_ms: u64) -> Telemetry {
        Telemetry {
            run_id: RunId(run),
            sequence,
            phase: JobPhase::Encoding,
            progress: JobProgress::OutputPositionMs(position_ms),
            fps_centi: None,
            eta_ms: None,
        }
    }

    fn ephemeral(delta: EphemeralDelta) -> DriverEvent {
        DriverEvent::Ephemeral(delta)
    }

    #[test]
    fn absorb_retains_the_latest_telemetry_per_run_until_cleared() {
        let ids = AtomicU64::new(1);
        let mut state = StreamState::new(Health::Ok);
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::Telemetry(telemetry(1, 1, 10))),
            &ids,
        );
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::Telemetry(telemetry(1, 2, 20))),
            &ids,
        );
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::Telemetry(telemetry(2, 1, 5))),
            &ids,
        );
        assert_eq!(state.telemetry.len(), 2);
        assert_eq!(state.telemetry.get(&RunId(1)), Some(&telemetry(1, 2, 20)));
        assert_eq!(state.telemetry.get(&RunId(2)), Some(&telemetry(2, 1, 5)));

        absorb(
            &mut state,
            ephemeral(EphemeralDelta::TelemetryCleared { run_id: RunId(1) }),
            &ids,
        );
        assert_eq!(state.telemetry.get(&RunId(1)), None);
        assert_eq!(state.telemetry.get(&RunId(2)), Some(&telemetry(2, 1, 5)));
    }

    #[test]
    fn absorb_tracks_session_and_tools_without_touching_telemetry() {
        let ids = AtomicU64::new(1);
        let mut state = StreamState::new(Health::Ok);
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::SessionChanged(SessionState::Running)),
            &ids,
        );
        let tools = ToolsState {
            update_available: true,
            ..ToolsState::default()
        };
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::ToolsChanged(tools.clone())),
            &ids,
        );
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::WorkerCrashed {
                message: "fixture".to_owned(),
            }),
            &ids,
        );
        assert_eq!(state.session, SessionState::Running);
        assert_eq!(state.tools, tools);
        assert!(state.telemetry.is_empty());
    }

    #[test]
    fn a_severed_engine_stream_marks_the_bridge_fatal() {
        let ids = AtomicU64::new(1);
        let stream = Arc::new(Mutex::new(StreamState::new(Health::Ok)));
        let (sender, receiver) = mpsc::channel();
        sender
            .send(ephemeral(EphemeralDelta::SessionChanged(
                SessionState::Running,
            )))
            .expect("send fixture event");
        drop(sender);

        forward(receiver, &stream, &ids);

        let state = lock_stream(&stream);
        // Events before the cut were absorbed; the cut itself became fatal
        // instead of a silently frozen mirror.
        assert_eq!(state.session, SessionState::Running);
        assert!(
            matches!(state.health, Health::Fatal { .. }),
            "expected fatal health after the stream disconnect"
        );
    }

    #[test]
    fn absorb_mirrors_the_latest_session_aggregates() {
        let ids = AtomicU64::new(1);
        let mut state = StreamState::new(Health::Ok);
        let first = SessionAggregates {
            completed: 1,
            input_bytes: 1_000,
            output_bytes: 400,
            ..SessionAggregates::default()
        };
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::SessionAggregates(first)),
            &ids,
        );
        assert_eq!(state.aggregates, first);

        // The zeroed emission at session start replaces the standing totals.
        absorb(
            &mut state,
            ephemeral(EphemeralDelta::SessionAggregates(
                SessionAggregates::default(),
            )),
            &ids,
        );
        assert_eq!(state.aggregates, SessionAggregates::default());
    }
}
