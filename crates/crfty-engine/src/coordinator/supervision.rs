//! The supervision loop: draining driver effects, owning the session worker
//! thread, re-running tool discovery, and the cancellation registry the
//! worker shares with force-stop and shutdown.

use std::{
    sync::{Arc, Mutex, MutexGuard, mpsc},
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

/// The cancellation registry Force Stop reaches into. One run is active at a
/// time; its scope owns the probe cancellation every ffprobe in the run
/// answers to, and the adapter handle for whichever encode, search, or
/// remux is in flight. A force flag latched before a scope opens cancels the
/// scope on entry, so no run can slip in unnoticed.
#[derive(Clone)]
pub(super) struct ActiveCancellation {
    state: Arc<Mutex<CancellationState>>,
}

pub(super) struct CancellationState {
    force_stopping: bool,
    active: Option<ActiveRun>,
    probe: Option<ProcessCancellation>,
}

struct ActiveRun {
    run_id: RunId,
    probes: ProcessCancellation,
    adapter: Option<ActiveJobCancellation>,
}

impl ActiveRun {
    fn cancel(&self) {
        self.probes.cancel();
        if let Some(adapter) = &self.adapter {
            adapter.cancel();
        }
    }
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
                active: None,
                probe: None,
            })),
        }
    }

    /// Opens the cancellation scope for one run, from its reservation through
    /// its terminal outcome. The returned scope hands out the probe
    /// cancellation; there is no other way to obtain one inside a session.
    pub(super) fn begin_run(&self, run_id: RunId) -> RunScope<'_> {
        let probes = ProcessCancellation::new();
        let mut state = self.lock();
        if state.force_stopping {
            probes.cancel();
        }
        state.active = Some(ActiveRun {
            run_id,
            probes: probes.clone(),
            adapter: None,
        });
        RunScope {
            cancellation: self,
            run_id,
            probes,
        }
    }

    fn end_run(&self, run_id: RunId) {
        let mut state = self.lock();
        if state
            .active
            .as_ref()
            .is_some_and(|run| run.run_id == run_id)
        {
            state.active = None;
        }
    }

    fn set_adapter(&self, run_id: RunId, handle: Option<ActiveJobCancellation>) {
        let mut state = self.lock();
        let force_stopping = state.force_stopping;
        if let Some(run) = state.active.as_mut()
            && run.run_id == run_id
        {
            run.adapter = handle;
            if force_stopping {
                run.cancel();
            }
        }
    }

    /// Registers the session-start probe so force-stop and shutdown reach
    /// it; the returned guard unregisters on drop.
    pub(super) fn register_probe(
        &self,
        cancellation: &ProcessCancellation,
    ) -> ProbeRegistration<'_> {
        let mut state = self.lock();
        state.probe = Some(cancellation.clone());
        if state.force_stopping {
            cancellation.cancel();
        }
        ProbeRegistration { cancellation: self }
    }

    fn clear_probe(&self) {
        let mut state = self.lock();
        state.probe = None;
    }

    fn force(&self, run_id: Option<RunId>) {
        let mut state = self.lock();
        state.force_stopping = true;
        if let Some(probe) = &state.probe {
            probe.cancel();
        }
        if let Some(run) = state.active.as_ref()
            && run_id.is_none_or(|expected| expected == run.run_id)
        {
            run.cancel();
        }
    }

    fn reset(&self) {
        let mut state = self.lock();
        state.force_stopping = false;
        state.active = None;
        state.probe = None;
    }

    fn is_force_stopping(&self) -> bool {
        self.lock().force_stopping
    }

    fn lock(&self) -> MutexGuard<'_, CancellationState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The active run's cancellation scope. Dropping it closes the run.
pub(super) struct RunScope<'a> {
    cancellation: &'a ActiveCancellation,
    run_id: RunId,
    probes: ProcessCancellation,
}

impl RunScope<'_> {
    /// The cancellation every supervised probe in this run must be given.
    pub(super) fn probes(&self) -> &ProcessCancellation {
        &self.probes
    }

    /// Registers the in-flight adapter job; the registration lasts until
    /// the returned guard drops.
    pub(super) fn register_adapter(
        &self,
        handle: ActiveJobCancellation,
    ) -> AdapterRegistration<'_> {
        self.cancellation.set_adapter(self.run_id, Some(handle));
        AdapterRegistration {
            cancellation: self.cancellation,
            run_id: self.run_id,
        }
    }
}

impl Drop for RunScope<'_> {
    fn drop(&mut self) {
        self.cancellation.end_run(self.run_id);
    }
}

pub(super) struct AdapterRegistration<'a> {
    cancellation: &'a ActiveCancellation,
    run_id: RunId,
}

impl Drop for AdapterRegistration<'_> {
    fn drop(&mut self) {
        self.cancellation.set_adapter(self.run_id, None);
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
) {
    let cancellation = ActiveCancellation::new();
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
            Effect::KillActiveRun { run_id } => cancellation.force(run_id),
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
    fn session_probe_registration_observes_latched_cancellation() {
        let cancellation = ActiveCancellation::new();
        let probe = ProcessCancellation::new();
        let registration = cancellation.register_probe(&probe);
        cancellation.force(None);
        assert!(probe.is_cancelled());
        drop(registration);
        let late = ProcessCancellation::new();
        let late_registration = cancellation.register_probe(&late);
        assert!(late.is_cancelled());
        drop(late_registration);
        cancellation.reset();
        let fresh = ProcessCancellation::new();
        let _fresh_registration = cancellation.register_probe(&fresh);
        assert!(!fresh.is_cancelled());
    }

    #[test]
    fn force_before_registration_cannot_miss_the_child() {
        let cancellation = ActiveCancellation::new();
        cancellation.force(None);
        let run = cancellation.begin_run(RunId(7));
        assert!(run.probes().is_cancelled());
        let (handle, receiver) = CancellationHandle::fixture();
        let _registration = run.register_adapter(ActiveJobCancellation::AbAv1(handle));
        assert_eq!(*receiver.borrow(), Some(CancelMode::Force));
    }

    #[test]
    fn force_reaches_the_active_run_probes_and_adapter_and_every_later_scope() {
        let cancellation = ActiveCancellation::new();
        let run = cancellation.begin_run(RunId(3));
        let (handle, receiver) = CancellationHandle::fixture();
        let registration = run.register_adapter(ActiveJobCancellation::AbAv1(handle));
        assert!(!run.probes().is_cancelled());

        cancellation.force(Some(RunId(3)));
        assert!(run.probes().is_cancelled());
        assert_eq!(*receiver.borrow(), Some(CancelMode::Force));

        // A probe that runs after the adapter registration is released, such
        // as output verification, still sees the same cancelled signal.
        drop(registration);
        assert!(run.probes().is_cancelled());
        drop(run);
        assert!(cancellation.begin_run(RunId(4)).probes().is_cancelled());
    }

    #[test]
    fn force_for_another_run_leaves_the_active_run_alone() {
        let cancellation = ActiveCancellation::new();
        let run = cancellation.begin_run(RunId(3));
        cancellation.force(Some(RunId(9)));
        assert!(!run.probes().is_cancelled());
    }

    #[test]
    fn reset_clears_the_latch_for_the_next_worker() {
        let cancellation = ActiveCancellation::new();
        cancellation.force(None);
        cancellation.reset();
        assert!(!cancellation.begin_run(RunId(1)).probes().is_cancelled());
    }
}
