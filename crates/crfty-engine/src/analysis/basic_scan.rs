//! Level 1 Basic Scan: the bounded worker pool that probes discovered
//! files, and the typed failures a probe can report.

use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use crfty_core::{
    AnalysisActivity, AnalysisCommand, AnalysisDiagnosticTail, AnalysisGenerationId, AnalysisRowId,
    AnalysisScanFailure, BasicScanDisposition, Command, CurrentFileIdentity, Reply,
};

use crate::{
    driver::CommandSender,
    history_import,
    media::{self, MediaInspector, SupervisedMediaError},
};

use super::runtime::{AnalysisGenerationRegistry, Cancellation};
use super::{NativeEntryKind, lock};

const MIN_SCAN_WORKERS: usize = 4;

const MAX_SCAN_WORKERS: usize = 8;

const SCAN_QUEUE_MULTIPLIER: usize = 2;

pub(super) struct BasicScanRequest {
    pub(super) generation: AnalysisGenerationId,
    pub(super) registry: Arc<Mutex<AnalysisGenerationRegistry>>,
    pub(super) ffprobe: PathBuf,
    pub(super) cancellation: Cancellation,
}

#[derive(Clone)]
struct BasicScanTask {
    row_id: AnalysisRowId,
    path: PathBuf,
}

pub(super) fn run_basic_scan_request(request: &BasicScanRequest, commands: &CommandSender) {
    let tasks: Vec<BasicScanTask> = lock(&request.registry)
        .rows
        .iter()
        .filter(|(_, row)| row.kind == NativeEntryKind::File)
        .map(|(row_id, row)| BasicScanTask {
            row_id: *row_id,
            path: row.path.clone(),
        })
        .collect();
    if tasks.is_empty() {
        submit_scan_activity(commands, request.generation, AnalysisActivity::Ready);
        return;
    }
    let available = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(MIN_SCAN_WORKERS);
    let workers = available
        .clamp(MIN_SCAN_WORKERS, MAX_SCAN_WORKERS)
        .min(tasks.len());
    let capacity = workers.saturating_mul(SCAN_QUEUE_MULTIPLIER).max(1);
    let (task_tx, task_rx) = mpsc::sync_channel::<BasicScanTask>(capacity);
    let task_rx = Arc::new(Mutex::new(task_rx));
    let infrastructure_failed = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(workers);
    for index in 0..workers {
        let receiver = Arc::clone(&task_rx);
        let worker_commands = commands.clone();
        let cancellation = request.cancellation.clone();
        let ffprobe = request.ffprobe.clone();
        let failed = Arc::clone(&infrastructure_failed);
        let generation = request.generation;
        let spawned = thread::Builder::new()
            .name(format!("crfty-basic-scan-{index}"))
            .spawn(move || {
                let inspector = MediaInspector::new(ffprobe);
                loop {
                    if cancellation.is_cancelled() {
                        return;
                    }
                    let task = match lock(&receiver).recv() {
                        Ok(task) => task,
                        Err(_) => return,
                    };
                    if let Err(detail) = scan_one_file(
                        &worker_commands,
                        generation,
                        &task,
                        &inspector,
                        &cancellation,
                    ) {
                        if !cancellation.is_cancelled() {
                            tracing::warn!(
                                generation = generation.0,
                                row_id = task.row_id.0,
                                "Basic Scan worker stopped: {detail}"
                            );
                            failed.store(true, Ordering::Release);
                            cancellation.cancel();
                        }
                        return;
                    }
                }
            });
        match spawned {
            Ok(handle) => handles.push(handle),
            Err(error) => {
                tracing::warn!("failed to start Basic Scan worker: {error}");
                infrastructure_failed.store(true, Ordering::Release);
                request.cancellation.cancel();
                break;
            }
        }
    }
    let mut pending = VecDeque::from(tasks);
    while let Some(task) = pending.pop_front() {
        if request.cancellation.is_cancelled() {
            break;
        }
        match task_tx.try_send(task) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(task)) => {
                pending.push_front(task);
                thread::yield_now();
            }
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
    }
    drop(task_tx);
    for handle in handles {
        if handle.join().is_err() {
            infrastructure_failed.store(true, Ordering::Release);
            request.cancellation.cancel();
        }
    }
    if infrastructure_failed.load(Ordering::Acquire) {
        submit_scan_activity(
            commands,
            request.generation,
            AnalysisActivity::Failed {
                detail: "Basic Scan could not complete its bounded worker pool".to_owned(),
            },
        );
    } else if !request.cancellation.is_cancelled() {
        match commands.submit(Command::Analysis(AnalysisCommand::FinishBasicScan {
            generation: request.generation,
        })) {
            Ok(Reply::Accepted | Reply::Rejected { .. }) => {}
            Ok(reply) => tracing::warn!("unexpected Basic Scan completion reply: {reply:?}"),
            Err(error) => tracing::warn!("failed to submit Basic Scan completion: {error}"),
        }
    }
}

