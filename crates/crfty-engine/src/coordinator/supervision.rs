//! The supervision loop: draining driver effects, owning the session and
//! vendor worker threads, and the cancellation registry those threads share.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crfty_core::{Command, Effect, Reply, RunId, WorkerCommand};

use crate::{
    ab_av1::{AbAv1Runtime, CancelMode, CancellationHandle},
    driver::CommandSender,
    remux::RemuxCancellationHandle,
    vendor::discovery::CurrentTools,
};

use super::EngineConfig;
use super::session::run_session;
use super::vendor_task::{VendorTask, spawn_vendor_worker};

/// How long shutdown waits for the vendor worker to observe cancellation and
/// unwind through its own staging cleanup before abandoning the thread to
/// process exit (#33 §12). Cancellation is observed between download chunks,
/// so anything slower than this is a wedged network read.
const VENDOR_SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

const VENDOR_SHUTDOWN_POLL: Duration = Duration::from_millis(25);

#[derive(Clone)]
pub(super) struct ActiveCancellation {
    state: Arc<Mutex<CancellationState>>,
}

pub(super) struct CancellationState {
    force_stopping: bool,
    slot: Option<(RunId, ActiveJobCancellation)>,
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

pub(super) struct CancellationRegistration<'a> {
    cancellation: &'a ActiveCancellation,
    run_id: RunId,
}

impl Drop for CancellationRegistration<'_> {
    fn drop(&mut self) {
        self.cancellation.clear(self.run_id);
    }
}

pub(super) fn supervise(
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crfty_core::RunId;

    use super::{ActiveCancellation, ActiveJobCancellation, join_within};
    use crate::ab_av1::{CancelMode, CancellationHandle};

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
