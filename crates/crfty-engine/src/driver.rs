use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use crfty_core::{
    AppSnapshot, AppState, COMPACTION_IDLE_MIN_JOURNAL_BYTES, Command, ConfigDelta,
    CorruptionReport, CorruptionSignature, DurableDelta, DurableState, Effect, EphemeralDelta,
    QueueItemState, Reply, RunId, Settings, SystemCommand, Telemetry, apply, compaction_due,
    compaction_quiescent,
};

use crate::{
    config::ConfigStore,
    journal::{DurabilityToken, JournalError, JournalWriter},
    lock::{DataLock, DataLockError},
    sentinel::CrashSentinel,
};

const DRIVER_CHANNEL_CAPACITY: usize = 64;
const COMMAND_REPLY_CHANNEL_CAPACITY: usize = 0;
const DRIVER_TICK: Duration = Duration::from_millis(20);
/// Idle ticks to wait before re-attempting a failed compaction (~10 s at the
/// 20 ms tick), so a persistent failure never becomes a tight retry loop.
const COMPACTION_RETRY_TICKS: u32 = 500;

#[derive(Debug, Clone, PartialEq)]
pub enum DriverEvent {
    Snapshot(AppSnapshot),
    Durable(DurableDelta),
    Config(ConfigDelta),
    Ephemeral(EphemeralDelta),
    Effect(Effect),
    Degraded(CorruptionReport),
    /// An acknowledged corruption was archived and compacted away; the
    /// journal is a fresh healthy generation and mutation is accepted again.
    Recovered,
    /// The previous run left the crash sentinel behind: it died without a
    /// clean driver shutdown. Durable state was already restored by the
    /// journal replay; this is the boot-scoped report of that fact (#33 §12).
    AbnormalShutdown,
    Fatal {
        message: String,
    },
}

#[derive(Debug)]
pub enum DriverStartError {
    /// The data-directory lock is held by another process: a second instance.
    AlreadyRunning {
        lock_path: std::path::PathBuf,
    },
    Failed(String),
}

impl fmt::Display for DriverStartError {
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

impl std::error::Error for DriverStartError {}

#[derive(Debug)]
pub enum SubmitError {
    Disconnected,
    ReplyDisconnected,
}

impl fmt::Display for SubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => formatter.write_str("driver command channel disconnected"),
            Self::ReplyDisconnected => formatter.write_str("driver reply channel disconnected"),
        }
    }
}

impl std::error::Error for SubmitError {}

struct Envelope {
    command: Command,
    reply: mpsc::SyncSender<Reply>,
}

struct DriverPersistence {
    journal: JournalWriter,
    config: ConfigStore,
    /// Held for the driver's lifetime; releasing it is what allows the next
    /// instance to start (ADR-008).
    _lock: DataLock,
    /// Armed for the driver's lifetime and disarmed only on clean exit; a
    /// leftover sentinel is what the next boot reports as an abnormal
    /// shutdown (#33 §12).
    sentinel: CrashSentinel,
}

#[derive(Clone)]
pub struct CommandSender {
    sender: mpsc::SyncSender<Envelope>,
    telemetry: Arc<Mutex<BTreeMap<RunId, Telemetry>>>,
}

impl CommandSender {
    pub fn submit(&self, command: Command) -> Result<Reply, SubmitError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(COMMAND_REPLY_CHANNEL_CAPACITY);
        self.sender
            .send(Envelope {
                command,
                reply: reply_tx,
            })
            .map_err(|_| SubmitError::Disconnected)?;
        reply_rx.recv().map_err(|_| SubmitError::ReplyDisconnected)
    }

    pub fn publish_telemetry(&self, telemetry: Telemetry) {
        let mut slot = match self.telemetry.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        let replace = slot
            .get(&telemetry.run_id)
            .is_none_or(|current| telemetry.sequence > current.sequence);
        if replace {
            slot.insert(telemetry.run_id, telemetry);
        }
    }
}

