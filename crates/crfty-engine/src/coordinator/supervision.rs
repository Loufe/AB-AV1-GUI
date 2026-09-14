//! The supervision loop: draining driver effects, owning the session worker
//! thread, re-running tool discovery, and the cancellation registry the
//! worker shares with force-stop and shutdown.

use std::{
    sync::{Arc, Mutex, atomic::AtomicU64, mpsc},
    thread,
};

use crfty_core::{Command, Effect, LocatedTools, Reply, RunId, SystemCommand, WorkerCommand};

use crate::{
    ab_av1::{AbAv1Runtime, CancelMode, CancellationHandle},
    driver::CommandSender,
    process_supervisor::ProcessCancellation,
    remux::RemuxCancellationHandle,
    tools::discovery,
};

use super::session::run_session;
use super::{EngineConfig, ToolsConfig, located_tools};

#[derive(Clone)]
pub(super) struct ActiveCancellation {
    state: Arc<Mutex<CancellationState>>,
}

pub(super) struct CancellationState {
    force_stopping: bool,
    slot: Option<(RunId, ActiveJobCancellation)>,
    /// The session-start capability probe while it runs. It has no run id:
    /// force-stop and shutdown cancel it unconditionally.
    probe: Option<ProcessCancellation>,
}

#[derive(Clone)]
pub(super) enum ActiveJobCancellation {
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
                probe: None,
            })),
        }
    }

    pub(super) fn register(
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

    /// Registers the session-start probe so force-stop and shutdown reach
    /// it; the returned guard unregisters on drop.
    pub(super) fn register_probe(
        &self,
        cancellation: &ProcessCancellation,
    ) -> ProbeRegistration<'_> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.probe = Some(cancellation.clone());
        if state.force_stopping {
            cancellation.cancel();
        }
        ProbeRegistration { cancellation: self }
    }

    fn clear_probe(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.probe = None;
    }

    fn force(&self, run_id: Option<RunId>) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.force_stopping = true;
        if let Some(probe) = state.probe.as_ref() {
            probe.cancel();
        }
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
        state.probe = None;
    }

    fn is_force_stopping(&self) -> bool {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.force_stopping
    }
}

pub(super) struct CancellationRegistration<'a> {
    cancellation: &'a ActiveCancellation,
    run_id: RunId,
}

impl Drop for CancellationRegistration<'_> {
    fn drop(&mut self) {
        self.cancellation.clear(self.run_id);
    }
}

pub(super) struct ProbeRegistration<'a> {
    cancellation: &'a ActiveCancellation,
}

impl Drop for ProbeRegistration<'_> {
    fn drop(&mut self) {
        self.cancellation.clear_probe();
    }
}

pub(super) fn supervise(
    effects: mpsc::Receiver<Effect>,
    commands: CommandSender,
    runtime: Arc<AbAv1Runtime>,
    config: EngineConfig,
    tools_slot: Arc<Mutex<Option<LocatedTools>>>,
    next_runtime_id: u64,
) {
    let cancellation = ActiveCancellation::new();
    let next_id = Arc::new(AtomicU64::new(next_runtime_id));
    let mut worker: Option<thread::JoinHandle<()>> = None;
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
            // Discovery is a handful of file-type checks, so it runs inline.
            // The slot is replaced before the reducer hears of the result, so
            // a session starting on the new report finds the new tools.
            Effect::DiscoverTools { configured } => {
                let ToolsConfig::Discover(environment) = &config.tools else {
                    continue;
                };
                let availability = discovery::discover(environment, &configured);
                {
                    let mut slot = match tools_slot.lock() {
                        Ok(slot) => slot,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    *slot = located_tools(&availability);
                }
                match commands.submit(Command::System(SystemCommand::ToolsDiscovered {
                    availability,
                })) {
                    Ok(Reply::Accepted) => {}
                    Ok(reply) => tracing::error!("tool rediscovery was not accepted: {reply:?}"),
                    Err(error) => tracing::error!("failed to report rediscovered tools: {error}"),
                }
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
                break;
            }
        }
    }
    // Reached on StopDriver (already force-flagged) or when the driver died
    // and the effect channel disconnected. Force-flag the worker again so
    // the join below is a winding-down wait, never an hours-long encode.
    cancellation.force(None);
    if let Some(worker) = worker
        && worker.join().is_err()
    {
        report_worker_crash(&commands, "session worker panicked during shutdown");
    }
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

#[cfg(test)]
mod tests {
    use crfty_core::RunId;

    use super::{ActiveCancellation, ActiveJobCancellation};
    use crate::{
        ab_av1::{CancelMode, CancellationHandle},
        process_supervisor::ProcessCancellation,
    };

    #[test]
    fn force_before_registration_cannot_miss_the_child() {
        let cancellation = ActiveCancellation::new();
        cancellation.force(None);
        let (handle, receiver) = CancellationHandle::fixture();
        let _registration = cancellation.register(RunId(7), ActiveJobCancellation::AbAv1(handle));
        assert_eq!(*receiver.borrow(), Some(CancelMode::Force));
    }

    #[test]
    fn force_reaches_a_running_probe_and_one_registered_afterwards() {
        let cancellation = ActiveCancellation::new();
        let running = ProcessCancellation::new();
        let registration = cancellation.register_probe(&running);
        assert!(!running.is_cancelled());
        cancellation.force(None);
        assert!(running.is_cancelled());
        drop(registration);

        let late = ProcessCancellation::new();
        let _late_registration = cancellation.register_probe(&late);
        assert!(late.is_cancelled());

        cancellation.reset();
        let fresh = ProcessCancellation::new();
        let _fresh_registration = cancellation.register_probe(&fresh);
        assert!(!fresh.is_cancelled());
    }
}
