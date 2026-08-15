//! The Analysis control state machine: generation registry, the request
//! slot a new command replaces, cancellation, and the worker thread that
//! serves whichever request currently holds the slot.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use crfty_core::{
    AnalysisActivity, AnalysisCommand, AnalysisGenerationId, AnalysisRowId, Command, Reply,
    VideoExtension,
};

use crate::{
    driver::CommandSender, process_supervisor::ProcessCancellation, vendor::discovery::CurrentTools,
};

use super::basic_scan::{BasicScanRequest, run_basic_scan_request};
use super::discovery::{DiscoveryRequest, deduplicate_roots, display_text, run_discovery_request};
use super::{AnalysisError, AnalysisNativeRow, NativeEntryKind, lock, wait};

#[derive(Debug)]
pub(super) struct AnalysisGenerationRegistry {
    pub(super) generation: AnalysisGenerationId,
    pub(super) next_row_id: u64,
    pub(super) rows: BTreeMap<AnalysisRowId, AnalysisNativeRow>,
}

impl AnalysisGenerationRegistry {
    pub(super) fn new(generation: AnalysisGenerationId) -> Self {
        Self {
            generation,
            next_row_id: 1,
            rows: BTreeMap::new(),
        }
    }

    pub(super) fn insert(
        &mut self,
        path: PathBuf,
        source_root: PathBuf,
        parent: Option<AnalysisRowId>,
        kind: NativeEntryKind,
    ) -> Result<AnalysisRowId, ()> {
        let id = AnalysisRowId(self.next_row_id);
        self.next_row_id = self.next_row_id.checked_add(1).ok_or(())?;
        self.rows.insert(
            id,
            AnalysisNativeRow {
                path,
                source_root,
                parent,
                kind,
            },
        );
        Ok(id)
    }
}

#[derive(Debug, Clone)]
pub(super) struct Cancellation {
    pub(super) process: ProcessCancellation,
}

impl Cancellation {
    pub(super) fn new() -> Self {
        Self {
            process: ProcessCancellation::new(),
        }
    }

    pub(super) fn cancel(&self) {
        self.process.cancel();
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.process.is_cancelled()
    }
}

enum AnalysisRequest {
    Discovery(DiscoveryRequest),
    BasicScan(BasicScanRequest),
}

impl AnalysisRequest {
    fn generation(&self) -> AnalysisGenerationId {
        match self {
            Self::Discovery(request) => request.generation,
            Self::BasicScan(request) => request.generation,
        }
    }

    fn cancellation(&self) -> &Cancellation {
        match self {
            Self::Discovery(request) => &request.cancellation,
            Self::BasicScan(request) => &request.cancellation,
        }
    }
}

struct ControlState {
    shutting_down: bool,
    pending: Option<AnalysisRequest>,
    active: Option<(AnalysisGenerationId, Cancellation)>,
    current_registry: Option<Arc<Mutex<AnalysisGenerationRegistry>>>,
    cancelled_generation: Option<AnalysisGenerationId>,
}

struct DiscoveryControl {
    state: Mutex<ControlState>,
    wake: Condvar,
}

/// Owns the one discovery worker and current generation's native registry.
/// The worker is intentionally serial: deterministic BFS row allocation is a
/// user-visible contract, while Level 0 does no expensive media work.
pub(crate) struct AnalysisRuntime {
    commands: CommandSender,
    control: Arc<DiscoveryControl>,
    start_gate: Mutex<()>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    tools: Arc<Mutex<Option<CurrentTools>>>,
}

impl AnalysisRuntime {
    pub(crate) fn start(
        commands: CommandSender,
        tools: Arc<Mutex<Option<CurrentTools>>>,
    ) -> io::Result<Arc<Self>> {
        let control = Arc::new(DiscoveryControl {
            state: Mutex::new(ControlState {
                shutting_down: false,
                pending: None,
                active: None,
                current_registry: None,
                cancelled_generation: None,
            }),
            wake: Condvar::new(),
        });
        let worker_control = Arc::clone(&control);
        let worker_commands = commands.clone();
        let worker = thread::Builder::new()
            .name("crfty-analysis-discovery".to_owned())
            .spawn(move || discovery_worker(worker_control, worker_commands))?;
        Ok(Arc::new(Self {
            commands,
            control,
            start_gate: Mutex::new(()),
            worker: Mutex::new(Some(worker)),
            tools,
        }))
    }

