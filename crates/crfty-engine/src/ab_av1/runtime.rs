use std::{
    future::Future,
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};

use super::{
    operation,
    types::{
        CancelMode, EncodeOutcome, EncodeRequest, JobReport, JobTerminal, RuntimeStartError,
        SearchOutcome, SearchRequest, ShutdownError, StartJobError, Telemetry, WaitError,
    },
};
use crate::vendor::discovery::MediaTools;

static RUNTIME_ACTIVE: AtomicBool = AtomicBool::new(false);
const RUNTIME_COMMAND_CAPACITY: usize = 1;
const RUNTIME_READY_CAPACITY: usize = 0;

#[cfg(feature = "contract-test-fixture")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FaultInjection {
    #[default]
    None,
    PanicAfterFirstProgress,
}

/// State of the single cancellation slot, scoped to one job's acquisition window.
///
/// A job acquires by setting `CancellationState::active` and only then installs
/// its `CancellationHandle`. A shutdown racing into that window would otherwise
/// snapshot an empty slot and silently lose the cancel. `CancelPending` records
/// that intent so the handle is cancelled the instant it is installed. The
/// active flag and slot share one mutex, so `finish_job` resets both atomically
/// and a pending cancel cannot leak into a later, unrelated job.
enum CancellationSlot {
    Idle,
    Installed(CancellationHandle),
    CancelPending,
}

struct RuntimeState {
    accepting: AtomicBool,
    cancellation: Mutex<CancellationState>,
}

struct CancellationState {
    active: bool,
    slot: CancellationSlot,
}

impl RuntimeState {
    fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            cancellation: Mutex::new(CancellationState {
                active: false,
                slot: CancellationSlot::Idle,
            }),
        }
    }

    fn lock_cancellation(&self) -> std::sync::MutexGuard<'_, CancellationState> {
        match self.cancellation.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn try_acquire(&self) -> bool {
        let mut state = self.lock_cancellation();
        if state.active {
            false
        } else {
            state.active = true;
            true
        }
    }

    fn finish_job(&self) {
        let mut state = self.lock_cancellation();
        state.active = false;
        state.slot = CancellationSlot::Idle;
    }

    /// Install the acquiring job's handle. If a cancel raced in ahead of the
    /// install (`CancelPending`), the freshly installed handle is cancelled at
    /// once so the intent is not lost.
    fn install_cancellation(&self, cancellation: &CancellationHandle) {
        let mut state = self.lock_cancellation();
        if matches!(&state.slot, CancellationSlot::CancelPending) {
            cancellation.cancel(CancelMode::Force);
        }
        state.slot = CancellationSlot::Installed(cancellation.clone());
    }

    fn cancel_active(&self) {
        let mut state = self.lock_cancellation();
        match &state.slot {
            CancellationSlot::Installed(cancellation) => cancellation.cancel(CancelMode::Force),
            // No handle installed yet: only record the intent while a job is
            // genuinely occupying its acquisition window (`active` is set
            // before the handle is installed and cleared atomically with the
            // slot in `finish_job`).
            // Without an active job there is nothing to cancel, and arming here
            // could otherwise cancel the next job that installs its handle.
            CancellationSlot::Idle if state.active => {
                state.slot = CancellationSlot::CancelPending;
            }
            CancellationSlot::Idle | CancellationSlot::CancelPending => {}
        }
    }
}

enum RuntimeCommand {
    Search {
        tools: MediaTools,
        request: SearchRequest,
        cancellation: tokio::sync::watch::Receiver<Option<CancelMode>>,
        telemetry: Arc<Mutex<Option<Telemetry>>>,
        result: mpsc::Sender<JobReport<SearchOutcome>>,
    },
    Encode {
        tools: MediaTools,
        request: EncodeRequest,
        cancellation: tokio::sync::watch::Receiver<Option<CancelMode>>,
        telemetry: Arc<Mutex<Option<Telemetry>>>,
        result: mpsc::Sender<JobReport<EncodeOutcome>>,
        #[cfg(feature = "contract-test-fixture")]
        fault: FaultInjection,
    },
    Shutdown,
}

pub struct AbAv1Runtime {
    sender: SyncSender<RuntimeCommand>,
    state: Arc<RuntimeState>,
    worker: Option<thread::JoinHandle<()>>,
    _permit: RuntimePermit,
}

