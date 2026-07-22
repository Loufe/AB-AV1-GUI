//! Generation-scoped Level-0 Analysis discovery.
//!
//! One dedicated worker owns traversal order. New requests replace any
//! pending request and cancel the active one, while the core reducer remains
//! the final stale-generation gate. Native paths never cross the IPC model.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsStr,
    fmt, fs, io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crfty_core::{
    AnalysisActivity, AnalysisCommand, AnalysisDirectoryFailure, AnalysisDisplayText,
    AnalysisEntryKind, AnalysisGenerationId, AnalysisRow, AnalysisRowId, Command, Reply,
    VideoExtension,
};

use crate::driver::{CommandSender, SubmitError};

const DISCOVERY_BATCH_SIZE: usize = 128;

#[derive(Debug)]
pub enum AnalysisDiscoveryError {
    EmptyRoots,
    ShuttingDown,
    Submit(SubmitError),
    Rejected(String),
    UnexpectedReply,
}

impl fmt::Display for AnalysisDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRoots => {
                formatter.write_str("Analysis discovery requires at least one root")
            }
            Self::ShuttingDown => formatter.write_str("Analysis discovery is shutting down"),
            Self::Submit(error) => write!(formatter, "failed to submit Analysis command: {error}"),
            Self::Rejected(reason) => formatter.write_str(reason),
            Self::UnexpectedReply => {
                formatter.write_str("driver returned an unexpected Analysis reply")
            }
        }
    }
}

impl std::error::Error for AnalysisDiscoveryError {}

