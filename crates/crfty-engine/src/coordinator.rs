use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use crfty_core::{
    AnalysisAttempt, AnalysisResult, CRF_FIXED_SCALE, ClaimId, ClaimedJob, Command, Crf,
    DurableDelta, DurableState, Effect, ExecutionSettings, ItemOutcome, JobPhase, MAX_VMAF_SCORE,
    Operation, OutputDelta, OutputTarget, PERCENT_BASIS_POINTS_SCALE, QueueItemState, Reply, RunId,
    SearchMeasurement, Telemetry, VMAF_SCORE_FIXED_SCALE, VmafScore, VmafTarget, WorkerCommand,
    fold,
};

const FIRST_RUNTIME_ID: u64 = 1;
const ADAPTER_REPORT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const NORMALIZED_PROGRESS_UNITS: f32 = 10_000.0;
const NORMALIZED_PROGRESS_MIN: f32 = 0.0;
const NORMALIZED_PROGRESS_MAX: f32 = 1.0;
const TERMINAL_TELEMETRY_SEQUENCE: u64 = u64::MAX;
const OUTPUT_CONTAINER_EXTENSION: &str = "mkv";

use crate::{
    ab_av1::{
        AbAv1Runtime, CancelMode, CancellationHandle, EncodeRequest, JobFailureKind, JobHandle,
        JobReport, JobTerminal, MediaTools, SearchOutcome, SearchRequest,
        Telemetry as AdapterTelemetry,
    },
    driver::{CommandSender, DriverEvent, DriverHandle, DriverStartError},
    output::{MediaArtifactInspector, OutputManager},
};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub journal_path: PathBuf,
    pub media_tools: MediaTools,
    pub execution: ExecutionSettings,
}

#[derive(Debug)]
pub struct EngineStartError(String);

impl fmt::Display for EngineStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EngineStartError {}

pub struct EngineRuntime {
    pub commands: CommandSender,
    pub events: mpsc::Receiver<DriverEvent>,
    driver: Option<DriverHandle>,
    supervisor: Option<thread::JoinHandle<()>>,
    event_forwarder: Option<thread::JoinHandle<()>>,
    runtime: Option<Arc<AbAv1Runtime>>,
}