pub struct DriverHandle {
    pub commands: CommandSender,
    events: Option<mpsc::Receiver<DriverEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DriverHandle {
    pub fn start(
        journal_path: impl AsRef<Path>,
        config_path: impl AsRef<Path>,
    ) -> Result<Self, DriverStartError> {
        Self::start_inner(journal_path, config_path, None)
    }

    pub(crate) fn start_with_effects(
        journal_path: impl AsRef<Path>,
        config_path: impl AsRef<Path>,
        effects: mpsc::Sender<Effect>,
    ) -> Result<Self, DriverStartError> {
        Self::start_inner(journal_path, config_path, Some(effects))
    }

    fn start_inner(
        journal_path: impl AsRef<Path>,
        config_path: impl AsRef<Path>,
        effect_sink: Option<mpsc::Sender<Effect>>,
    ) -> Result<Self, DriverStartError> {
        let journal_path = journal_path.as_ref();
        // The data directory is the journal's directory; the lock is taken
        // before settings load or journal fold so a second instance never
        // reads (let alone writes) shared durable state (#33 §12).
        let data_dir = journal_path.parent().unwrap_or(Path::new("."));
        let lock = DataLock::acquire(data_dir).map_err(|error| match error {
            DataLockError::AlreadyHeld { path } => {
                DriverStartError::AlreadyRunning { lock_path: path }
            }
            DataLockError::Io { .. } => {
                DriverStartError::Failed(format!("failed to lock data directory: {error}"))
            }
        })?;
        // Armed while holding the lock and before any durable state is
        // touched (#33 §12): whatever happens from here on, dying without a
        // clean shutdown leaves the sentinel for the next boot to report.
        let sentinel = CrashSentinel::arm(data_dir);
        let config = ConfigStore::new(config_path.as_ref().to_path_buf());
        let loaded = config.load().map_err(|error| {
            DriverStartError::Failed(format!("failed to load settings: {error}"))
        })?;
        let (writer, replay) = JournalWriter::open(journal_path).map_err(|error| {
            DriverStartError::Failed(format!("failed to start driver: {error}"))
        })?;
        let degraded = replay
            .corruption
            .as_ref()
            .map(|corruption| CorruptionReport {
                reason: format!(
                    "journal is corrupt at byte {}: {}",
                    corruption.offset, corruption.reason
                ),
                signature: corruption.signature.clone(),
            });
        let state = AppState {
            durable: replay.state,
            settings: loaded.settings,
            ..AppState::default()
        };
        let (command_tx, command_rx) = mpsc::sync_channel(DRIVER_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel();
        let telemetry = Arc::new(Mutex::new(BTreeMap::new()));
        let driver_telemetry = Arc::clone(&telemetry);
        let worker = thread::Builder::new()
            .name("crfty-driver".to_owned())
            .spawn(move || {
                run_driver(
                    state,
                    DriverPersistence {
                        journal: writer,
                        config,
                        _lock: lock,
                        sentinel,
                    },
                    degraded,
                    command_rx,
                    event_tx,
                    driver_telemetry,
                    effect_sink,
                );
            })
            .map_err(|error| {
                DriverStartError::Failed(format!("failed to spawn driver: {error}"))
            })?;
        Ok(Self {
            commands: CommandSender {
                sender: command_tx,
                telemetry,
            },
            events: Some(event_rx),
            worker: Some(worker),
        })
    }

    pub fn events(&self) -> Option<&mpsc::Receiver<DriverEvent>> {
        self.events.as_ref()
    }

    pub(crate) fn take_events(&mut self) -> Option<mpsc::Receiver<DriverEvent>> {
        self.events.take()
    }

    pub fn shutdown(mut self) -> Result<(), DriverStartError> {
        let _reply = self
            .commands
            .submit(Command::System(SystemCommand::Shutdown));
        self.join()
    }

    fn join(&mut self) -> Result<(), DriverStartError> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| DriverStartError::Failed("driver thread panicked".to_owned()))?;
        }
        Ok(())
    }
}

impl Drop for DriverHandle {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _reply = self
                .commands
                .submit(Command::System(SystemCommand::Shutdown));
            let _result = self.join();
        }
    }
}