    pub(crate) fn begin(
        &self,
        roots: Vec<PathBuf>,
        extensions: BTreeSet<VideoExtension>,
    ) -> Result<AnalysisGenerationId, AnalysisError> {
        let _gate = lock(&self.start_gate);
        if lock(&self.control.state).shutting_down {
            return Err(AnalysisError::ShuttingDown);
        }
        let roots = deduplicate_roots(roots);
        if roots.is_empty() {
            return Err(AnalysisError::EmptyRoots);
        }
        let display_roots = roots
            .iter()
            .map(|root| display_text(root.as_os_str()))
            .collect();
        let generation = match self
            .commands
            .submit(Command::Analysis(AnalysisCommand::Begin {
                roots: display_roots,
            }))? {
            Reply::AnalysisStarted { generation } => generation,
            Reply::Rejected { reason } | Reply::DurabilityUnknown { reason } => {
                return Err(AnalysisError::Rejected(reason));
            }
            Reply::Accepted
            | Reply::BasicScan(_)
            | Reply::Reserved(_)
            | Reply::Claimed(_)
            | Reply::Imported { .. } => {
                return Err(AnalysisError::UnexpectedReply);
            }
        };

        let registry = Arc::new(Mutex::new(AnalysisGenerationRegistry::new(generation)));
        let request = DiscoveryRequest {
            generation,
            roots,
            extensions,
            registry: Arc::clone(&registry),
            cancellation: Cancellation::new(),
        };
        let mut state = lock(&self.control.state);
        if let Some(pending) = state.pending.take() {
            pending.cancellation().cancel();
        }
        if let Some((_, active)) = &state.active {
            active.cancel();
        }
        state.current_registry = Some(registry);
        state.cancelled_generation = None;
        state.pending = Some(AnalysisRequest::Discovery(request));
        self.control.wake.notify_one();
        Ok(generation)
    }

    pub(crate) fn begin_basic_scan(
        &self,
        generation: AnalysisGenerationId,
    ) -> Result<(), AnalysisError> {
        let _gate = lock(&self.start_gate);
        if lock(&self.control.state).shutting_down {
            return Err(AnalysisError::ShuttingDown);
        }
        let ffprobe = lock(&self.tools)
            .as_ref()
            .map(|tools| tools.media.ffprobe.clone())
            .ok_or(AnalysisError::MissingTools)?;
        let registry = {
            let state = lock(&self.control.state);
            let registry = state.current_registry.as_ref().cloned().ok_or_else(|| {
                AnalysisError::Rejected("no Analysis generation exists".to_owned())
            })?;
            if lock(&registry).generation != generation {
                return Err(AnalysisError::Rejected(
                    "Basic Scan names a stale Analysis generation".to_owned(),
                ));
            }
            registry
        };
        match self
            .commands
            .submit(Command::Analysis(AnalysisCommand::BeginBasicScan {
                generation,
            }))? {
            Reply::Accepted => {}
            Reply::Rejected { reason } | Reply::DurabilityUnknown { reason } => {
                return Err(AnalysisError::Rejected(reason));
            }
            _ => return Err(AnalysisError::UnexpectedReply),
        }
        let request = BasicScanRequest {
            generation,
            registry,
            ffprobe,
            cancellation: Cancellation::new(),
        };
        let mut state = lock(&self.control.state);
        if let Some(pending) = state.pending.take() {
            pending.cancellation().cancel();
        }
        if let Some((_, active)) = &state.active {
            active.cancel();
        }
        state.cancelled_generation = None;
        state.pending = Some(AnalysisRequest::BasicScan(request));
        self.control.wake.notify_one();
        Ok(())
    }

    pub(crate) fn cancel(&self) -> Result<(), AnalysisError> {
        let _gate = lock(&self.start_gate);
        let generation = {
            let mut state = lock(&self.control.state);
            if state.shutting_down {
                return Err(AnalysisError::ShuttingDown);
            }
            let generation = state
                .current_registry
                .as_ref()
                .map(|registry| lock(registry).generation);
            if generation.is_some() && state.cancelled_generation == generation {
                return Ok(());
            }
            if let Some(pending) = state.pending.take() {
                pending.cancellation().cancel();
            }
            if let Some((_, active)) = &state.active {
                active.cancel();
            }
            state.cancelled_generation = generation;
            generation
        };
        let Some(generation) = generation else {
            return Ok(());
        };
        match self
            .commands
            .submit(Command::Analysis(AnalysisCommand::SetActivity {
                generation,
                activity: AnalysisActivity::Cancelled,
            }))? {
            Reply::Accepted => Ok(()),
            Reply::Rejected { reason } | Reply::DurabilityUnknown { reason } => {
                Err(AnalysisError::Rejected(reason))
            }
            Reply::AnalysisStarted { .. }
            | Reply::BasicScan(_)
            | Reply::Reserved(_)
            | Reply::Claimed(_)
            | Reply::Imported { .. } => Err(AnalysisError::UnexpectedReply),
        }
    }