impl EngineRuntime {
    pub fn start(config: EngineConfig) -> Result<Self, EngineStartError> {
        let runtime = Arc::new(
            AbAv1Runtime::start()
                .map_err(|error| EngineStartError(format!("failed to start encoder: {error}")))?,
        );
        let (effect_tx, effect_rx) = mpsc::channel();
        let mut driver = DriverHandle::start_with_effects(&config.journal_path, effect_tx)
            .map_err(map_driver_start)?;
        let driver_events = driver
            .take_events()
            .ok_or_else(|| EngineStartError("driver event receiver is missing".to_owned()))?;
        let initial = match driver_events.recv() {
            Ok(DriverEvent::Snapshot(snapshot)) => snapshot,
            Ok(_) => {
                return Err(EngineStartError(
                    "driver did not emit its snapshot first".to_owned(),
                ));
            }
            Err(error) => {
                return Err(EngineStartError(format!(
                    "driver disconnected before startup recovery: {error}"
                )));
            }
        };
        let recovered = recover_startup(&driver.commands, &config.media_tools, initial);
        let next_runtime_id = next_runtime_id(&recovered)?;
        let (public_event_tx, public_event_rx) = mpsc::channel();
        public_event_tx
            .send(DriverEvent::Snapshot(recovered))
            .map_err(|error| {
                EngineStartError(format!("failed to emit startup snapshot: {error}"))
            })?;
        for pending in driver_events.try_iter() {
            if !matches!(pending, DriverEvent::Durable(_)) {
                public_event_tx.send(pending).map_err(|error| {
                    EngineStartError(format!("failed to emit startup event: {error}"))
                })?;
            }
        }
        let event_forwarder = thread::Builder::new()
            .name("crfty-event-forwarder".to_owned())
            .spawn(move || {
                while let Ok(event) = driver_events.recv() {
                    if public_event_tx.send(event).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| EngineStartError(format!("failed to start event bridge: {error}")))?;
        let commands = driver.commands.clone();
        let supervisor_commands = commands.clone();
        let supervisor_runtime = Arc::clone(&runtime);
        let supervisor = thread::Builder::new()
            .name("crfty-job-supervisor".to_owned())
            .spawn(move || {
                supervise(
                    effect_rx,
                    supervisor_commands,
                    supervisor_runtime,
                    config,
                    next_runtime_id,
                );
            })
            .map_err(|error| EngineStartError(format!("failed to start coordinator: {error}")))?;
        Ok(Self {
            commands,
            events: public_event_rx,
            driver: Some(driver),
            supervisor: Some(supervisor),
            event_forwarder: Some(event_forwarder),
            runtime: Some(runtime),
        })
    }

    pub fn shutdown(mut self) -> Result<(), EngineStartError> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), EngineStartError> {
        if let Some(driver) = self.driver.take() {
            driver.shutdown().map_err(map_driver_start)?;
        }
        if let Some(supervisor) = self.supervisor.take() {
            supervisor
                .join()
                .map_err(|_| EngineStartError("job supervisor panicked".to_owned()))?;
        }
        if let Some(forwarder) = self.event_forwarder.take() {
            forwarder
                .join()
                .map_err(|_| EngineStartError("event forwarder panicked".to_owned()))?;
        }
        if let Some(runtime) = self.runtime.take() {
            let runtime = Arc::try_unwrap(runtime).map_err(|_| {
                EngineStartError("encoder runtime still has active owners".to_owned())
            })?;
            runtime
                .shutdown()
                .map_err(|error| EngineStartError(format!("encoder shutdown failed: {error}")))?;
        }
        Ok(())
    }
}

impl Drop for EngineRuntime {
    fn drop(&mut self) {
        let _result = self.stop_and_join();
    }
}

fn map_driver_start(error: DriverStartError) -> EngineStartError {
    EngineStartError(format!("driver error: {error}"))
}

fn next_runtime_id(state: &DurableState) -> Result<u64, EngineStartError> {
    let maximum = state
        .queue
        .iter()
        .filter_map(|item| match item.state {
            QueueItemState::Claimed { claim_id, run_id }
            | QueueItemState::Running { claim_id, run_id } => Some(claim_id.0.max(run_id.0)),
            QueueItemState::Queued | QueueItemState::Finished(_) => None,
        })
        .chain(state.conversion_runs.keys().map(|run_id| run_id.0))
        .chain(state.outputs.keys().map(|run_id| run_id.0))
        .max()
        .unwrap_or(FIRST_RUNTIME_ID.saturating_sub(1));
    maximum
        .checked_add(1)
        .ok_or_else(|| EngineStartError("runtime id space is exhausted".to_owned()))
}

fn recover_startup(
    commands: &CommandSender,
    tools: &MediaTools,
    mut state: DurableState,
) -> DurableState {
    let manager = OutputManager::new(MediaArtifactInspector::new(tools.ffprobe.clone()));
    let active: Vec<_> = state
        .queue
        .iter()
        .filter_map(|item| match item.state {
            QueueItemState::Claimed { claim_id, run_id }
            | QueueItemState::Running { claim_id, run_id } => Some((item.id, claim_id, run_id)),
            QueueItemState::Queued | QueueItemState::Finished(_) => None,
        })
        .collect();
    for (item_id, claim_id, run_id) in active {
        while let Some(transaction) = state.outputs.get(&run_id).cloned() {
            if transaction.is_settled() {
                break;
            }
            let delta = match manager.recover_once(&transaction) {
                Ok(Some(delta)) => delta,
                Ok(None) => break,
                Err(error) => OutputDelta::Conflict {
                    run_id,
                    reason: format!("startup output recovery failed: {error}"),
                },
            };
            if !accepted(commands.submit(Command::Worker(WorkerCommand::Output(delta.clone())))) {
                break;
            }
            fold(&mut state, &DurableDelta::Output(delta));
        }
        let output_settled = state
            .outputs
            .get(&run_id)
            .is_none_or(crfty_core::OutputTransaction::is_settled);
        if output_settled {
            let outcome = ItemOutcome::Stopped;
            if accepted(commands.submit(Command::Worker(WorkerCommand::Terminal {
                item_id,
                claim_id,
                run_id,
                outcome: outcome.clone(),
                final_telemetry: None,
            }))) {
                fold(
                    &mut state,
                    &DurableDelta::ItemFinished {
                        item_id,
                        claim_id,
                        run_id,
                        outcome,
                    },
                );
            }
        }
    }
    state
}

#[derive(Clone)]
struct ActiveCancellation {
    force_stopping: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<(RunId, CancellationHandle)>>>,
}

impl ActiveCancellation {
    fn register(&self, run_id: RunId, handle: CancellationHandle) {
        if self.force_stopping.load(Ordering::Acquire) {
            handle.cancel(CancelMode::Force);
        }
        match self.slot.lock() {
            Ok(mut slot) => *slot = Some((run_id, handle)),
            Err(poisoned) => *poisoned.into_inner() = Some((run_id, handle)),
        }
    }