fn run_driver(
    mut state: AppState,
    mut persistence: DriverPersistence,
    mut degraded: Option<CorruptionReport>,
    receiver: mpsc::Receiver<Envelope>,
    events: mpsc::Sender<DriverEvent>,
    telemetry: Arc<Mutex<BTreeMap<RunId, Telemetry>>>,
    effect_sink: Option<mpsc::Sender<Effect>>,
) {
    let _result = events.send(DriverEvent::Snapshot(AppSnapshot {
        durable: state.durable.clone(),
        settings: state.settings.clone(),
    }));
    if let Some(report) = &degraded {
        let _result = events.send(DriverEvent::Degraded(report.clone()));
    }
    if persistence.sentinel.previous_run_abnormal() {
        let _result = events.send(DriverEvent::AbnormalShutdown);
    }
    let mut compaction_backoff: u32 = 0;
    // A history import must not linger as journal deltas: the parked records
    // carry cleartext paths, and adoption semantics assume the inbox state is
    // the snapshot. Imports therefore force a compaction at the next idle
    // tick, retried until one actually lands.
    let mut force_compact_pending = false;
    'running: loop {
        let first = match receiver.recv_timeout(DRIVER_TICK) {
            Ok(envelope) => envelope,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                emit_latest_telemetry(&state, &events, &telemetry);
                if compaction_backoff > 0 {
                    compaction_backoff = compaction_backoff.saturating_sub(1);
                } else {
                    match maybe_compact(
                        &state,
                        &mut persistence,
                        degraded.is_some(),
                        force_compact_pending,
                    ) {
                        CompactionOutcome::Compacted => force_compact_pending = false,
                        CompactionOutcome::Skipped => {}
                        CompactionOutcome::Failed => compaction_backoff = COMPACTION_RETRY_TICKS,
                    }
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break 'running,
        };
        emit_latest_telemetry(&state, &events, &telemetry);
        let mut batch = vec![first];
        batch.extend(receiver.try_iter());
        for batch in split_batch_at_settings(batch) {
            let outcome = process_batch(
                &mut state,
                &mut persistence.journal,
                &mut persistence.config,
                &mut degraded,
                batch,
                &events,
                effect_sink.as_ref(),
            );
            force_compact_pending = force_compact_pending || outcome.imported_history;
            if outcome.should_stop {
                break 'running;
            }
        }
    }
    // Reached only when the loop exits normally (shutdown command or every
    // sender dropped) — a driver panic unwinds past this and leaves the
    // sentinel for the next boot to report.
    persistence.sentinel.disarm();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionOutcome {
    /// A snapshot was written; the journal is a fresh generation.
    Compacted,
    /// Nothing to do: degraded, not quiescent, or not due by size policy.
    Skipped,
    /// A compaction was attempted and failed; the caller backs off and the
    /// old journal generation remains authoritative.
    Failed,
}

/// Compact the journal at the driver's idle tick (#33 §10). The writer
/// barrier is implicit: the driver thread is the only journal writer and it
/// sits between batches here. `force` bypasses the size policy (but never the
/// quiescence or degraded checks) for the durable transforms that require a
/// fresh generation — scrub, corruption acknowledgment, history import.
fn maybe_compact(
    state: &AppState,
    persistence: &mut DriverPersistence,
    degraded: bool,
    force: bool,
) -> CompactionOutcome {
    if degraded || !compaction_quiescent(state) {
        return CompactionOutcome::Skipped;
    }
    if !force {
        let journal_bytes = persistence.journal.journal_bytes();
        // Floor check first: measuring live state serializes all of it, which
        // is not free on every idle tick.
        if journal_bytes < COMPACTION_IDLE_MIN_JOURNAL_BYTES {
            return CompactionOutcome::Skipped;
        }
        let live_bytes = match serde_json::to_vec(&state.durable) {
            Ok(encoded) => u64::try_from(encoded.len()).unwrap_or(u64::MAX),
            Err(error) => {
                tracing::warn!("skipping compaction; failed to measure live state: {error}");
                return CompactionOutcome::Failed;
            }
        };
        if !compaction_due(journal_bytes, live_bytes) {
            return CompactionOutcome::Skipped;
        }
    }
    match persistence.journal.compact(
        &state.durable,
        env!("CARGO_PKG_VERSION"),
        crate::coordinator::now_millis(),
    ) {
        Ok(()) => CompactionOutcome::Compacted,
        Err(error) => {
            tracing::warn!("journal compaction failed; old journal remains authoritative: {error}");
            CompactionOutcome::Failed
        }
    }
}

fn split_batch_at_settings(batch: Vec<Envelope>) -> Vec<Vec<Envelope>> {
    let mut groups: Vec<Vec<Envelope>> = Vec::new();
    for envelope in batch {
        let settings = matches!(&envelope.command, Command::Settings(_));
        let append = groups.last().is_some_and(|group| {
            group
                .first()
                .is_some_and(|first| matches!(&first.command, Command::Settings(_)) == settings)
        });
        if append {
            if let Some(group) = groups.last_mut() {
                group.push(envelope);
            }
        } else {
            groups.push(vec![envelope]);
        }
    }
    groups
}

struct BatchOutcome {
    should_stop: bool,
    /// The batch's durable deltas contained a `HistoryImported`; the caller
    /// schedules a forced compaction.
    imported_history: bool,
}

fn process_batch(
    state: &mut AppState,
    writer: &mut impl JournalSink,
    config: &mut impl ConfigSink,
    degraded: &mut Option<CorruptionReport>,
    batch: Vec<Envelope>,
    events: &mpsc::Sender<DriverEvent>,
    effect_sink: Option<&mpsc::Sender<Effect>>,
) -> BatchOutcome {
    let settings_before = state.settings.clone();
    let mut durable = Vec::new();
    let mut applied_batch = Vec::with_capacity(batch.len());
    let mut recovered = false;
    for envelope in batch {
        // Acknowledgement is intercepted ahead of the reducer: degraded state
        // deliberately lives outside `AppState`, so only the driver can
        // verify the signature and rewrite the journal.
        if let Command::System(SystemCommand::AcknowledgeCorruption { signature }) =
            &envelope.command
        {
            let applied = acknowledge_corruption(signature, degraded, writer, &state.durable);
            recovered = recovered || matches!(applied.reply, Reply::Accepted);
            applied_batch.push((envelope.reply, applied));
            continue;
        }
        let applied = if let Some(report) = degraded.as_ref() {
            // System commands (shutdown, tool availability) emit no durable
            // deltas, so they stay usable over a corrupt journal.
            if matches!(envelope.command, Command::System(_)) {
                apply(state, envelope.command)
            } else {
                rejected_applied(report.reason.clone())
            }
        } else {
            apply(state, envelope.command)
        };
        durable.extend(applied.durable.iter().cloned());
        applied_batch.push((envelope.reply, applied));
    }

    let durability = if durable.is_empty() {
        None
    } else {
        match writer.append(&durable) {
            Ok(token) => Some(token),
            Err(error) => {
                fail_batch(applied_batch, events, error);
                return BatchOutcome {
                    should_stop: true,
                    imported_history: false,
                };
            }
        }
    };
    let imported_history = durable
        .iter()
        .any(|delta| matches!(delta, DurableDelta::HistoryImported { .. }));
    persist_settings(state, config, &settings_before, &mut applied_batch);
    let should_stop = emit_batch(durability, applied_batch, state, events, effect_sink);
    // Sent after the batch's own events so a rejection issued while still
    // degraded is never observed after the recovery notice.
    if recovered {
        let _result = events.send(DriverEvent::Recovered);
    }
    BatchOutcome {
        should_stop,
        imported_history,
    }
}

/// Recovery is consent to discard the corrupt tail — but only the tail the
/// operator actually saw. The submitted signature must match the standing
/// report; on any mismatch or rewrite failure the journal is left untouched
/// and the acknowledgement stays retryable.
fn acknowledge_corruption(
    signature: &CorruptionSignature,
    degraded: &mut Option<CorruptionReport>,
    writer: &mut impl JournalSink,
    durable: &DurableState,
) -> crfty_core::Applied {
    let Some(report) = degraded.as_ref() else {
        return rejected_applied("journal is not degraded; nothing to acknowledge".to_owned());
    };
    if report.signature != *signature {
        return rejected_applied(
            "corruption signature does not match the journal on disk".to_owned(),
        );
    }
    match writer.recover(durable) {
        Ok(()) => {
            *degraded = None;
            crfty_core::Applied {
                durable: Vec::new(),
                config: Vec::new(),
                ephemeral: Vec::new(),
                effects: Vec::new(),
                reply: Reply::Accepted,
            }
        }
        Err(error) => {
            tracing::error!("journal recovery failed; corrupt journal left in place: {error}");
            rejected_applied(format!("journal recovery failed: {error}"))
        }
    }
}

/// Mirrors the reducer's rejection convention: the caller's `Reply` and a
/// `CommandRejected` ephemeral on the stream, in lockstep.
fn rejected_applied(reason: String) -> crfty_core::Applied {
    crfty_core::Applied {
        durable: Vec::new(),
        config: Vec::new(),
        ephemeral: vec![EphemeralDelta::CommandRejected {
            reason: reason.clone(),
        }],
        effects: Vec::new(),
        reply: Reply::Rejected { reason },
    }
}

trait ConfigSink {
    fn write(&mut self, settings: &Settings) -> Result<(), String>;
}

impl ConfigSink for ConfigStore {
    fn write(&mut self, settings: &Settings) -> Result<(), String> {
        ConfigStore::write(self, settings).map_err(|error| error.to_string())?;
        // Only durably persisted settings reach the live sink: a rejected
        // write must not change what the logs anonymize or where they land.
        crate::logging::reconfigure(settings);
        Ok(())
    }
}

fn persist_settings(
    state: &mut AppState,
    config: &mut impl ConfigSink,
    settings_before: &Settings,
    applied_batch: &mut [(mpsc::SyncSender<Reply>, crfty_core::Applied)],
) {
    let final_write =
        applied_batch
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, (_reply, applied))| {
                let settings = applied
                    .effects
                    .iter()
                    .rev()
                    .find_map(|effect| match effect {
                        Effect::WriteSettings { settings } => Some(settings.clone()),
                        Effect::StartWorker
                        | Effect::KillActiveRun { .. }
                        | Effect::VendorInstall
                        | Effect::VendorCheck
                        | Effect::StopDriver => None,
                    });
                settings.map(|settings| (index, settings))
            });
    let Some((final_write_index, final_settings)) = final_write else {
        return;
    };
    let write_result = config.write(&final_settings);
    let final_change =
        (settings_before != &final_settings).then_some(ConfigDelta::SettingsChanged {
            settings: final_settings,
        });
    let mut final_change = final_change;
    for (_reply, applied) in applied_batch.iter_mut() {
        let wrote_settings = applied
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::WriteSettings { .. }));
        applied
            .effects
            .retain(|effect| !matches!(effect, Effect::WriteSettings { .. }));
        applied.config.clear();
        if !wrote_settings {
            continue;
        }
        if let Err(reason) = &write_result {
            applied.ephemeral.push(EphemeralDelta::CommandRejected {
                reason: reason.clone(),
            });
            applied.reply = Reply::Rejected {
                reason: reason.clone(),
            };
        }
    }
    match write_result {
        Ok(()) => {
            if let Some(delta) = final_change.take()
                && let Some((_reply, applied)) = applied_batch.get_mut(final_write_index)
            {
                applied.config.push(delta);
            }
        }
        Err(_) => state.settings = settings_before.clone(),
    }
}