    pub(crate) fn shutdown(&self) -> thread::Result<()> {
        let _gate = lock(&self.start_gate);
        {
            let mut state = lock(&self.control.state);
            state.shutting_down = true;
            if let Some(pending) = state.pending.take() {
                pending.cancellation().cancel();
            }
            if let Some((_, active)) = &state.active {
                active.cancel();
            }
            self.control.wake.notify_one();
        }
        let worker = lock(&self.worker).take();
        worker.map_or(Ok(()), thread::JoinHandle::join)
    }
}

fn discovery_worker(control: Arc<DiscoveryControl>, commands: CommandSender) {
    loop {
        let request = {
            let mut state = lock(&control.state);
            while state.pending.is_none() && !state.shutting_down {
                state = wait(&control.wake, state);
            }
            if state.shutting_down {
                return;
            }
            let Some(request) = state.pending.take() else {
                continue;
            };
            state.active = Some((request.generation(), request.cancellation().clone()));
            request
        };

        match &request {
            AnalysisRequest::Discovery(request) => run_discovery_request(request, &commands),
            AnalysisRequest::BasicScan(request) => run_basic_scan_request(request, &commands),
        }

        let mut state = lock(&control.state);
        if state.active.as_ref().map(|(generation, _)| *generation) == Some(request.generation()) {
            state.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, Instant},
    };

    use crfty_core::{AnalysisDelta, AnalysisSnapshot, EphemeralDelta, fold_analysis};

    use super::super::test_support::{extensions, test_directory};
    use super::*;
    use crate::driver::{DriverEvent, DriverHandle};

    #[test]
    #[expect(clippy::expect_used, reason = "test assertion")]
    fn superseding_generation_is_the_only_one_that_can_finish_and_retain_paths() {
        let data = tempfile::tempdir().expect("data directory");
        let first_root = test_directory("generation-first");
        let second_root = test_directory("generation-second");
        for index in 0..200 {
            let directory = first_root.join(format!("directory-{index:03}"));
            fs::create_dir(&directory).expect("first directory");
            fs::write(directory.join("old.mkv"), b"old").expect("old video");
        }
        fs::write(second_root.join("current.mkv"), b"current").expect("current video");

        let mut driver = DriverHandle::start(
            data.path().join("journal.jsonl"),
            data.path().join("config.json"),
        )
        .expect("driver");
        let events = driver.take_events().expect("events");
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(2)),
            Ok(DriverEvent::Snapshot(_))
        ));
        let runtime = AnalysisRuntime::start(driver.commands.clone(), Arc::new(Mutex::new(None)))
            .expect("runtime");
        let first = runtime
            .begin(vec![first_root.clone()], extensions())
            .expect("first generation");
        let second = runtime
            .begin(vec![second_root.clone()], extensions())
            .expect("second generation");
        assert_eq!(first, AnalysisGenerationId(1));
        assert_eq!(second, AnalysisGenerationId(2));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut snapshot = AnalysisSnapshot::default();
        let mut second_reset_seen = false;
        while Instant::now() < deadline {
            let event = match events.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("driver event stream disconnected")
                }
            };
            if let DriverEvent::Ephemeral(EphemeralDelta::Analysis(delta)) = event {
                if matches!(
                    &delta,
                    AnalysisDelta::Reset { snapshot }
                        if snapshot.current.as_ref().map(|generation| generation.id) == Some(second)
                ) {
                    second_reset_seen = true;
                }
                if second_reset_seen {
                    match &delta {
                        AnalysisDelta::RowsUpserted { generation, .. }
                        | AnalysisDelta::ActivityChanged { generation, .. } => {
                            assert_eq!(*generation, second);
                        }
                        AnalysisDelta::Reset { .. } => {}
                    }
                }
                fold_analysis(&mut snapshot, &delta);
            }
            if snapshot.current.as_ref().is_some_and(|generation| {
                generation.id == second && generation.activity == AnalysisActivity::Discovered
            }) {
                break;
            }
        }
        let generation = snapshot.current.expect("current generation");
        assert_eq!(generation.id, second);
        assert_eq!(generation.activity, AnalysisActivity::Discovered);
        assert_eq!(
            generation.roots,
            vec![display_text(second_root.as_os_str())]
        );
        assert!(
            generation
                .rows
                .values()
                .all(|row| !row.display_path.text.contains("generation-first"))
        );
        let file = generation
            .rows
            .values()
            .find(|row| row.display_name.text == "current.mkv")
            .expect("current row");
        let registry = lock(&runtime.control.state)
            .current_registry
            .clone()
            .expect("current registry");
        let registry = lock(&registry);
        assert_eq!(registry.generation, second);
        assert_eq!(
            registry.rows.get(&file.id).expect("native path").path,
            second_root.join("current.mkv")
        );

        runtime.shutdown().expect("runtime shutdown");
        driver.shutdown().expect("driver shutdown");
        fs::remove_dir_all(first_root).expect("remove first root");
        fs::remove_dir_all(second_root).expect("remove second root");
    }
}