    fn clear(&self, run_id: RunId) {
        let mut slot = match self.slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.as_ref().is_some_and(|(active, _)| *active == run_id) {
            *slot = None;
        }
    }

    fn force(&self, run_id: Option<RunId>) {
        self.force_stopping.store(true, Ordering::Release);
        let slot = match self.slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((active, handle)) = slot.as_ref()
            && run_id.is_none_or(|expected| expected == *active)
        {
            handle.cancel(CancelMode::Force);
        }
    }
}

fn supervise(
    effects: mpsc::Receiver<Effect>,
    commands: CommandSender,
    runtime: Arc<AbAv1Runtime>,
    config: EngineConfig,
    next_runtime_id: u64,
) {
    let cancellation = ActiveCancellation {
        force_stopping: Arc::new(AtomicBool::new(false)),
        slot: Arc::new(Mutex::new(None)),
    };
    let next_id = Arc::new(AtomicU64::new(next_runtime_id));
    let mut worker: Option<thread::JoinHandle<()>> = None;
    while let Ok(effect) = effects.recv() {
        match effect {
            Effect::StartWorker => {
                if let Some(previous) = worker.take() {
                    let _result = previous.join();
                }
                cancellation.force_stopping.store(false, Ordering::Release);
                let worker_commands = commands.clone();
                let worker_runtime = Arc::clone(&runtime);
                let worker_config = config.clone();
                let worker_cancellation = cancellation.clone();
                let worker_ids = Arc::clone(&next_id);
                worker = thread::Builder::new()
                    .name("crfty-session-worker".to_owned())
                    .spawn(move || {
                        run_session(
                            &worker_commands,
                            &worker_runtime,
                            &worker_config,
                            &worker_cancellation,
                            &worker_ids,
                        );
                    })
                    .ok();
            }
            Effect::KillActiveRun { run_id } => cancellation.force(Some(run_id)),
            Effect::StopDriver => {
                cancellation.force(None);
                break;
            }
        }
    }
    if let Some(worker) = worker {
        let _result = worker.join();
    }
}

fn run_session(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    config: &EngineConfig,
    cancellation: &ActiveCancellation,
    ids: &AtomicU64,
) {
    loop {
        let claim_id = ClaimId(ids.fetch_add(1, Ordering::Relaxed));
        let run_id = RunId(ids.fetch_add(1, Ordering::Relaxed));
        let claim = commands.submit(Command::Worker(WorkerCommand::ClaimNext {
            claim_id,
            run_id,
            execution: config.execution.clone(),
        }));
        let job = match claim {
            Ok(Reply::Claimed(Some(job))) => job,
            Ok(Reply::Claimed(None) | Reply::Rejected { .. }) | Err(_) => break,
            Ok(Reply::Accepted) => break,
        };
        if !accepted(commands.submit(Command::Worker(WorkerCommand::Started {
            item_id: job.spec.item_id,
            claim_id,
            run_id,
        }))) {
            break;
        }
        process_job(commands, runtime, config, cancellation, &job);
    }
    let _reply = commands.submit(Command::Worker(WorkerCommand::Finished));
}

fn process_job(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    config: &EngineConfig,
    cancellation: &ActiveCancellation,
    job: &ClaimedJob,
) {
    let mut telemetry_sequence = 0_u64;
    publish_phase(
        commands,
        job.spec.run_id,
        &mut telemetry_sequence,
        JobPhase::Preparing,
    );
    let analysis = match search_with_fallback(
        commands,
        runtime,
        config,
        cancellation,
        job,
        &mut telemetry_sequence,
    ) {
        Ok(result) => result,
        Err(outcome) => {
            terminal(commands, job, outcome, None);
            return;
        }
    };
    if !accepted(
        commands.submit(Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: job.spec.item_id,
            claim_id: job.spec.claim_id,
            run_id: job.spec.run_id,
            result: Box::new(analysis.clone()),
        })),
    ) {
        return;
    }
    if job.spec.operation == Operation::Analyze {
        publish_phase(
            commands,
            job.spec.run_id,
            &mut telemetry_sequence,
            JobPhase::Finalizing,
        );
        terminal(commands, job, ItemOutcome::Analyzed, None);
        return;
    }

    let (final_path, replacement) = match resolve_output(job) {
        Ok(resolved) => resolved,
        Err(message) => {
            terminal(commands, job, ItemOutcome::Failed { message }, None);
            return;
        }
    };
    if let Some(parent) = final_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        terminal(
            commands,
            job,
            ItemOutcome::Failed {
                message: format!("failed to create output directory: {error}"),
            },
            None,
        );
        return;
    }
    if final_path.exists() && !job.spec.execution.overwrite_existing && final_path != job.spec.input
    {
        terminal(
            commands,
            job,
            ItemOutcome::Skipped {
                reason: "output already exists".to_owned(),
            },
            None,
        );
        return;
    }
    let manager = OutputManager::new(MediaArtifactInspector::new(
        config.media_tools.ffprobe.clone(),
    ));
    let mut transaction =
        match manager.prepare(job.spec.run_id, &job.spec.input, &final_path, replacement) {
            Ok(transaction) => transaction,
            Err(error) => {
                terminal(
                    commands,
                    job,
                    ItemOutcome::Failed {
                        message: error.to_string(),
                    },
                    None,
                );
                return;
            }
        };
    let started = OutputDelta::EncodeStarted {
        transaction: Box::new(transaction.clone()),
    };
    if !submit_output(commands, started) {
        let _cleanup = manager.discard_unjournaled(&transaction);
        return;
    }

    let request = EncodeRequest {
        input: job.spec.input.clone(),
        output: transaction.staging.clone(),
        crf: crf_to_f32(analysis.measurement.crf),
        preset: analysis.profile.preset,
    };
    let handle = match runtime.start_encode(config.media_tools.clone(), request) {
        Ok(handle) => handle,
        Err(error) => {
            settle_abandoned(&manager, commands, &transaction);
            terminal(
                commands,
                job,
                ItemOutcome::Failed {
                    message: error.to_string(),
                },
                None,
            );
            return;
        }
    };
    let report = wait_for_report(
        commands,
        job.spec.run_id,
        handle,
        cancellation,
        JobPhase::Encoding,
        &mut telemetry_sequence,
    );
    match report {
        Ok(JobReport {
            terminal: JobTerminal::Completed(_),
            final_telemetry,
        }) => {
            publish_phase(
                commands,
                job.spec.run_id,
                &mut telemetry_sequence,
                JobPhase::Verifying,
            );
            let ready = match manager.mark_ready(&transaction) {
                Ok(ready) => ready,
                Err(error) => {
                    settle_abandoned(&manager, commands, &transaction);
                    terminal(
                        commands,
                        job,
                        ItemOutcome::Failed {
                            message: error.to_string(),
                        },
                        map_telemetry(
                            job.spec.run_id,
                            TERMINAL_TELEMETRY_SEQUENCE,
                            JobPhase::Verifying,
                            final_telemetry,
                        ),
                    );
                    return;
                }
            };
            if !submit_output(commands, ready.clone()) {
                return;
            }
            fold_transaction(&mut transaction, ready);
            publish_phase(
                commands,
                job.spec.run_id,
                &mut telemetry_sequence,
                JobPhase::Finalizing,
            );
            while !transaction.is_settled() {
                let next = match manager.recover_once(&transaction) {
                    Ok(Some(next)) => next,
                    Ok(None) => break,
                    Err(error) => {
                        settle_conflict(commands, job.spec.run_id, error.to_string());
                        terminal(
                            commands,
                            job,
                            ItemOutcome::Failed {
                                message: error.to_string(),
                            },
                            map_telemetry(
                                job.spec.run_id,
                                TERMINAL_TELEMETRY_SEQUENCE,
                                JobPhase::Finalizing,
                                final_telemetry,
                            ),
                        );
                        return;
                    }
                };
                if !submit_output(commands, next.clone()) {
                    return;
                }
                fold_transaction(&mut transaction, next);
            }
            terminal(
                commands,
                job,
                ItemOutcome::Converted,
                map_telemetry(
                    job.spec.run_id,
                    TERMINAL_TELEMETRY_SEQUENCE,
                    JobPhase::Finalizing,
                    final_telemetry,
                ),
            );
        }
        Ok(JobReport {
            terminal: JobTerminal::Cancelled,
            final_telemetry,
        }) => {
            settle_abandoned(&manager, commands, &transaction);
            terminal(
                commands,
                job,
                ItemOutcome::Stopped,
                map_telemetry(
                    job.spec.run_id,
                    TERMINAL_TELEMETRY_SEQUENCE,
                    JobPhase::Encoding,
                    final_telemetry,
                ),
            );
        }
        Ok(report) => {
            settle_abandoned(&manager, commands, &transaction);
            terminal(
                commands,
                job,
                ItemOutcome::Failed {
                    message: format!("encode failed: {:?}", report.terminal),
                },
                map_telemetry(
                    job.spec.run_id,
                    TERMINAL_TELEMETRY_SEQUENCE,
                    JobPhase::Encoding,
                    report.final_telemetry,
                ),
            );
        }
        Err(message) => {
            settle_abandoned(&manager, commands, &transaction);
            terminal(commands, job, ItemOutcome::Failed { message }, None);
        }
    }
}