impl From<SubmitError> for AnalysisDiscoveryError {
    fn from(error: SubmitError) -> Self {
        Self::Submit(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnalysisNativeRow {
    pub(crate) path: PathBuf,
    pub(crate) source_root: PathBuf,
    pub(crate) parent: Option<AnalysisRowId>,
    pub(crate) kind: AnalysisEntryKind,
}

#[derive(Debug)]
struct AnalysisGenerationRegistry {
    generation: AnalysisGenerationId,
    next_row_id: u64,
    rows: BTreeMap<AnalysisRowId, AnalysisNativeRow>,
}

impl AnalysisGenerationRegistry {
    fn new(generation: AnalysisGenerationId) -> Self {
        Self {
            generation,
            next_row_id: 1,
            rows: BTreeMap::new(),
        }
    }

    fn insert(
        &mut self,
        path: PathBuf,
        source_root: PathBuf,
        parent: Option<AnalysisRowId>,
        kind: AnalysisEntryKind,
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
struct Cancellation {
    cancelled: Arc<AtomicBool>,
}

impl Cancellation {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

struct DiscoveryRequest {
    generation: AnalysisGenerationId,
    roots: Vec<PathBuf>,
    extensions: BTreeSet<VideoExtension>,
    registry: Arc<Mutex<AnalysisGenerationRegistry>>,
    cancellation: Cancellation,
}

struct ControlState {
    shutting_down: bool,
    pending: Option<DiscoveryRequest>,
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
pub(crate) struct AnalysisDiscoveryRuntime {
    commands: CommandSender,
    control: Arc<DiscoveryControl>,
    start_gate: Mutex<()>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl AnalysisDiscoveryRuntime {
    pub(crate) fn start(commands: CommandSender) -> io::Result<Arc<Self>> {
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
        }))
    }

    pub(crate) fn begin(
        &self,
        roots: Vec<PathBuf>,
        extensions: BTreeSet<VideoExtension>,
    ) -> Result<AnalysisGenerationId, AnalysisDiscoveryError> {
        let _gate = lock(&self.start_gate);
        if lock(&self.control.state).shutting_down {
            return Err(AnalysisDiscoveryError::ShuttingDown);
        }
        let roots = deduplicate_roots(roots);
        if roots.is_empty() {
            return Err(AnalysisDiscoveryError::EmptyRoots);
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
                return Err(AnalysisDiscoveryError::Rejected(reason));
            }
            Reply::Accepted | Reply::Reserved(_) | Reply::Claimed(_) | Reply::Imported { .. } => {
                return Err(AnalysisDiscoveryError::UnexpectedReply);
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
            pending.cancellation.cancel();
        }
        if let Some((_, active)) = &state.active {
            active.cancel();
        }
        state.current_registry = Some(registry);
        state.cancelled_generation = None;
        state.pending = Some(request);
        self.control.wake.notify_one();
        Ok(generation)
    }

    pub(crate) fn cancel(&self) -> Result<(), AnalysisDiscoveryError> {
        let _gate = lock(&self.start_gate);
        let generation = {
            let mut state = lock(&self.control.state);
            if state.shutting_down {
                return Err(AnalysisDiscoveryError::ShuttingDown);
            }
            let generation = state
                .current_registry
                .as_ref()
                .map(|registry| lock(registry).generation);
            if generation.is_some() && state.cancelled_generation == generation {
                return Ok(());
            }
            if let Some(pending) = state.pending.take() {
                pending.cancellation.cancel();
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
                Err(AnalysisDiscoveryError::Rejected(reason))
            }
            Reply::AnalysisStarted { .. }
            | Reply::Reserved(_)
            | Reply::Claimed(_)
            | Reply::Imported { .. } => Err(AnalysisDiscoveryError::UnexpectedReply),
        }
    }

    pub(crate) fn shutdown(&self) -> thread::Result<()> {
        let _gate = lock(&self.start_gate);
        {
            let mut state = lock(&self.control.state);
            state.shutting_down = true;
            if let Some(pending) = state.pending.take() {
                pending.cancellation.cancel();
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
            state.active = Some((request.generation, request.cancellation.clone()));
            request
        };

        run_request(&request, &commands);

        let mut state = lock(&control.state);
        if state.active.as_ref().map(|(generation, _)| *generation) == Some(request.generation) {
            state.active = None;
        }
    }
}

fn run_request(request: &DiscoveryRequest, commands: &CommandSender) {
    let Some(batch_size) = NonZeroUsize::new(DISCOVERY_BATCH_SIZE) else {
        return;
    };
    let mut batch_error = None;
    let outcome = discover(request, batch_size, |rows| {
        if request.cancellation.is_cancelled() {
            return false;
        }
        match commands.submit(Command::Analysis(AnalysisCommand::UpsertRows {
            generation: request.generation,
            rows,
        })) {
            Ok(Reply::Accepted) => true,
            Ok(Reply::Rejected { reason } | Reply::DurabilityUnknown { reason }) => {
                batch_error = Some(reason);
                false
            }
            Ok(reply) => {
                batch_error = Some(format!("unexpected Analysis batch reply: {reply:?}"));
                false
            }
            Err(error) => {
                batch_error = Some(error.to_string());
                false
            }
        }
    });
    if request.cancellation.is_cancelled() {
        return;
    }
    let activity = if let Some(detail) = batch_error {
        tracing::debug!(
            generation = request.generation.0,
            "Analysis discovery batch stopped: {detail}"
        );
        AnalysisActivity::Failed {
            detail: "Analysis discovery could not publish a row batch".to_owned(),
        }
    } else {
        match outcome {
            DiscoveryOutcome::Complete => AnalysisActivity::Discovered,
            DiscoveryOutcome::Cancelled => return,
            DiscoveryOutcome::RowIdExhausted => AnalysisActivity::Failed {
                detail: "Analysis row id space is exhausted".to_owned(),
            },
        }
    };
    match commands.submit(Command::Analysis(AnalysisCommand::SetActivity {
        generation: request.generation,
        activity,
    })) {
        Ok(Reply::Accepted | Reply::Rejected { .. }) => {}
        Ok(reply) => tracing::warn!(
            generation = request.generation.0,
            "unexpected Analysis completion reply: {reply:?}"
        ),
        Err(error) => tracing::warn!(
            generation = request.generation.0,
            "failed to submit Analysis completion: {error}"
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryOutcome {
    Complete,
    Cancelled,
    RowIdExhausted,
}

struct DirectoryTask {
    id: AnalysisRowId,
    path: PathBuf,
    source_root: PathBuf,
    parent: Option<AnalysisRowId>,
}

struct ChildEntry {
    path: PathBuf,
    kind: AnalysisEntryKind,
}

fn discover(
    request: &DiscoveryRequest,
    batch_size: NonZeroUsize,
    mut emit: impl FnMut(Vec<AnalysisRow>) -> bool,
) -> DiscoveryOutcome {
    let mut pending = VecDeque::new();
    let mut batch = Vec::with_capacity(batch_size.get());
    for root in &request.roots {
        if request.cancellation.is_cancelled() {
            return DiscoveryOutcome::Cancelled;
        }
        let id = match insert_native_row(
            &request.registry,
            root.clone(),
            root.clone(),
            None,
            AnalysisEntryKind::Folder,
        ) {
            Ok(id) => id,
            Err(()) => return DiscoveryOutcome::RowIdExhausted,
        };
        batch.push(public_row(id, None, AnalysisEntryKind::Folder, root, None));
        pending.push_back(DirectoryTask {
            id,
            path: root.clone(),
            source_root: root.clone(),
            parent: None,
        });
        if batch.len() == batch_size.get() && !emit_batch(request, &mut emit, &mut batch) {
            return DiscoveryOutcome::Cancelled;
        }
    }
    if !batch.is_empty() && !emit_batch(request, &mut emit, &mut batch) {
        return DiscoveryOutcome::Cancelled;
    }

    while let Some(directory) = pending.pop_front() {
        if request.cancellation.is_cancelled() {
            return DiscoveryOutcome::Cancelled;
        }
        let metadata = match fs::symlink_metadata(&directory.path) {
            Ok(metadata) => metadata,
            Err(error) => {
                if !emit_directory_failure(request, &directory, failure_for(error), &mut emit) {
                    return DiscoveryOutcome::Cancelled;
                }
                continue;
            }
        };
        if is_link_or_reparse(&metadata) {
            if !emit_directory_failure(
                request,
                &directory,
                AnalysisDirectoryFailure::TraversalRefused,
                &mut emit,
            ) {
                return DiscoveryOutcome::Cancelled;
            }
            continue;
        }
        if !metadata.is_dir() {
            if !emit_directory_failure(
                request,
                &directory,
                AnalysisDirectoryFailure::NotDirectory,
                &mut emit,
            ) {
                return DiscoveryOutcome::Cancelled;
            }
            continue;
        }
        if request.cancellation.is_cancelled() {
            return DiscoveryOutcome::Cancelled;
        }
        let entries = match fs::read_dir(&directory.path) {
            Ok(entries) => entries,
            Err(error) => {
                if !emit_directory_failure(request, &directory, failure_for(error), &mut emit) {
                    return DiscoveryOutcome::Cancelled;
                }
                continue;
            }
        };

        let mut children = Vec::new();
        let mut entry_error_count = 0_u32;
        let mut first_entry_error = None;
        for entry in entries {
            if request.cancellation.is_cancelled() {
                return DiscoveryOutcome::Cancelled;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    entry_error_count = entry_error_count.saturating_add(1);
                    first_entry_error.get_or_insert_with(|| error.to_string());
                    continue;
                }
            };
            match classify_entry(&entry, &request.extensions) {
                Ok(Some(kind)) => children.push(ChildEntry {
                    path: entry.path(),
                    kind,
                }),
                Ok(None) => {}
                Err(error) => {
                    entry_error_count = entry_error_count.saturating_add(1);
                    first_entry_error.get_or_insert_with(|| error.to_string());
                }
            }
        }
        children.sort_by(|left, right| left.path.cmp(&right.path));

        for child in children {
            if request.cancellation.is_cancelled() {
                return DiscoveryOutcome::Cancelled;
            }
            let id = match insert_native_row(
                &request.registry,
                child.path.clone(),
                directory.source_root.clone(),
                Some(directory.id),
                child.kind,
            ) {
                Ok(id) => id,
                Err(()) => return DiscoveryOutcome::RowIdExhausted,
            };
            batch.push(public_row(
                id,
                Some(directory.id),
                child.kind,
                &child.path,
                None,
            ));
            if child.kind == AnalysisEntryKind::Folder {
                pending.push_back(DirectoryTask {
                    id,
                    path: child.path,
                    source_root: directory.source_root.clone(),
                    parent: Some(directory.id),
                });
            }
            if batch.len() == batch_size.get() && !emit_batch(request, &mut emit, &mut batch) {
                return DiscoveryOutcome::Cancelled;
            }
        }
        if !batch.is_empty() && !emit_batch(request, &mut emit, &mut batch) {
            return DiscoveryOutcome::Cancelled;
        }
        if let Some(detail) = first_entry_error
            && !emit_directory_failure(
                request,
                &directory,
                AnalysisDirectoryFailure::EntriesUnavailable {
                    count: entry_error_count,
                    detail,
                },
                &mut emit,
            )
        {
            return DiscoveryOutcome::Cancelled;
        }
    }
    DiscoveryOutcome::Complete
}

fn emit_batch(
    request: &DiscoveryRequest,
    emit: &mut impl FnMut(Vec<AnalysisRow>) -> bool,
    batch: &mut Vec<AnalysisRow>,
) -> bool {
    if request.cancellation.is_cancelled() {
        return false;
    }
    emit(std::mem::take(batch))
}

fn emit_directory_failure(
    request: &DiscoveryRequest,
    directory: &DirectoryTask,
    failure: AnalysisDirectoryFailure,
    emit: &mut impl FnMut(Vec<AnalysisRow>) -> bool,
) -> bool {
    if request.cancellation.is_cancelled() {
        return false;
    }
    emit(vec![public_row(
        directory.id,
        directory.parent,
        AnalysisEntryKind::Folder,
        &directory.path,
        Some(failure),
    )])
}

fn insert_native_row(
    registry: &Mutex<AnalysisGenerationRegistry>,
    path: PathBuf,
    source_root: PathBuf,
    parent: Option<AnalysisRowId>,
    kind: AnalysisEntryKind,
) -> Result<AnalysisRowId, ()> {
    lock(registry).insert(path, source_root, parent, kind)
}

fn public_row(
    id: AnalysisRowId,
    parent: Option<AnalysisRowId>,
    kind: AnalysisEntryKind,
    path: &Path,
    directory_failure: Option<AnalysisDirectoryFailure>,
) -> AnalysisRow {
    let name = path.file_name().unwrap_or(path.as_os_str());
    AnalysisRow {
        id,
        parent,
        kind,
        display_name: display_text(name),
        display_path: display_text(path.as_os_str()),
        directory_failure,
    }
}

fn display_text(value: &OsStr) -> AnalysisDisplayText {
    AnalysisDisplayText {
        text: value.to_string_lossy().into_owned(),
        lossy: value.to_str().is_none(),
    }
}

fn deduplicate_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    roots
        .into_iter()
        .filter(|root| !root.as_os_str().is_empty())
        .filter(|root| seen.insert(root.clone()))
        .collect()
}

fn classify_entry(
    entry: &fs::DirEntry,
    extensions: &BTreeSet<VideoExtension>,
) -> io::Result<Option<AnalysisEntryKind>> {
    let file_type = entry.file_type()?;
    if file_type.is_symlink() {
        return Ok(None);
    }
    #[cfg(windows)]
    if is_link_or_reparse(&fs::symlink_metadata(entry.path())?) {
        return Ok(None);
    }
    if file_type.is_dir() {
        Ok(Some(AnalysisEntryKind::Folder))
    } else if file_type.is_file() && matches_extension(&entry.path(), extensions) {
        Ok(Some(AnalysisEntryKind::File))
    } else {
        Ok(None)
    }
}

fn matches_extension(path: &Path, extensions: &BTreeSet<VideoExtension>) -> bool {
    path.extension().is_some_and(|extension| {
        extensions
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(OsStr::new(candidate.as_extension())))
    })
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn failure_for(error: io::Error) -> AnalysisDirectoryFailure {
    match error.kind() {
        io::ErrorKind::NotFound => AnalysisDirectoryFailure::Missing,
        io::ErrorKind::PermissionDenied => AnalysisDirectoryFailure::PermissionDenied,
        io::ErrorKind::NotADirectory => AnalysisDirectoryFailure::NotDirectory,
        _ => AnalysisDirectoryFailure::Unavailable {
            detail: error.to_string(),
        },
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    match condvar.wait(guard) {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::AtomicU64,
        time::{Duration, Instant},
    };

    use crfty_core::{AnalysisDelta, AnalysisSnapshot, EphemeralDelta, fold_analysis};

    use crate::driver::{DriverEvent, DriverHandle};

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_directory(label: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "crfty-analysis-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("fixture directory");
        path
    }

    fn extensions() -> BTreeSet<VideoExtension> {
        [
            VideoExtension::Mp4,
            VideoExtension::Mkv,
            VideoExtension::Avi,
            VideoExtension::Wmv,
        ]
        .into_iter()
        .collect()
    }

    fn request(roots: Vec<PathBuf>) -> DiscoveryRequest {
        DiscoveryRequest {
            generation: AnalysisGenerationId(1),
            roots,
            extensions: extensions(),
            registry: Arc::new(Mutex::new(AnalysisGenerationRegistry::new(
                AnalysisGenerationId(1),
            ))),
            cancellation: Cancellation::new(),
        }
    }

    fn collect(request: &DiscoveryRequest, batch_size: usize) -> Vec<Vec<AnalysisRow>> {
        let mut batches = Vec::new();
        assert_eq!(
            discover(
                request,
                NonZeroUsize::new(batch_size).expect("non-zero batch"),
                |rows| {
                    batches.push(rows);
                    true
                }
            ),
            DiscoveryOutcome::Complete
        );
        batches
    }

    #[test]
    fn streams_deterministic_breadth_first_rows_and_filters_extensions() {
        let root = test_directory("bfs");
        fs::write(root.join("b.mkv"), b"b").expect("b.mkv");
        fs::write(root.join("a.MP4"), b"a").expect("a.MP4");
        fs::write(root.join("notes.txt"), b"n").expect("notes.txt");
        fs::create_dir(root.join("zeta")).expect("zeta");
        fs::write(root.join("zeta/c.avi"), b"c").expect("c.avi");
        fs::create_dir(root.join("alpha")).expect("alpha");
        fs::write(root.join("alpha/d.wmv"), b"d").expect("d.wmv");

        let first_request = request(vec![root.clone()]);
        let first = collect(&first_request, 3);
        let second_request = request(vec![root.clone()]);
        let second = collect(&second_request, 3);
        assert_eq!(first, second);
        assert!(first.iter().all(|batch| batch.len() <= 3));
        let names: Vec<_> = first
            .into_iter()
            .flatten()
            .map(|row| row.display_name.text)
            .collect();
        assert_eq!(
            names,
            vec![
                root.file_name().expect("root name").to_string_lossy(),
                "a.MP4".into(),
                "alpha".into(),
                "b.mkv".into(),
                "zeta".into(),
                "d.wmv".into(),
                "c.avi".into(),
            ]
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn deep_and_wide_trees_keep_parents_before_children_and_batches_bounded() {
        let root = test_directory("deep-wide");
        let mut current = root.clone();
        for depth in 0..20 {
            let next = current.join(format!("depth-{depth:02}"));
            fs::create_dir(&next).expect("deep directory");
            fs::write(current.join(format!("video-{depth:02}.mkv")), b"v").expect("deep video");
            current = next;
        }
        for index in 0..70 {
            fs::write(root.join(format!("wide-{index:03}.mp4")), b"w").expect("wide video");
        }
        let request = request(vec![root.clone()]);
        let batches = collect(&request, 7);
        assert!(batches.len() > 10);
        assert!(batches.iter().all(|batch| batch.len() <= 7));
        let rows: Vec<_> = batches.into_iter().flatten().collect();
        let positions: BTreeMap<_, _> = rows
            .iter()
            .enumerate()
            .map(|(position, row)| (row.id, position))
            .collect();
        for row in &rows {
            if let Some(parent) = row.parent {
                assert!(positions[&parent] < positions[&row.id]);
            }
        }
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn duplicate_roots_are_removed_without_normalizing_native_spelling() {
        let root = test_directory("duplicates");
        let roots = deduplicate_roots(vec![root.clone(), root.clone(), PathBuf::new()]);
        assert_eq!(roots, vec![root.clone()]);
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn disappearing_directory_is_a_typed_row_failure_and_does_not_abort() {
        let root = test_directory("disappearing");
        let doomed = root.join("a-doomed");
        fs::create_dir(&doomed).expect("doomed directory");
        let kept = root.join("z-kept");
        fs::create_dir(&kept).expect("kept directory");
        fs::write(kept.join("movie.mkv"), b"v").expect("movie");
        let request = request(vec![root.clone()]);
        let mut batches = Vec::new();
        let mut removed = false;
        assert_eq!(
            discover(&request, NonZeroUsize::new(32).expect("batch"), |rows| {
                if !removed && rows.iter().any(|row| row.display_name.text == "a-doomed") {
                    fs::remove_dir(&doomed).expect("remove doomed");
                    removed = true;
                }
                batches.push(rows);
                true
            }),
            DiscoveryOutcome::Complete
        );
        let rows: Vec<_> = batches.into_iter().flatten().collect();
        assert!(rows.iter().any(|row| {
            row.display_name.text == "a-doomed"
                && row.directory_failure == Some(AnalysisDirectoryFailure::Missing)
        }));
        assert!(rows.iter().any(|row| row.display_name.text == "movie.mkv"));
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn cancellation_is_checked_before_the_next_batch() {
        let root = test_directory("cancel");
        for index in 0..10 {
            fs::write(root.join(format!("video-{index:02}.mkv")), b"v").expect("video");
        }
        let request = request(vec![root.clone()]);
        let mut batches = Vec::new();
        assert_eq!(
            discover(&request, NonZeroUsize::new(2).expect("batch"), |rows| {
                batches.push(rows);
                request.cancellation.cancel();
                true
            }),
            DiscoveryOutcome::Cancelled
        );
        assert_eq!(batches.len(), 1);
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
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
        let runtime = AnalysisDiscoveryRuntime::start(driver.commands.clone()).expect("runtime");
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

    #[cfg(unix)]
    #[test]
    fn non_unicode_paths_remain_native_and_display_is_marked_lossy() {
        use std::os::unix::ffi::OsStringExt;

        let root = test_directory("non-unicode");
        let name = std::ffi::OsString::from_vec(b"movie-\x80.mkv".to_vec());
        let path = root.join(&name);
        fs::write(&path, b"v").expect("non-unicode video");
        let request = request(vec![root.clone()]);
        let rows: Vec<_> = collect(&request, 32).into_iter().flatten().collect();
        let row = rows
            .iter()
            .find(|row| row.kind == AnalysisEntryKind::File)
            .expect("file row");
        assert!(row.display_name.lossy);
        let native = lock(&request.registry)
            .rows
            .get(&row.id)
            .expect("native row")
            .path
            .clone();
        assert_eq!(native, path);
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_and_unc_root_spellings_are_retained() {
        let roots = vec![
            PathBuf::from(r"\\?\C:\Videos\Long Folder"),
            PathBuf::from(r"\\server\share\Videos"),
        ];
        let request = request(roots.clone());
        let _batches = collect(&request, 32);
        let native: Vec<_> = lock(&request.registry)
            .rows
            .values()
            .map(|row| row.path.clone())
            .collect();
        assert_eq!(native, roots);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directories_are_not_traversed() {
        let root = test_directory("symlink");
        let target = test_directory("symlink-target");
        fs::write(target.join("hidden.mkv"), b"v").expect("hidden video");
        std::os::unix::fs::symlink(&target, root.join("linked")).expect("directory symlink");
        fs::write(root.join("visible.mkv"), b"v").expect("visible video");
        let request = request(vec![root.clone()]);
        let names: Vec<_> = collect(&request, 32)
            .into_iter()
            .flatten()
            .map(|row| row.display_name.text)
            .collect();
        assert!(names.contains(&"visible.mkv".to_owned()));
        assert!(!names.contains(&"hidden.mkv".to_owned()));
        assert!(!names.contains(&"linked".to_owned()));
        fs::remove_dir_all(root).expect("remove fixture");
        fs::remove_dir_all(target).expect("remove target");
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_directory_is_typed_when_permissions_are_enforced() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_directory("permission");
        let secret = root.join("secret");
        fs::create_dir(&secret).expect("secret");
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read_dir(&secret).is_ok() {
            fs::set_permissions(&secret, fs::Permissions::from_mode(0o755)).expect("restore");
            fs::remove_dir_all(root).expect("remove fixture");
            return;
        }
        let request = request(vec![root.clone()]);
        let rows: Vec<_> = collect(&request, 32).into_iter().flatten().collect();
        assert!(rows.iter().any(|row| {
            row.display_name.text == "secret"
                && row.directory_failure == Some(AnalysisDirectoryFailure::PermissionDenied)
        }));
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o755)).expect("restore");
        fs::remove_dir_all(root).expect("remove fixture");
    }
}