impl AbAv1Runtime {
    pub fn start() -> Result<Self, RuntimeStartError> {
        let permit = RuntimePermit::acquire()?;
        let (sender, receiver) = mpsc::sync_channel(RUNTIME_COMMAND_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(RUNTIME_READY_CAPACITY);
        let state = Arc::new(RuntimeState::new());
        let worker_state = Arc::clone(&state);
        let worker = thread::Builder::new()
            .name("crfty-ab-av1-runtime".into())
            .spawn(move || runtime_thread(receiver, worker_state, ready_tx))
            .map_err(|error| {
                RuntimeStartError(format!("failed to start ab-av1 runtime: {error}"))
            })?;

        ready_rx.recv().map_err(|error| {
            RuntimeStartError(format!("ab-av1 runtime did not initialize: {error}"))
        })??;
        Ok(Self {
            sender,
            state,
            worker: Some(worker),
            _permit: permit,
        })
    }

    pub fn start_search(
        &self,
        tools: MediaTools,
        request: SearchRequest,
    ) -> Result<JobHandle<SearchOutcome>, StartJobError> {
        validate_search_request(&request)?;
        self.validate_and_acquire(&tools)?;
        let (handle, cancellation, telemetry, result) = JobHandle::channels();
        self.state.install_cancellation(&handle.cancellation);
        self.send_job(RuntimeCommand::Search {
            tools,
            request,
            cancellation,
            telemetry,
            result,
        })?;
        Ok(handle)
    }

    pub fn start_encode(
        &self,
        tools: MediaTools,
        request: EncodeRequest,
    ) -> Result<JobHandle<EncodeOutcome>, StartJobError> {
        self.start_encode_inner(
            tools,
            request,
            #[cfg(feature = "contract-test-fixture")]
            FaultInjection::None,
        )
    }

    #[cfg(feature = "contract-test-fixture")]
    pub fn start_encode_with_fault(
        &self,
        tools: MediaTools,
        request: EncodeRequest,
        fault: FaultInjection,
    ) -> Result<JobHandle<EncodeOutcome>, StartJobError> {
        self.start_encode_inner(tools, request, fault)
    }

    fn start_encode_inner(
        &self,
        tools: MediaTools,
        request: EncodeRequest,
        #[cfg(feature = "contract-test-fixture")] fault: FaultInjection,
    ) -> Result<JobHandle<EncodeOutcome>, StartJobError> {
        validate_encode_request(&request)?;
        self.validate_and_acquire(&tools)?;
        let (handle, cancellation, telemetry, result) = JobHandle::channels();
        self.state.install_cancellation(&handle.cancellation);
        self.send_job(RuntimeCommand::Encode {
            tools,
            request,
            cancellation,
            telemetry,
            result,
            #[cfg(feature = "contract-test-fixture")]
            fault,
        })?;
        Ok(handle)
    }

    pub fn shutdown(mut self) -> Result<(), ShutdownError> {
        self.stop_and_join()
    }

    fn validate_and_acquire(&self, tools: &MediaTools) -> Result<(), StartJobError> {
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(StartJobError::ShuttingDown);
        }
        validate_tool("ffmpeg", &tools.ffmpeg)?;
        validate_tool("ffprobe", &tools.ffprobe)?;
        if self.state.try_acquire() {
            Ok(())
        } else {
            Err(StartJobError::Busy)
        }
    }

    fn send_job(&self, command: RuntimeCommand) -> Result<(), StartJobError> {
        match self.sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.state.finish_job();
                Err(StartJobError::Busy)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.state.finish_job();
                Err(StartJobError::ShuttingDown)
            }
        }
    }

    fn stop_and_join(&mut self) -> Result<(), ShutdownError> {
        self.state.accepting.store(false, Ordering::Release);
        self.state.cancel_active();
        let _result = self.sender.send(RuntimeCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| ShutdownError("the ab-av1 runtime thread panicked".into()))?;
        }
        Ok(())
    }
}

struct RuntimePermit;

impl RuntimePermit {
    fn acquire() -> Result<Self, RuntimeStartError> {
        RUNTIME_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| RuntimeStartError("an ab-av1 runtime is already active".into()))
    }
}

impl Drop for RuntimePermit {
    fn drop(&mut self) {
        RUNTIME_ACTIVE.store(false, Ordering::Release);
    }
}

impl Drop for AbAv1Runtime {
    fn drop(&mut self) {
        let _result = self.stop_and_join();
    }
}

pub struct JobHandle<T> {
    cancellation: CancellationHandle,
    result: Receiver<JobReport<T>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    cancel_on_drop: bool,
    marker: PhantomData<T>,
}

type JobChannels<T> = (
    JobHandle<T>,
    tokio::sync::watch::Receiver<Option<CancelMode>>,
    Arc<Mutex<Option<Telemetry>>>,
    mpsc::Sender<JobReport<T>>,
);

