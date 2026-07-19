use std::{
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
};

use super::{
    operation,
    types::{
        EncodeOutcome, EncodeRequest, JobReport, JobTerminal, MediaTools, RuntimeStartError,
        SearchOutcome, SearchRequest, ShutdownError, StartJobError, Telemetry, WaitError,
    },
};

static RUNTIME_ACTIVE: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "contract-test-fixture")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FaultInjection {
    #[default]
    None,
    PanicAfterFirstProgress,
}

struct RuntimeState {
    accepting: AtomicBool,
    active: AtomicBool,
    cancellation: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
}

impl RuntimeState {
    fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            active: AtomicBool::new(false),
            cancellation: Mutex::new(None),
        }
    }

    fn finish_job(&self) {
        match self.cancellation.lock() {
            Ok(mut cancellation) => *cancellation = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
        self.active.store(false, Ordering::Release);
    }

    fn cancel_active(&self) {
        let cancellation = match self.cancellation.lock() {
            Ok(cancellation) => cancellation.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if let Some(cancellation) = cancellation {
            let _result = cancellation.send(true);
        }
    }
}

enum RuntimeCommand {
    Search {
        tools: MediaTools,
        request: SearchRequest,
        cancellation: tokio::sync::watch::Receiver<bool>,
        telemetry: Arc<Mutex<Option<Telemetry>>>,
        result: mpsc::Sender<JobReport<SearchOutcome>>,
    },
    Encode {
        tools: MediaTools,
        request: EncodeRequest,
        cancellation: tokio::sync::watch::Receiver<bool>,
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
        let (sender, receiver) = mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
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
        self.validate_and_acquire(&tools)?;
        let (handle, cancellation, telemetry, result) = JobHandle::channels();
        self.install_cancellation(&handle.cancellation);
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
        self.validate_and_acquire(&tools)?;
        let (handle, cancellation, telemetry, result) = JobHandle::channels();
        self.install_cancellation(&handle.cancellation);
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
        self.state
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| StartJobError::Busy)
    }

    fn install_cancellation(&self, cancellation: &tokio::sync::watch::Sender<bool>) {
        match self.state.cancellation.lock() {
            Ok(mut slot) => *slot = Some(cancellation.clone()),
            Err(poisoned) => *poisoned.into_inner() = Some(cancellation.clone()),
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
    cancellation: tokio::sync::watch::Sender<bool>,
    result: Receiver<JobReport<T>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    cancel_on_drop: bool,
    marker: PhantomData<T>,
}

type JobChannels<T> = (
    JobHandle<T>,
    tokio::sync::watch::Receiver<bool>,
    Arc<Mutex<Option<Telemetry>>>,
    mpsc::Sender<JobReport<T>>,
);

impl<T> JobHandle<T> {
    fn channels() -> JobChannels<T> {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let (result_tx, result_rx) = mpsc::channel();
        let telemetry = Arc::new(Mutex::new(None));
        (
            Self {
                cancellation: cancel_tx,
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

    pub fn cancel(&self) {
        let _result = self.cancellation.send(true);
    }

    pub fn wait(mut self) -> Result<JobReport<T>, WaitError> {
        let report = self
            .result
            .recv()
            .map_err(|error| WaitError(format!("ab-av1 runtime dropped the result: {error}")))?;
        self.cancel_on_drop = false;
        Ok(report)
    }
}

impl<T> Drop for JobHandle<T> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            let _result = self.cancellation.send(true);
        }
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

use std::future::Future;
