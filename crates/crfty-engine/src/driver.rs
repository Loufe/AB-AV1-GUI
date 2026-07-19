use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use crfty_core::{
    AppState, Command, DurableDelta, DurableState, Effect, EphemeralDelta, QueueItemState, Reply,
    RunId, SystemCommand, Telemetry, apply,
};

use crate::journal::{DurabilityToken, JournalError, JournalWriter};

const DRIVER_CHANNEL_CAPACITY: usize = 64;
const COMMAND_REPLY_CHANNEL_CAPACITY: usize = 0;
const DRIVER_TICK: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverEvent {
    Snapshot(DurableState),
    Durable(DurableDelta),
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
    pub fn start(journal_path: impl AsRef<Path>) -> Result<Self, DriverStartError> {
        Self::start_inner(journal_path, None)
    }

    pub(crate) fn start_with_effects(
        journal_path: impl AsRef<Path>,
        effects: mpsc::Sender<Effect>,
    ) -> Result<Self, DriverStartError> {
        Self::start_inner(journal_path, Some(effects))
    }

    fn start_inner(
        journal_path: impl AsRef<Path>,
        effect_sink: Option<mpsc::Sender<Effect>>,
    ) -> Result<Self, DriverStartError> {
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
                    writer,
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
    mut writer: JournalWriter,
    degraded: Option<String>,
    receiver: mpsc::Receiver<Envelope>,
    events: mpsc::Sender<DriverEvent>,
    telemetry: Arc<Mutex<BTreeMap<RunId, Telemetry>>>,
    effect_sink: Option<mpsc::Sender<Effect>>,
) {
    let _result = events.send(DriverEvent::Snapshot(state.durable.clone()));
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
        let should_stop = process_batch(
            &mut state,
            &mut writer,
            degraded.as_deref(),
            batch,
            &events,
            effect_sink.as_ref(),
        );
        if should_stop {
            break;
        }
    }
}

fn process_batch(
    state: &mut AppState,
    writer: &mut impl JournalSink,
    degraded: Option<&str>,
    batch: Vec<Envelope>,
    events: &mpsc::Sender<DriverEvent>,
    effect_sink: Option<&mpsc::Sender<Effect>>,
) -> bool {
    let mut durable = Vec::new();
    let mut applied_batch = Vec::with_capacity(batch.len());
    for envelope in batch {
        let applied = if let Some(reason) = degraded {
            if matches!(envelope.command, Command::System(SystemCommand::Shutdown)) {
                apply(state, envelope.command)
            } else {
                crfty_core::Applied {
                    durable: Vec::new(),
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
    emit_batch(durability, applied_batch, state, events, effect_sink)
}

trait JournalSink {
    fn append(&mut self, deltas: &[DurableDelta]) -> Result<DurabilityToken, JournalError>;
}

impl JournalSink for JournalWriter {
    fn append(&mut self, deltas: &[DurableDelta]) -> Result<DurabilityToken, JournalError> {
        self.append_batch(deltas).map(|(_records, token)| token)
    }
}

fn emit_batch(
    durability: Option<DurabilityToken>,
    applied_batch: Vec<(mpsc::SyncSender<Reply>, crfty_core::Applied)>,
    state: &AppState,
    events: &mpsc::Sender<DriverEvent>,
    effect_sink: Option<&mpsc::Sender<Effect>>,
) -> bool {
    if let Some(token) = durability {
        emit_durable(token, &applied_batch, events);
    }
    let mut effects = Vec::new();
    for (reply, applied) in applied_batch {
        for delta in applied.ephemeral {
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
    _token: DurabilityToken,
    applied_batch: &[(mpsc::SyncSender<Reply>, crfty_core::Applied)],
    events: &mpsc::Sender<DriverEvent>,
) {
    for (_reply, applied) in applied_batch {
        for delta in &applied.durable {
            let _result = events.send(DriverEvent::Durable(delta.clone()));
        }
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
        Command, Effect, Operation, OutputTarget, QueueCommand, QueueItemId, Reply, SessionState,
    };

    use super::{
        AppState, DriverEvent, Envelope, JournalError, JournalSink, process_batch,
        reconcile_effects,
    };
    use crate::journal::DurabilityToken;

    struct FailingJournal;

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

    #[test]
    fn effect_reconciliation_suppresses_obsolete_start() {
        let state = AppState {
            session: SessionState::StopAfterCurrent,
            ..AppState::default()
        };
        assert!(reconcile_effects(vec![Effect::StartWorker], &state).is_empty());
    }
}