trait JournalSink {
    fn append(&mut self, deltas: &[DurableDelta]) -> Result<DurabilityToken, JournalError>;
    /// Archive the corrupt journal and replace it with a snapshot of `state`.
    fn recover(&mut self, state: &DurableState) -> Result<(), JournalError>;
}

impl JournalSink for JournalWriter {
    fn append(&mut self, deltas: &[DurableDelta]) -> Result<DurabilityToken, JournalError> {
        self.append_batch(deltas).map(|(_records, token)| token)
    }

    fn recover(&mut self, state: &DurableState) -> Result<(), JournalError> {
        self.recover_corrupt(
            state,
            env!("CARGO_PKG_VERSION"),
            crate::coordinator::now_millis(),
        )
        .map(|_archive| ())
    }
}

/// Publishes each applied command in fold order as config, then ephemeral,
/// then durable deltas, followed by its reply. Ephemerals precede durables so
/// terminal state is never followed by stale progress: a finishing item is
/// observed as final telemetry, telemetry-cleared, then `ItemFinished`. The
/// standing read-model exceptions are `SessionAggregates` and Analysis:
/// both can summarize durable facts from the same command, so they publish
/// after the durables and never lead their source. Discovery-only Analysis
/// commands have no durable deltas, making the same ordering harmless there.
/// Durable publication still requires the token minted by the journal fsync,
/// so nothing observable can outrun durability.
fn emit_batch(
    durability: Option<DurabilityToken>,
    applied_batch: Vec<(mpsc::SyncSender<Reply>, crfty_core::Applied)>,
    state: &AppState,
    events: &mpsc::Sender<DriverEvent>,
    effect_sink: Option<&mpsc::Sender<Effect>>,
) -> bool {
    let mut effects = Vec::new();
    for (reply, applied) in applied_batch {
        for delta in applied.config {
            let _result = events.send(DriverEvent::Config(delta));
        }
        let (post_durable, ephemerals): (Vec<_>, Vec<_>) =
            applied.ephemeral.into_iter().partition(|delta| {
                matches!(
                    delta,
                    EphemeralDelta::Analysis(_) | EphemeralDelta::SessionAggregates(_)
                )
            });
        for delta in ephemerals {
            let _result = events.send(DriverEvent::Ephemeral(delta));
        }
        if let Some(token) = &durability {
            emit_durable(token, &applied.durable, events);
        }
        for delta in post_durable {
            let _result = events.send(DriverEvent::Ephemeral(delta));
        }
        effects.extend(applied.effects);
        let _result = reply.send(applied.reply);
    }
    let effects = reconcile_effects(effects, state);
    let mut should_stop = effects.contains(&Effect::StopDriver);
    for effect in effects {
        match effect_sink {
            Some(sink) => {
                if sink.send(effect).is_err() {
                    should_stop = true;
                    let _result = events.send(DriverEvent::Fatal {
                        message: "job supervisor effect channel disconnected".to_owned(),
                    });
                    break;
                }
            }
            None => {
                let _result = events.send(DriverEvent::Effect(effect));
            }
        }
    }
    should_stop
}