fn search_with_fallback(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    config: &EngineConfig,
    cancellation: &ActiveCancellation,
    job: &ClaimedJob,
    telemetry_sequence: &mut u64,
) -> Result<AnalysisResult, ItemOutcome> {
    let execution = &job.spec.execution;
    let mut target = execution.requested_target.0;
    let mut failed_attempts = Vec::new();
    loop {
        let request = SearchRequest {
            input: job.spec.input.clone(),
            target_vmaf: f32::from(target),
            max_encoded_percent: execution.profile.max_encoded_percent_basis_points as f32
                / PERCENT_BASIS_POINTS_SCALE as f32,
            preset: execution.profile.preset,
            samples: execution.profile.samples,
            sample_duration: Duration::from_millis(execution.profile.sample_duration_ms),
            thorough: execution.profile.thorough,
        };
        let handle = runtime
            .start_search(config.media_tools.clone(), request)
            .map_err(|error| ItemOutcome::Failed {
                message: error.to_string(),
            })?;
        let report = wait_for_report(
            commands,
            job.spec.run_id,
            handle,
            cancellation,
            JobPhase::Analyzing,
            telemetry_sequence,
        )
        .map_err(|message| ItemOutcome::Failed { message })?;
        match report.terminal {
            JobTerminal::Completed(outcome) => {
                return Ok(AnalysisResult {
                    requested_target: execution.requested_target,
                    successful_target: VmafTarget(target),
                    fallback_floor: execution.fallback_floor,
                    fallback_step: execution.fallback_step,
                    failed_attempts,
                    measurement: measurement(outcome),
                    profile: execution.profile.clone(),
                });
            }
            JobTerminal::Failed(failure) => match failure.kind {
                JobFailureKind::NoGoodCrf { last } => failed_attempts.push(AnalysisAttempt {
                    target: VmafTarget(target),
                    last_measurement: Some(measurement(last)),
                }),
                JobFailureKind::Other => {
                    return Err(ItemOutcome::Failed {
                        message: failure.message,
                    });
                }
            },
            JobTerminal::Cancelled => return Err(ItemOutcome::Stopped),
            JobTerminal::Panicked { cleanup_failure } => {
                return Err(ItemOutcome::Failed {
                    message: format!("analysis panicked; cleanup: {cleanup_failure:?}"),
                });
            }
        }
        if target <= execution.fallback_floor.0
            || execution.fallback_step == 0
            || target.saturating_sub(execution.fallback_step) < execution.fallback_floor.0
        {
            return Err(ItemOutcome::NotWorthwhile {
                attempts: failed_attempts,
            });
        }
        target = target.saturating_sub(execution.fallback_step);
    }
}