fn scan_one_file(
    commands: &CommandSender,
    generation: AnalysisGenerationId,
    task: &BasicScanTask,
    inspector: &MediaInspector,
    cancellation: &Cancellation,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Ok(());
    }
    let current = match media::destructive_identity(&task.path) {
        Ok(identity) => identity,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return submit_scan_failure(
                commands,
                generation,
                task.row_id,
                AnalysisScanFailure::Missing,
            );
        }
        Err(error) => {
            return submit_scan_failure(
                commands,
                generation,
                task.row_id,
                AnalysisScanFailure::Unavailable {
                    detail: format!("filesystem identity is unavailable: {:?}", error.kind()),
                },
            );
        }
    };
    let path_hash = match media::path_hash(&task.path) {
        Ok(path_hash) => path_hash,
        Err(error) => {
            return submit_scan_failure(
                commands,
                generation,
                task.row_id,
                AnalysisScanFailure::Unavailable {
                    detail: format!("path identity is unavailable: {:?}", error.kind()),
                },
            );
        }
    };
    let import_paths = history_import::import_path_candidates(&task.path);
    let reliability = media::timestamp_reliability(&current, std::time::SystemTime::now());
    let disposition = match commands.submit(Command::Analysis(AnalysisCommand::InspectFile {
        generation,
        row_id: task.row_id,
        path_hash,
        current: CurrentFileIdentity::Present(current),
        timestamp_reliability: reliability,
        import_paths: import_paths.clone(),
    })) {
        Ok(Reply::BasicScan(disposition)) => disposition,
        Ok(Reply::Rejected { reason: _ }) if cancellation.is_cancelled() => return Ok(()),
        Ok(Reply::Rejected { reason } | Reply::DurabilityUnknown { reason }) => {
            return Err(reason);
        }
        Ok(reply) => return Err(format!("unexpected Basic Scan inspection reply: {reply:?}")),
        Err(error) => return Err(error.to_string()),
    };
    match disposition {
        BasicScanDisposition::Complete => return Ok(()),
        BasicScanDisposition::Missing => {
            return submit_scan_failure(
                commands,
                generation,
                task.row_id,
                AnalysisScanFailure::Missing,
            );
        }
        BasicScanDisposition::Unavailable => {
            return submit_scan_failure(
                commands,
                generation,
                task.row_id,
                AnalysisScanFailure::Unavailable {
                    detail: "file identity is unavailable".to_owned(),
                },
            );
        }
        BasicScanDisposition::Observe => {}
    }
    let observation = match inspector.observe_supervised(&task.path, &cancellation.process) {
        Ok(observation) => observation,
        Err(SupervisedMediaError::Cancelled) => return Ok(()),
        Err(error) => {
            return submit_scan_failure(
                commands,
                generation,
                task.row_id,
                scan_failure(error, &task.path),
            );
        }
    };
    // Canonical spelling can change along with a replaced/rebound path.
    // Recompute candidates after the stable observation rather than adopting
    // against the pre-probe target's spelling.
    let import_paths = history_import::import_path_candidates(&task.path);
    match commands.submit(Command::Analysis(AnalysisCommand::ObserveFile {
        generation,
        row_id: task.row_id,
        observation: Box::new(observation),
        import_paths,
    })) {
        Ok(Reply::BasicScan(BasicScanDisposition::Complete)) => Ok(()),
        Ok(Reply::Rejected { .. }) if cancellation.is_cancelled() => Ok(()),
        Ok(Reply::Rejected { reason } | Reply::DurabilityUnknown { reason }) => Err(reason),
        Ok(reply) => Err(format!(
            "unexpected Basic Scan observation reply: {reply:?}"
        )),
        Err(error) => Err(error.to_string()),
    }
}

