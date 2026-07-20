use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use crfty_core::{
    AppSnapshot, AppState, Command, ConfigDelta, DurableDelta, Effect, EphemeralDelta,
    QueueItemState, Reply, RunId, Settings, SystemCommand, Telemetry, apply,
};

use crate::{
    config::ConfigStore,
    journal::{DurabilityToken, JournalError, JournalWriter},
};

const DRIVER_CHANNEL_CAPACITY: usize = 64;
const COMMAND_REPLY_CHANNEL_CAPACITY: usize = 0;
const DRIVER_TICK: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverEvent {
    Snapshot(AppSnapshot),
    Durable(DurableDelta),
    Config(ConfigDelta),
    Ephemeral(EphemeralDelta),
    Effect(Effect),
    Degraded { reason: String },
    Fatal { message: String },
}

#[derive(Debug)]
pub struct DriverStartError(String);

impl fmt::Display for DriverStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
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
        let config = ConfigStore::new(config_path.as_ref().to_path_buf());
        let loaded = config
            .load()
            .map_err(|error| DriverStartError(format!("failed to load settings: {error}")))?;
        let (writer, replay) = JournalWriter::open(journal_path)
            .map_err(|error| DriverStartError(format!("failed to start driver: {error}")))?;
        let degraded = replay.corruption.as_ref().map(|corruption| {
            format!(
                "journal is corrupt at byte {}: {}",
                corruption.offset, corruption.reason
            )
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
                    },
                    degraded,
                    command_rx,
                    event_tx,
                    driver_telemetry,
                    effect_sink,
                );
            })
            .map_err(|error| DriverStartError(format!("failed to spawn driver: {error}")))?;
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
                .map_err(|_| DriverStartError("driver thread panicked".to_owned()))?;
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
    degraded: Option<String>,
    receiver: mpsc::Receiver<Envelope>,
    events: mpsc::Sender<DriverEvent>,
    telemetry: Arc<Mutex<BTreeMap<RunId, Telemetry>>>,
    effect_sink: Option<mpsc::Sender<Effect>>,
) {
    let _result = events.send(DriverEvent::Snapshot(AppSnapshot {
        durable: state.durable.clone(),
        settings: state.settings.clone(),
    }));
    if let Some(reason) = &degraded {
        let _result = events.send(DriverEvent::Degraded {
            reason: reason.clone(),
        });
    }
    loop {
        let first = match receiver.recv_timeout(DRIVER_TICK) {
            Ok(envelope) => envelope,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                emit_latest_telemetry(&state, &events, &telemetry);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        emit_latest_telemetry(&state, &events, &telemetry);
        let mut batch = vec![first];
        batch.extend(receiver.try_iter());
        for batch in split_batch_at_settings(batch) {
            let should_stop = process_batch(
                &mut state,
                &mut persistence.journal,
                &mut persistence.config,
                degraded.as_deref(),
                batch,
                &events,
                effect_sink.as_ref(),
            );
            if should_stop {
                return;
            }
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

fn process_batch(
    state: &mut AppState,
    writer: &mut impl JournalSink,
    config: &mut impl ConfigSink,
    degraded: Option<&str>,
    batch: Vec<Envelope>,
    events: &mpsc::Sender<DriverEvent>,
    effect_sink: Option<&mpsc::Sender<Effect>>,
) -> bool {
    let settings_before = state.settings.clone();
    let mut durable = Vec::new();
    let mut applied_batch = Vec::with_capacity(batch.len());
    for envelope in batch {
        let applied = if let Some(reason) = degraded {
            // System commands (shutdown, tool availability) emit no durable
            // deltas, so they stay usable over a corrupt journal.
            if matches!(envelope.command, Command::System(_)) {
                apply(state, envelope.command)
            } else {
                crfty_core::Applied {
                    durable: Vec::new(),
                    config: Vec::new(),
                    ephemeral: vec![EphemeralDelta::CommandRejected {
                        reason: reason.to_owned(),
                    }],
                    effects: Vec::new(),
                    reply: Reply::Rejected {
                        reason: reason.to_owned(),
                    },
                }
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
                return true;
            }
        }
    };
    persist_settings(state, config, &settings_before, &mut applied_batch);
    emit_batch(durability, applied_batch, state, events, effect_sink)
}

trait ConfigSink {
    fn write(&mut self, settings: &Settings) -> Result<(), String>;
}

impl ConfigSink for ConfigStore {
    fn write(&mut self, settings: &Settings) -> Result<(), String> {
        ConfigStore::write(self, settings).map_err(|error| error.to_string())
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
                        Effect::StartWorker | Effect::KillActiveRun { .. } | Effect::StopDriver => {
                            None
                        }
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
}

impl JournalSink for JournalWriter {
    fn append(&mut self, deltas: &[DurableDelta]) -> Result<DurabilityToken, JournalError> {
        self.append_batch(deltas).map(|(_records, token)| token)
    }
}

/// Publishes each applied command in fold order as config, then ephemeral,
/// then durable deltas, followed by its reply. Ephemerals precede durables so
/// terminal state is never followed by stale progress: a finishing item is
/// observed as final telemetry, telemetry-cleared, then `ItemFinished`. A
/// future ephemeral that needs post-durable semantics would force revisiting
/// this contract. Durable publication still requires the token minted by the
/// journal fsync, so nothing observable can outrun durability.
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
        for delta in applied.ephemeral {
            let _result = events.send(DriverEvent::Ephemeral(delta));
        }
        if let Some(token) = &durability {
            emit_durable(token, &applied.durable, events);
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
            Effect::KillActiveRun { .. } | Effect::StopDriver => {
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
        AnalysisProfile, ClaimId, Command, ConfigDelta, DurableDelta, Effect, EphemeralDelta,
        ExecutionSettings, ItemOutcome, JobPhase, JobProgress, Operation, OutputTarget,
        QueueCommand, QueueItemId, Reply, RunId, SessionCommand, SessionState, Settings,
        SettingsCommand, SystemCommand, Telemetry, ToolAvailability, ToolRevisions, UnixMillis,
        WorkerCommand, apply,
    };

    use super::{
        AppState, ConfigSink, DriverEvent, Envelope, JournalError, JournalSink, process_batch,
        reconcile_effects, split_batch_at_settings,
    };
    use crate::journal::DurabilityToken;

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
    }

    #[test]
    fn journal_failure_emits_no_durable_delta_and_stops_driver() {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let envelope = Envelope {
            command: Command::Queue(QueueCommand::Add {
                item_id: QueueItemId(1),
                input: PathBuf::from("video.mkv"),
                operation: Operation::Convert,
                output_target: OutputTarget::Replace,
            }),
            reply: reply_tx,
        };
        let (event_tx, event_rx) = mpsc::channel();
        let mut state = AppState::default();
        let stopped = process_batch(
            &mut state,
            &mut FailingJournal,
            &mut AcceptingConfig,
            None,
            vec![envelope],
            &event_tx,
            None,
        );
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
    }

    #[test]
    fn terminal_publishes_ephemerals_before_the_durable_finish() {
        let execution = ExecutionSettings::production(
            AnalysisProfile::production(ToolRevisions {
                ab_av1: "fixture".to_owned(),
                ffmpeg: "fixture".to_owned(),
                encoder: "fixture".to_owned(),
            }),
            false,
        );
        let mut state = AppState::default();
        for command in [
            Command::Queue(QueueCommand::Add {
                item_id: QueueItemId(1),
                input: PathBuf::from("video.mkv"),
                operation: Operation::Convert,
                output_target: OutputTarget::Replace,
            }),
            Command::System(SystemCommand::ToolsDiscovered {
                availability: ToolAvailability::Available,
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
                }),
            }),
            reply: reply_tx,
        };
        let (event_tx, event_rx) = mpsc::channel();
        assert!(!process_batch(
            &mut state,
            &mut NoopJournal,
            &mut AcceptingConfig,
            None,
            vec![envelope],
            &event_tx,
            None,
        ));
        assert_eq!(reply_rx.recv().expect("terminal reply"), Reply::Accepted);
        let observed: Vec<&'static str> = event_rx
            .try_iter()
            .map(|event| match event {
                DriverEvent::Ephemeral(EphemeralDelta::Telemetry(_)) => "telemetry",
                DriverEvent::Ephemeral(EphemeralDelta::TelemetryCleared { .. }) => "cleared",
                DriverEvent::Durable(DurableDelta::ItemFinished { .. }) => "finished",
                _ => "other",
            })
            .collect();
        assert_eq!(observed, ["telemetry", "cleared", "finished"]);
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
        assert!(!process_batch(
            &mut state,
            &mut NoopJournal,
            &mut config,
            None,
            batch,
            &event_tx,
            None,
        ));
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
        assert!(!process_batch(
            &mut state,
            &mut NoopJournal,
            &mut config,
            None,
            vec![envelope],
            &event_tx,
            None,
        ));
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
    fn effect_reconciliation_suppresses_obsolete_start() {
        let state = AppState {
            session: SessionState::StopAfterCurrent,
            ..AppState::default()
        };
        assert!(reconcile_effects(vec![Effect::StartWorker], &state).is_empty());
    }

    #[test]
    fn settings_form_ordered_batch_barriers_but_consecutive_writes_coalesce() {
        let envelope = |command| {
            let (reply, _receiver) = mpsc::sync_channel(1);
            Envelope { command, reply }
        };
        let settings = Settings::default();
        let groups = split_batch_at_settings(vec![
            envelope(Command::Queue(QueueCommand::Add {
                item_id: QueueItemId(1),
                input: PathBuf::from("one.mkv"),
                operation: Operation::Convert,
                output_target: OutputTarget::Replace,
            })),
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