fn wait_for_report<T>(
    commands: &CommandSender,
    run_id: RunId,
    mut handle: JobHandle<T>,
    cancellation: &ActiveCancellation,
    phase: JobPhase,
    telemetry_sequence: &mut u64,
) -> Result<JobReport<T>, String> {
    cancellation.register(run_id, handle.cancellation_handle());
    loop {
        match handle.try_report().map_err(|error| error.to_string())? {
            Some(report) => {
                cancellation.clear(run_id);
                return Ok(report);
            }
            None => {
                if let Some(update) = handle.latest_telemetry() {
                    *telemetry_sequence = telemetry_sequence.saturating_add(1);
                    commands.publish_telemetry(Telemetry {
                        run_id,
                        sequence: *telemetry_sequence,
                        phase,
                        completed_units: telemetry_units(&update),
                    });
                }
                thread::sleep(ADAPTER_REPORT_POLL_INTERVAL);
            }
        }
    }
}

fn telemetry_units(telemetry: &AdapterTelemetry) -> u64 {
    match telemetry {
        AdapterTelemetry::Search(search) => {
            (search
                .progress
                .clamp(NORMALIZED_PROGRESS_MIN, NORMALIZED_PROGRESS_MAX)
                * NORMALIZED_PROGRESS_UNITS) as u64
        }
        AdapterTelemetry::Encode(encode) => encode.position.as_millis() as u64,
    }
}