fn emit_durable(
    _token: &DurabilityToken,
    deltas: &[DurableDelta],
    events: &mpsc::Sender<DriverEvent>,
) {
    for delta in deltas {
        let _result = events.send(DriverEvent::Durable(delta.clone()));
    }
}

fn reconcile_effects(effects: Vec<Effect>, state: &AppState) -> Vec<Effect> {
    let mut reconciled = Vec::new();
    for effect in effects {
        match effect {
            Effect::StartWorker if state.session == crfty_core::SessionState::Running => {
                if !reconciled.contains(&Effect::StartWorker) {
                    reconciled.push(Effect::StartWorker);
                }
            }
            Effect::StartWorker => {}
            Effect::WriteSettings { .. } => {}
            Effect::KillActiveRun { .. }
            | Effect::VendorInstall
            | Effect::VendorCheck
            | Effect::StopDriver => {
                if !reconciled.contains(&effect) {
                    reconciled.push(effect);
                }
            }
        }
    }
    reconciled
}

fn fail_batch(
    applied_batch: Vec<(mpsc::SyncSender<Reply>, crfty_core::Applied)>,
    events: &mpsc::Sender<DriverEvent>,
    error: JournalError,
) {
    let message = format!("durable driver failure: {error}");
    for (reply, _applied) in applied_batch {
        let _result = reply.send(Reply::DurabilityUnknown {
            reason: message.clone(),
        });
    }
    let _result = events.send(DriverEvent::Fatal { message });
}

fn emit_latest_telemetry(
    state: &AppState,
    events: &mpsc::Sender<DriverEvent>,
    telemetry: &Mutex<BTreeMap<RunId, Telemetry>>,
) {
    let updates = {
        let mut slot = match telemetry.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        std::mem::take(&mut *slot)
    };
    for (run_id, update) in updates {
        if run_is_active(state, run_id) {
            let _result = events.send(DriverEvent::Ephemeral(EphemeralDelta::Telemetry(update)));
        }
    }
}