impl<T> JobHandle<T> {
    fn channels() -> JobChannels<T> {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(None);
        let (result_tx, result_rx) = mpsc::channel();
        let telemetry = Arc::new(Mutex::new(None));
        (
            Self {
                cancellation: CancellationHandle { sender: cancel_tx },
                result: result_rx,
                telemetry: Arc::clone(&telemetry),
                cancel_on_drop: true,
                marker: PhantomData,
            },
            cancel_rx,
            telemetry,
            result_tx,
        )
    }

    #[must_use]
    pub fn latest_telemetry(&self) -> Option<Telemetry> {
        match self.telemetry.lock() {
            Ok(telemetry) => telemetry.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn cancellation_handle(&self) -> CancellationHandle {
        self.cancellation.clone()
    }

    pub fn cancel(&self, mode: CancelMode) {
        self.cancellation.cancel(mode);
    }

    pub fn wait(mut self) -> Result<JobReport<T>, WaitError> {
        let report = self
            .result
            .recv()
            .map_err(|error| WaitError(format!("ab-av1 runtime dropped the result: {error}")))?;
        self.cancel_on_drop = false;
        Ok(report)
    }

    /// Blocks up to `timeout` for the report; `Ok(None)` means the job is
    /// still running when the timeout elapses.
    pub fn recv_report(&mut self, timeout: Duration) -> Result<Option<JobReport<T>>, WaitError> {
        match self.result.recv_timeout(timeout) {
            Ok(report) => {
                self.cancel_on_drop = false;
                Ok(Some(report))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WaitError(
                "ab-av1 runtime dropped the result channel".to_owned(),
            )),
        }
    }
}

impl<T> Drop for JobHandle<T> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.cancellation.cancel(CancelMode::Force);
        }
    }
}

#[derive(Clone)]
pub struct CancellationHandle {
    sender: tokio::sync::watch::Sender<Option<CancelMode>>,
}

impl CancellationHandle {
    pub fn cancel(&self, mode: CancelMode) {
        let _result = self.sender.send(Some(mode));
    }

    #[cfg(test)]
    pub(crate) fn fixture() -> (Self, tokio::sync::watch::Receiver<Option<CancelMode>>) {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        (Self { sender }, receiver)
    }
}

fn runtime_thread(
    receiver: Receiver<RuntimeCommand>,
    state: Arc<RuntimeState>,
    ready: SyncSender<Result<(), RuntimeStartError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _result = ready.send(Err(RuntimeStartError(format!(
                "failed to build ab-av1 Tokio runtime: {error}"
            ))));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }

    while let Ok(command) = receiver.recv() {
        match command {
            RuntimeCommand::Search {
                tools,
                request,
                cancellation,
                telemetry,
                result,
            } => {
                let terminal = run_catching_panic(&runtime, || {
                    operation::run_search(tools, request, cancellation, Arc::clone(&telemetry))
                });
                send_report(&state, result, telemetry, terminal);
            }
            RuntimeCommand::Encode {
                tools,
                request,
                cancellation,
                telemetry,
                result,
                #[cfg(feature = "contract-test-fixture")]
                fault,
            } => {
                let terminal = run_catching_panic(&runtime, || {
                    operation::run_encode(
                        tools,
                        request,
                        cancellation,
                        Arc::clone(&telemetry),
                        #[cfg(feature = "contract-test-fixture")]
                        fault,
                    )
                });
                send_report(&state, result, telemetry, terminal);
            }
            RuntimeCommand::Shutdown => break,
        }
    }
    state.accepting.store(false, Ordering::Release);
}

fn run_catching_panic<T, F, Fut>(runtime: &tokio::runtime::Runtime, job: F) -> JobTerminal<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = JobTerminal<T>> + 'static,
{
    let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let local = tokio::task::LocalSet::new();
        runtime.block_on(local.run_until(job()))
    }));
    match execution {
        Ok(terminal) => terminal,
        Err(_) => {
            let cleanup_failure = runtime
                .block_on(ab_av1::cancel_job())
                .err()
                .map(|error| error.to_string());
            JobTerminal::Panicked { cleanup_failure }
        }
    }
}