fn measurement(outcome: SearchOutcome) -> SearchMeasurement {
    SearchMeasurement {
        crf: Crf((outcome.crf * CRF_FIXED_SCALE as f32).round().max(0.0) as u32),
        score: VmafScore(
            (outcome.vmaf * f32::from(VMAF_SCORE_FIXED_SCALE))
                .round()
                .clamp(
                    0.0,
                    f32::from(VMAF_SCORE_FIXED_SCALE) * f32::from(MAX_VMAF_SCORE),
                ) as u16,
        ),
        predicted_size: outcome.predicted_size,
        predicted_percent_basis_points: (outcome.predicted_percent
            * f64::from(PERCENT_BASIS_POINTS_SCALE))
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32,
        predicted_duration_ms: outcome.predicted_duration.as_millis() as u64,
        from_cache: outcome.from_cache,
    }
}

fn crf_to_f32(crf: Crf) -> f32 {
    crf.0 as f32 / CRF_FIXED_SCALE as f32
}

fn resolve_output(job: &ClaimedJob) -> Result<(PathBuf, crfty_core::Replacement), String> {
    let input = &job.spec.input;
    let stem = input
        .file_stem()
        .ok_or_else(|| "input has no file stem".to_owned())?;
    match &job.spec.output_target {
        OutputTarget::Replace => Ok((
            input.with_extension(OUTPUT_CONTAINER_EXTENSION),
            if input
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case(OUTPUT_CONTAINER_EXTENSION))
            {
                crfty_core::Replacement::KeepOriginal
            } else {
                crfty_core::Replacement::RetireOriginal
            },
        )),
        OutputTarget::Suffix { suffix } => {
            if suffix.is_empty() {
                return Err("output suffix cannot be empty".to_owned());
            }
            let mut name = stem.to_os_string();
            name.push(suffix);
            name.push(".");
            name.push(OUTPUT_CONTAINER_EXTENSION);
            Ok((
                input.with_file_name(name),
                crfty_core::Replacement::KeepOriginal,
            ))
        }
        OutputTarget::SeparateFolder {
            directory,
            source_root,
        } => {
            let relative_parent = source_root.as_ref().and_then(|root| {
                input
                    .parent()
                    .and_then(|parent| parent.strip_prefix(root).ok())
            });
            let parent =
                relative_parent.map_or_else(|| directory.clone(), |path| directory.join(path));
            Ok((
                parent.join(stem).with_extension(OUTPUT_CONTAINER_EXTENSION),
                crfty_core::Replacement::KeepOriginal,
            ))
        }
    }
}