fn run_is_active(state: &AppState, run_id: RunId) -> bool {
    state.durable.queue.iter().any(|item| {
        matches!(
            item.state,
            QueueItemState::Claimed { run_id: active, .. }
                | QueueItemState::Running { run_id: active, .. }
                if active == run_id
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{io, path::PathBuf, sync::mpsc};

    use crfty_core::{
        AnalysisDelta, AnalysisIntent, AnalysisProfile, AnalysisSnapshot, ClaimId, Command,
        ConfigDelta, DurableDelta, Effect, EphemeralDelta, ExecutionSettings, ItemOutcome,
        JobPhase, JobProgress, Operation, OutputTarget, OverwriteDecision, QueueAddRequest,
        QueueCommand, QueueItemId, Reply, RunId, SessionCommand, SessionState, Settings,
        SettingsCommand, SystemCommand, Telemetry, ToolAvailability, ToolRevisions, UnixMillis,
        WorkerCommand, apply,
    };

    fn add_command(id: u64, input: &str) -> Command {
        Command::Queue(QueueCommand::AddMany {
            requests: vec![QueueAddRequest {
                item_id: QueueItemId(id),
                input: PathBuf::from(input),
                path_hash: None,
                stamp: None,
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                overwrite: OverwriteDecision::FollowSettings,
            }],
        })
    }

    use super::{
        AppState, CompactionOutcome, ConfigSink, DriverEvent, DriverPersistence, Envelope,
        JournalError, JournalSink, emit_batch, maybe_compact, process_batch, reconcile_effects,
        split_batch_at_settings,
    };
    use crate::{
        config::ConfigStore,
        journal::{DurabilityToken, JournalWriter},
        lock::DataLock,
        sentinel::CrashSentinel,
    };

    struct FailingJournal;
    struct AcceptingConfig;

    impl ConfigSink for AcceptingConfig {
        fn write(&mut self, _settings: &Settings) -> Result<(), String> {
            Ok(())
        }
    }

    impl JournalSink for FailingJournal {
        fn append(
            &mut self,
            _deltas: &[crfty_core::DurableDelta],
        ) -> Result<DurabilityToken, JournalError> {
            Err(JournalError::injected(io::Error::other("sync failed")))
        }

        fn recover(&mut self, _state: &crfty_core::DurableState) -> Result<(), JournalError> {
            Err(JournalError::injected(io::Error::other("recover failed")))
        }
    }

    #[test]
    fn journal_failure_emits_no_durable_delta_and_stops_driver() {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command: add_command(1, "video.mkv"),
            reply: reply_tx,
        };
        let (event_tx, event_rx) = mpsc::channel();
        let mut state = AppState::default();
        let stopped = process_batch(
            &mut state,
            &mut FailingJournal,
            &mut AcceptingConfig,
            &mut None,
            vec![envelope],
            &event_tx,
            None,
        )
        .should_stop;
        assert!(stopped);
        assert!(matches!(
            reply_rx.recv().expect("failure reply"),
            Reply::DurabilityUnknown { .. }
        ));
        assert!(matches!(
            event_rx.recv().expect("fatal event"),
            DriverEvent::Fatal { .. }
        ));
        assert!(event_rx.try_recv().is_err());
    }

    struct NoopJournal;

    impl JournalSink for NoopJournal {
        fn append(
            &mut self,
            _deltas: &[crfty_core::DurableDelta],
        ) -> Result<DurabilityToken, JournalError> {
            Ok(DurabilityToken::new())
        }

        fn recover(&mut self, _state: &crfty_core::DurableState) -> Result<(), JournalError> {
            Ok(())
        }
    }

    #[test]
    fn standing_analysis_publishes_after_durable_facts_from_the_same_command() {
        let mut state = AppState::default();
        let mut applied = apply(&mut state, add_command(1, "video.mkv"));
        applied
            .ephemeral
            .push(EphemeralDelta::Analysis(AnalysisDelta::Reset {
                snapshot: Box::new(AnalysisSnapshot::default()),
            }));
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let (event_tx, event_rx) = mpsc::channel();

        assert!(!emit_batch(
            Some(DurabilityToken::new()),
            vec![(reply_tx, applied)],
            &state,
            &event_tx,
            None,
        ));
        assert_eq!(reply_rx.recv().expect("reply"), Reply::Accepted);
        let observed: Vec<&'static str> = event_rx
            .try_iter()
            .map(|event| match event {
                DriverEvent::Ephemeral(EphemeralDelta::QueueAddSummary { .. }) => "summary",
                DriverEvent::Durable(DurableDelta::QueueAdded { .. }) => "durable",
                DriverEvent::Ephemeral(EphemeralDelta::Analysis(_)) => "analysis",
                _ => "other",
            })
            .collect();
        assert_eq!(observed, ["summary", "durable", "analysis"]);
    }

    #[test]
    fn terminal_orders_progress_before_and_aggregates_after_the_durable_finish() {
        let execution = {
            let mut profile = AnalysisProfile::production();
            profile.ab_av1_revision = "fixture".to_owned();
            profile.ffmpeg_revision = "fixture".to_owned();
            profile.encoder_revision = "fixture".to_owned();
            ExecutionSettings::production(profile, false)
        };
        let mut state = AppState::default();
        for command in [
            add_command(1, "video.mkv"),
            Command::System(SystemCommand::ToolsDiscovered {
                availability: ToolAvailability::Available {
                    source: crfty_core::ToolSource::System,
                    revisions: ToolRevisions {
                        ab_av1: "fixture".to_owned(),
                        ffmpeg: "fixture".to_owned(),
                        encoder: "fixture".to_owned(),
                    },
                },
                update_available: false,
            }),
            Command::Session(SessionCommand::Start),
            Command::Worker(WorkerCommand::ReserveNext {
                claim_id: ClaimId(2),
                run_id: RunId(3),
            }),
            Command::Worker(WorkerCommand::PrepareReserved {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
                observation: None,
                import_paths: Vec::new(),
                execution,
            }),
        ] {
            let applied = apply(&mut state, command);
            assert!(!matches!(applied.reply, Reply::Rejected { .. }));
        }

        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command: Command::Worker(WorkerCommand::Terminal {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
                outcome: ItemOutcome::Failed(crfty_core::FailureFacts::new(
                    crfty_core::FailureKind::Internal,
                    "fixture",
                )),
                at: UnixMillis(1_000),
                phase_spans: Vec::new(),
                final_telemetry: Some(Telemetry {
                    run_id: RunId(3),
                    sequence: 7,
                    phase: JobPhase::Finalizing,
                    progress: JobProgress::OutputPositionMs(100),
                    fps_centi: None,
                    eta_ms: None,
                }),
            }),
            reply: reply_tx,
        };
        let (event_tx, event_rx) = mpsc::channel();
        assert!(
            !process_batch(
                &mut state,
                &mut NoopJournal,
                &mut AcceptingConfig,
                &mut None,
                vec![envelope],
                &event_tx,
                None,
            )
            .should_stop
        );
        assert_eq!(reply_rx.recv().expect("terminal reply"), Reply::Accepted);
        let observed: Vec<&'static str> = event_rx
            .try_iter()
            .map(|event| match event {
                DriverEvent::Ephemeral(EphemeralDelta::Telemetry(_)) => "telemetry",
                DriverEvent::Ephemeral(EphemeralDelta::TelemetryCleared { .. }) => "cleared",
                DriverEvent::Ephemeral(EphemeralDelta::SessionAggregates(_)) => "aggregates",
                DriverEvent::Durable(DurableDelta::ItemFinished { .. }) => "finished",
                _ => "other",
            })
            .collect();
        assert_eq!(observed, ["telemetry", "cleared", "finished", "aggregates"]);
    }

    struct RecordingConfig {
        writes: Vec<Settings>,
        failure: Option<String>,
    }

    impl ConfigSink for RecordingConfig {
        fn write(&mut self, settings: &Settings) -> Result<(), String> {
            self.writes.push(settings.clone());
            self.failure.clone().map_or(Ok(()), Err)
        }
    }

    #[test]
    fn settings_are_written_once_before_the_coalesced_config_event() {
        let first = Settings {
            hardware_decode: false,
            ..Settings::default()
        };
        let mut second = first.clone();
        second.output.overwrite_existing = true;
        let (first_reply_tx, first_reply_rx) = mpsc::sync_channel(1);
        let (second_reply_tx, second_reply_rx) = mpsc::sync_channel(1);
        let batch = vec![
            Envelope {
                command: Command::Settings(SettingsCommand::Set { settings: first }),
                reply: first_reply_tx,
            },
            Envelope {
                command: Command::Settings(SettingsCommand::Set {
                    settings: second.clone(),
                }),
                reply: second_reply_tx,
            },
        ];
        let (event_tx, event_rx) = mpsc::channel();
        let mut state = AppState::default();
        let mut config = RecordingConfig {
            writes: Vec::new(),
            failure: None,
        };
        assert!(
            !process_batch(
                &mut state,
                &mut NoopJournal,
                &mut config,
                &mut None,
                batch,
                &event_tx,
                None,
            )
            .should_stop
        );
        assert_eq!(config.writes, vec![second.clone()]);
        assert_eq!(state.settings, second.clone());
        assert_eq!(first_reply_rx.recv().expect("first reply"), Reply::Accepted);
        assert_eq!(
            second_reply_rx.recv().expect("second reply"),
            Reply::Accepted
        );
        assert_eq!(
            event_rx.recv().expect("config event"),
            DriverEvent::Config(ConfigDelta::SettingsChanged { settings: second })
        );
        assert!(event_rx.try_recv().is_err());
    }

    #[test]
    fn settings_write_failure_rolls_back_state_and_emits_no_config_delta() {
        let changed = Settings {
            hardware_decode: false,
            ..Settings::default()
        };
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command: Command::Settings(SettingsCommand::Set { settings: changed }),
            reply: reply_tx,
        };
        let (event_tx, event_rx) = mpsc::channel();
        let mut state = AppState::default();
        let mut config = RecordingConfig {
            writes: Vec::new(),
            failure: Some("config sync failed".to_owned()),
        };
        assert!(
            !process_batch(
                &mut state,
                &mut NoopJournal,
                &mut config,
                &mut None,
                vec![envelope],
                &event_tx,
                None,
            )
            .should_stop
        );
        assert_eq!(state.settings, Settings::default());
        assert!(matches!(
            reply_rx.recv().expect("rejected reply"),
            Reply::Rejected { .. }
        ));
        assert!(matches!(
            event_rx.recv().expect("rejection event"),
            DriverEvent::Ephemeral(crfty_core::EphemeralDelta::CommandRejected { .. })
        ));
        assert!(event_rx.try_recv().is_err());
    }

    #[test]
    fn import_batches_flag_a_forced_compaction() {
        let record = crfty_core::ImportedHistoryRecord {
            status: crfty_core::ParkedStatus::Scanned,
            size: None,
            modified_ns: None,
            video_codec: None,
            width: None,
            height: None,
            duration_ms: None,
            output_size: None,
            encoding_time: None,
            crf: None,
            vmaf: None,
            target: None,
            requested_target: None,
            floor_target: None,
            decided_at: UnixMillis(1),
        };
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command: Command::History(crfty_core::HistoryCommand::Import {
                records: vec![(
                    crfty_core::ImportPath("c:/videos/movie.mkv".to_owned()),
                    record,
                )],
            }),
            reply: reply_tx,
        };
        let (event_tx, _event_rx) = mpsc::channel();
        let mut state = AppState::default();
        let outcome = process_batch(
            &mut state,
            &mut NoopJournal,
            &mut AcceptingConfig,
            &mut None,
            vec![envelope],
            &event_tx,
            None,
        );
        assert!(!outcome.should_stop);
        assert!(outcome.imported_history);
        assert_eq!(
            reply_rx.recv().expect("import reply"),
            Reply::Imported {
                parked: 1,
                skipped: 0
            }
        );
        // An ordinary durable batch does not schedule a forced compaction.
        let (reply_tx, _reply_rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command: add_command(1, "video.mkv"),
            reply: reply_tx,
        };
        let outcome = process_batch(
            &mut state,
            &mut NoopJournal,
            &mut AcceptingConfig,
            &mut None,
            vec![envelope],
            &event_tx,
            None,
        );
        assert!(!outcome.imported_history);
    }

    #[test]
    fn effect_reconciliation_suppresses_obsolete_start() {
        let state = AppState {
            session: SessionState::StopAfterCurrent,
            ..AppState::default()
        };
        assert!(reconcile_effects(vec![Effect::StartWorker], &state).is_empty());
    }

    fn add_item(state: &mut AppState, id: u64) -> Vec<DurableDelta> {
        let applied = apply(state, add_command(id, &format!("video-{id}.mkv")));
        assert_eq!(applied.reply, Reply::Accepted);
        applied.durable
    }

    #[test]
    fn forced_compaction_rewrites_only_when_quiescent_and_not_degraded() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let journal_path = directory.path().join("state.jsonl");
        let (mut journal, _replay) = JournalWriter::open(&journal_path).expect("journal writer");
        let mut state = AppState::default();
        let durable = add_item(&mut state, 1);
        journal.append_batch(&durable).expect("journal append");
        let mut persistence = DriverPersistence {
            journal,
            config: ConfigStore::new(directory.path().join("config.json")),
            _lock: DataLock::acquire(directory.path()).expect("data lock"),
            sentinel: CrashSentinel::arm(directory.path()),
        };
        let before = std::fs::read(&journal_path).expect("journal bytes");

        // Degraded skips compaction even when forced; the corrupt journal is
        // evidence and must stay byte-identical.
        assert_eq!(
            maybe_compact(&state, &mut persistence, true, true),
            CompactionOutcome::Skipped
        );
        assert_eq!(std::fs::read(&journal_path).expect("journal bytes"), before);
        // A non-quiescent state (active session) also skips a forced request.
        state.session = SessionState::Running;
        assert_eq!(
            maybe_compact(&state, &mut persistence, false, true),
            CompactionOutcome::Skipped
        );
        assert_eq!(std::fs::read(&journal_path).expect("journal bytes"), before);
        // Below the size floor, an unforced idle tick leaves the journal alone.
        state.session = SessionState::Idle;
        assert_eq!(
            maybe_compact(&state, &mut persistence, false, false),
            CompactionOutcome::Skipped
        );
        assert_eq!(std::fs::read(&journal_path).expect("journal bytes"), before);

        // Quiescent + forced compacts to a single snapshot head line.
        assert_eq!(
            maybe_compact(&state, &mut persistence, false, true),
            CompactionOutcome::Compacted
        );
        let bytes = std::fs::read(&journal_path).expect("compacted journal");
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
        let replayed = crfty_core::replay(&bytes);
        assert!(replayed.corruption.is_none());
        assert_eq!(replayed.state, state.durable);
        assert_eq!(replayed.next_sequence.0, 1);

        // Appends continue on the new generation with unbroken numbering.
        let durable = add_item(&mut state, 2);
        persistence
            .journal
            .append_batch(&durable)
            .expect("append after compaction");
        let replayed = crfty_core::replay(&std::fs::read(&journal_path).expect("appended journal"));
        assert!(replayed.corruption.is_none());
        assert_eq!(replayed.state.queue.len(), 2);
        assert_eq!(replayed.next_sequence.0, 2);
    }

    #[test]
    fn acknowledgement_requires_the_matching_signature_and_recovers_in_place() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let journal_path = directory.path().join("state.jsonl");
        let mut healthy_state = AppState::default();
        let durable = add_item(&mut healthy_state, 1);
        {
            let (mut journal, _replay) =
                JournalWriter::open(&journal_path).expect("journal writer");
            journal.append_batch(&durable).expect("journal append");
        }
        let mut corrupt_bytes = std::fs::read(&journal_path).expect("journal bytes");
        corrupt_bytes.extend(b"not-json\n");
        std::fs::write(&journal_path, &corrupt_bytes).expect("corrupt journal");

        let (mut journal, replay) = JournalWriter::open(&journal_path).expect("corrupt open");
        let corruption = replay.corruption.expect("corruption report");
        let mut degraded = Some(crfty_core::CorruptionReport {
            reason: corruption.reason.clone(),
            signature: corruption.signature.clone(),
        });
        let mut state = AppState {
            durable: replay.state,
            ..AppState::default()
        };
        let envelope = |signature| {
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            (
                Envelope {
                    command: Command::System(SystemCommand::AcknowledgeCorruption { signature }),
                    reply: reply_tx,
                },
                reply_rx,
            )
        };

        // A stale signature never touches the file: the operator consented to
        // discarding different bytes than the ones on disk.
        let (wrong, wrong_reply) = envelope(crfty_core::corruption_signature(b"different tail"));
        let (event_tx, event_rx) = mpsc::channel();
        assert!(
            !process_batch(
                &mut state,
                &mut journal,
                &mut AcceptingConfig,
                &mut degraded,
                vec![wrong],
                &event_tx,
                None,
            )
            .should_stop
        );
        assert!(matches!(
            wrong_reply.recv().expect("wrong-signature reply"),
            Reply::Rejected { .. }
        ));
        assert!(degraded.is_some());
        assert_eq!(
            std::fs::read(&journal_path).expect("journal bytes"),
            corrupt_bytes
        );

        // The matching signature archives the corrupt file, compacts the
        // valid prefix, clears degraded, and announces recovery last.
        let (matching, matching_reply) = envelope(corruption.signature);
        assert!(
            !process_batch(
                &mut state,
                &mut journal,
                &mut AcceptingConfig,
                &mut degraded,
                vec![matching],
                &event_tx,
                None,
            )
            .should_stop
        );
        assert_eq!(
            matching_reply.recv().expect("matching reply"),
            Reply::Accepted
        );
        assert!(degraded.is_none());
        let events: Vec<DriverEvent> = event_rx.try_iter().collect();
        assert!(matches!(events.last(), Some(DriverEvent::Recovered)));
        let recovered = std::fs::read(&journal_path).expect("recovered journal");
        let replayed = crfty_core::replay(&recovered);
        assert!(replayed.corruption.is_none());
        assert_eq!(replayed.state, state.durable);
        let archived: Vec<_> = std::fs::read_dir(directory.path())
            .expect("data directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
            .collect();
        assert_eq!(archived.len(), 1);
        assert_eq!(
            std::fs::read(archived[0].path()).expect("archive bytes"),
            corrupt_bytes
        );

        // Mutation is accepted again on the fresh generation.
        let durable = add_item(&mut state, 2);
        journal
            .append_batch(&durable)
            .expect("append after recovery");
        let replayed = crfty_core::replay(&std::fs::read(&journal_path).expect("appended journal"));
        assert!(replayed.corruption.is_none());
        assert_eq!(replayed.state.queue.len(), 2);
    }

    #[test]
    fn acknowledgement_is_rejected_while_healthy() {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command: Command::System(SystemCommand::AcknowledgeCorruption {
                signature: crfty_core::corruption_signature(b"anything"),
            }),
            reply: reply_tx,
        };
        let (event_tx, event_rx) = mpsc::channel();
        let mut state = AppState::default();
        assert!(
            !process_batch(
                &mut state,
                &mut NoopJournal,
                &mut AcceptingConfig,
                &mut None,
                vec![envelope],
                &event_tx,
                None,
            )
            .should_stop
        );
        assert!(matches!(
            reply_rx.recv().expect("healthy-ack reply"),
            Reply::Rejected { .. }
        ));
        assert!(matches!(
            event_rx.recv().expect("rejection event"),
            DriverEvent::Ephemeral(EphemeralDelta::CommandRejected { .. })
        ));
        assert!(
            !event_rx
                .try_iter()
                .any(|event| matches!(event, DriverEvent::Recovered))
        );
    }

    #[test]
    fn settings_form_ordered_batch_barriers_but_consecutive_writes_coalesce() {
        let envelope = |command| {
            let (reply, _receiver) = mpsc::sync_channel(1);
            Envelope { command, reply }
        };
        let settings = Settings::default();
        let groups = split_batch_at_settings(vec![
            envelope(add_command(1, "one.mkv")),
            envelope(Command::Settings(SettingsCommand::Set {
                settings: settings.clone(),
            })),
            envelope(Command::Settings(SettingsCommand::Set { settings })),
            envelope(Command::Session(crfty_core::SessionCommand::Start)),
        ]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[1].len(), 2);
        assert_eq!(groups[2].len(), 1);
    }
}