fn send_report<T>(
    state: &RuntimeState,
    result: mpsc::Sender<JobReport<T>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    terminal: JobTerminal<T>,
) {
    let final_telemetry = match telemetry.lock() {
        Ok(telemetry) => telemetry.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    state.finish_job();
    let _result = result.send(JobReport {
        terminal,
        final_telemetry,
    });
}

fn validate_tool(name: &'static str, path: &std::path::Path) -> Result<(), StartJobError> {
    if path.is_absolute() && path.is_file() {
        Ok(())
    } else {
        Err(StartJobError::InvalidTool {
            name,
            path: path.to_owned(),
        })
    }
}

fn validate_search_request(request: &SearchRequest) -> Result<(), StartJobError> {
    if !request.target_vmaf.is_finite()
        || !(0.0..=f32::from(crfty_core::MAX_VMAF_SCORE)).contains(&request.target_vmaf)
    {
        return Err(StartJobError::InvalidRequest {
            reason: "target VMAF must be finite and in 0..=100".to_owned(),
        });
    }
    if !request.max_encoded_percent.is_finite() || request.max_encoded_percent <= 0.0 {
        return Err(StartJobError::InvalidRequest {
            reason: "maximum encoded percent must be positive and finite".to_owned(),
        });
    }
    if request.preset > crfty_core::MAX_ENCODING_PRESET
        || request.samples == Some(0)
        || request.sample_duration.is_zero()
    {
        return Err(StartJobError::InvalidRequest {
            reason: "preset and sample settings are outside the supported range".to_owned(),
        });
    }
    Ok(())
}

fn validate_encode_request(request: &EncodeRequest) -> Result<(), StartJobError> {
    if !request.crf.is_finite()
        || request.crf < 0.0
        || request.preset > crfty_core::MAX_ENCODING_PRESET
    {
        return Err(StartJobError::InvalidRequest {
            reason: "CRF must be non-negative and finite and preset must be supported".to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use crfty_core::DecodeMode;

    use super::{
        CancellationHandle, RuntimeState, validate_encode_request, validate_search_request,
    };
    use crate::ab_av1::{CancelMode, EncodeRequest, SearchRequest};

    #[test]
    fn cancel_during_acquisition_window_cancels_the_installed_handle() {
        let state = RuntimeState::new();
        // Acquisition flips `active` before the handle is installed.
        assert!(state.try_acquire());
        // Shutdown races into the window: no handle to snapshot yet.
        state.cancel_active();
        let (handle, receiver) = CancellationHandle::fixture();
        assert_eq!(*receiver.borrow(), None);
        // Installing the handle observes the pending cancel and fires it at once.
        state.install_cancellation(&handle);
        assert_eq!(*receiver.borrow(), Some(CancelMode::Force));
    }

    #[test]
    fn install_then_cancel_cancels_the_active_handle() {
        let state = RuntimeState::new();
        assert!(state.try_acquire());
        let (handle, receiver) = CancellationHandle::fixture();
        state.install_cancellation(&handle);
        assert_eq!(*receiver.borrow(), None);
        state.cancel_active();
        assert_eq!(*receiver.borrow(), Some(CancelMode::Force));
    }

    #[test]
    fn cancel_with_no_active_job_does_not_arm_a_pending_cancel() {
        let state = RuntimeState::new();
        // No job has acquired, so there is nothing to cancel.
        state.cancel_active();
        let (handle, receiver) = CancellationHandle::fixture();
        state.install_cancellation(&handle);
        assert_eq!(*receiver.borrow(), None);
    }

    #[test]
    fn acquisition_is_exclusive_until_the_job_finishes() {
        let state = RuntimeState::new();
        assert!(state.try_acquire());
        assert!(!state.try_acquire());

        state.finish_job();

        assert!(state.try_acquire());
    }

    #[test]
    fn finish_job_clears_pending_cancel_so_it_cannot_leak() {
        let state = RuntimeState::new();
        assert!(state.try_acquire());
        state.cancel_active();
        // The job boundary must discard the pending cancel.
        state.finish_job();
        // A later acquisition installs its handle and must survive.
        assert!(state.try_acquire());
        let (handle, receiver) = CancellationHandle::fixture();
        state.install_cancellation(&handle);
        assert_eq!(*receiver.borrow(), None);
    }

    #[test]
    fn rejects_non_finite_adapter_requests() {
        let search = SearchRequest {
            input: PathBuf::from("input.mkv"),
            target_vmaf: f32::NAN,
            max_encoded_percent: 80.0,
            preset: 6,
            samples: None,
            sample_duration: Duration::from_secs(20),
            thorough: false,
            decode_mode: DecodeMode::Software,
        };
        assert!(validate_search_request(&search).is_err());
        let encode = EncodeRequest {
            input: PathBuf::from("input.mkv"),
            output: PathBuf::from("output.mkv"),
            crf: f32::INFINITY,
            preset: 6,
            decode_mode: DecodeMode::Software,
        };
        assert!(validate_encode_request(&encode).is_err());
    }
}