fn submit_output(commands: &CommandSender, delta: OutputDelta) -> bool {
    accepted(commands.submit(Command::Worker(WorkerCommand::Output(delta))))
}

fn settle_conflict(commands: &CommandSender, run_id: RunId, reason: String) {
    let _accepted = submit_output(commands, OutputDelta::Conflict { run_id, reason });
}

fn settle_abandoned(
    manager: &OutputManager<MediaArtifactInspector>,
    commands: &CommandSender,
    transaction: &crfty_core::OutputTransaction,
) {
    let intent = match manager.abandon_intent(transaction) {
        Ok(intent) => intent,
        Err(error) => {
            settle_conflict(commands, transaction.run_id, error.to_string());
            return;
        }
    };
    if !submit_output(commands, intent.clone()) {
        return;
    }
    let mut abandoning = transaction.clone();
    fold_transaction(&mut abandoning, intent);
    match manager.recover_once(&abandoning) {
        Ok(Some(abandoned)) => {
            let _accepted = submit_output(commands, abandoned);
        }
        Ok(None) => {}
        Err(error) => settle_conflict(commands, transaction.run_id, error.to_string()),
    }
}

fn fold_transaction(transaction: &mut crfty_core::OutputTransaction, delta: OutputDelta) {
    let mut state = DurableState::default();
    state
        .outputs
        .insert(transaction.run_id, transaction.clone());
    fold(&mut state, &DurableDelta::Output(delta));
    if let Some(updated) = state.outputs.remove(&transaction.run_id) {
        *transaction = updated;
    }
}

fn terminal(
    commands: &CommandSender,
    job: &ClaimedJob,
    outcome: ItemOutcome,
    final_telemetry: Option<Telemetry>,
) {
    let _reply = commands.submit(Command::Worker(WorkerCommand::Terminal {
        item_id: job.spec.item_id,
        claim_id: job.spec.claim_id,
        run_id: job.spec.run_id,
        outcome,
        final_telemetry,
    }));
}

fn map_telemetry(
    run_id: RunId,
    sequence: u64,
    phase: JobPhase,
    telemetry: Option<AdapterTelemetry>,
) -> Option<Telemetry> {
    telemetry.map(|telemetry| Telemetry {
        run_id,
        sequence,
        phase,
        completed_units: telemetry_units(&telemetry),
    })
}

fn publish_phase(commands: &CommandSender, run_id: RunId, sequence: &mut u64, phase: JobPhase) {
    *sequence = sequence.saturating_add(1);
    commands.publish_telemetry(Telemetry {
        run_id,
        sequence: *sequence,
        phase,
        completed_units: 0,
    });
}

fn accepted(reply: Result<Reply, crate::driver::SubmitError>) -> bool {
    matches!(reply, Ok(Reply::Accepted))
}