fn scan_failure(error: SupervisedMediaError, path: &Path) -> AnalysisScanFailure {
    match error {
        SupervisedMediaError::Cancelled => AnalysisScanFailure::Unavailable {
            detail: "scan was cancelled".to_owned(),
        },
        SupervisedMediaError::TimedOut { diagnostic } => AnalysisScanFailure::TimedOut {
            diagnostic: diagnostic_tail(diagnostic, path),
        },
        SupervisedMediaError::Rejected { diagnostic } => AnalysisScanFailure::Rejected {
            diagnostic: diagnostic_tail(diagnostic, path),
        },
        SupervisedMediaError::InvalidOutput { detail, diagnostic } => {
            AnalysisScanFailure::InvalidOutput {
                detail,
                diagnostic: diagnostic_tail(diagnostic, path),
            }
        }
        SupervisedMediaError::Supervision { detail, diagnostic } => {
            AnalysisScanFailure::Supervision {
                detail,
                diagnostic: diagnostic_tail(diagnostic, path),
            }
        }
        SupervisedMediaError::Io(error) if error.kind() == io::ErrorKind::NotFound => {
            AnalysisScanFailure::Missing
        }
        SupervisedMediaError::Io(error) => AnalysisScanFailure::Unavailable {
            detail: format!("media observation failed: {:?}", error.kind()),
        },
        SupervisedMediaError::ChangedAfterProbe => AnalysisScanFailure::ChangedAfterProbe,
        SupervisedMediaError::ChangedDuringSampling => AnalysisScanFailure::ChangedDuringSampling,
    }
}

fn diagnostic_tail(
    output: crate::process_supervisor::BoundedOutput,
    path: &Path,
) -> AnalysisDiagnosticTail {
    let mut text = String::from_utf8_lossy(output.as_bytes()).into_owned();
    let display_path = path.to_string_lossy();
    if !display_path.is_empty() {
        text = text.replace(display_path.as_ref(), "[input]");
    }
    if let Some(name) = path.file_name().map(|name| name.to_string_lossy())
        && !name.is_empty()
    {
        text = text.replace(name.as_ref(), "[input]");
    }
    AnalysisDiagnosticTail {
        text,
        truncated: output.was_truncated(),
    }
}

fn submit_scan_failure(
    commands: &CommandSender,
    generation: AnalysisGenerationId,
    row_id: AnalysisRowId,
    failure: AnalysisScanFailure,
) -> Result<(), String> {
    match commands.submit(Command::Analysis(AnalysisCommand::FailFile {
        generation,
        row_id,
        failure,
    })) {
        Ok(Reply::Accepted) => Ok(()),
        Ok(Reply::Rejected { reason } | Reply::DurabilityUnknown { reason }) => Err(reason),
        Ok(reply) => Err(format!("unexpected Basic Scan failure reply: {reply:?}")),
        Err(error) => Err(error.to_string()),
    }
}

fn submit_scan_activity(
    commands: &CommandSender,
    generation: AnalysisGenerationId,
    activity: AnalysisActivity,
) {
    match commands.submit(Command::Analysis(AnalysisCommand::SetActivity {
        generation,
        activity,
    })) {
        Ok(Reply::Accepted | Reply::Rejected { .. }) => {}
        Ok(reply) => tracing::warn!("unexpected Analysis activity reply: {reply:?}"),
        Err(error) => tracing::warn!("failed to submit Analysis activity: {error}"),
    }
}
