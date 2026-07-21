//! Engine-owned structured logging: a reconfigurable file/stderr sink with
//! privacy scrubbing applied inside the write path.
//!
//! Initialized by the shell before any durable-state work (#33 §12). The
//! subscriber layers never change after [`init`]; everything a settings
//! change can alter — the anonymization toggle, the configured-folder
//! placeholders, the log folder — lives behind one shared [`LogControl`]
//! that [`reconfigure`] swaps atomically. Scrubbing happens inside the
//! write path itself, so there is no window where a sink exists without its
//! privacy filter, and a folder switch that fails keeps the old sink.
//!
//! Nothing in this module calls `tracing` while holding the sink lock: the
//! sink is what tracing writes into, and re-entry would deadlock. Internal
//! sink failures report through stderr directly, once.

mod privacy;
mod rolling;

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use crfty_core::Settings;
use tracing_subscriber::{
    Layer,
    filter::{LevelFilter, Targets},
    fmt::{self, MakeWriter},
    layer::SubscriberExt,
};

use privacy::PathScrubber;
use rolling::{FILE_CAP_BYTES, launch_file_name, prune_selection, rotation_plan};

static CONTROL: OnceLock<Arc<LogControl>> = OnceLock::new();

/// Result of a retroactive log scrub, counted per file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrubOutcome {
    pub total: u32,
    pub modified: u32,
    pub failed: u32,
}

struct LogControl {
    state: Mutex<SinkState>,
}

struct SinkState {
    scrub_enabled: bool,
    scrubber: PathScrubber,
    /// `<data_dir>/logs`, used whenever no custom folder is configured or
    /// the custom folder is unusable.
    default_dir: PathBuf,
    /// The `Settings::log_folder` value last applied, so [`reconfigure`]
    /// only re-opens the sink when it actually changes.
    configured_dir: Option<PathBuf>,
    active_dir: PathBuf,
    base_name: String,
    file: Option<fs::File>,
    written: u64,
    write_error_reported: bool,
}

impl LogControl {
    fn lock(&self) -> MutexGuard<'_, SinkState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Scrubs (when enabled) and writes one line to the target sink.
    fn write_line(&self, target: SinkTarget, line: &str) {
        let mut state = self.lock();
        let rendered = if state.scrub_enabled {
            state.scrubber.scrub_line(line)
        } else {
            line.to_owned()
        };
        match target {
            SinkTarget::Stderr => {
                let mut stderr = io::stderr().lock();
                let _ = writeln!(stderr, "{rendered}");
            }
            SinkTarget::File => write_file_line(&mut state, &rendered),
        }
    }
}

fn write_file_line(state: &mut SinkState, line: &str) {
    if state.file.is_none() {
        return;
    }
    let pending = line.len() as u64 + 1;
    if state.written + pending > FILE_CAP_BYTES {
        rotate(state);
    }
    let Some(file) = state.file.as_mut() else {
        return;
    };
    let result = file
        .write_all(line.as_bytes())
        .and_then(|()| file.write_all(b"\n"));
    match result {
        Ok(()) => state.written += pending,
        Err(error) => {
            // The sink is what tracing writes into; report the loss directly
            // to stderr, once, and keep the app running (#33: logging must
            // never abort work).
            state.file = None;
            if !state.write_error_reported {
                state.write_error_reported = true;
                eprintln!("log file write failed; continuing without a log file: {error}");
            }
        }
    }
}

/// Executes the rename cascade and reopens a fresh base file. Failures leave
/// the sink fileless rather than panicking; the next launch starts clean.
fn rotate(state: &mut SinkState) {
    state.file = None;
    let plan = rotation_plan(&state.active_dir, &state.base_name);
    if let Err(error) = fs::remove_file(&plan.discard)
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!("failed to discard oldest rolled log: {error}");
    }
    for (from, to) in &plan.renames {
        if let Err(error) = fs::rename(from, to)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("failed to roll log file: {error}");
        }
    }
    match open_append(&state.active_dir.join(&state.base_name)) {
        Ok((file, written)) => {
            state.file = Some(file);
            state.written = written;
        }
        Err(error) => {
            if !state.write_error_reported {
                state.write_error_reported = true;
                eprintln!("failed to reopen log file after rotation: {error}");
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SinkTarget {
    File,
    Stderr,
}

#[derive(Clone)]
struct SinkMakeWriter {
    control: Arc<LogControl>,
    target: SinkTarget,
}

impl<'a> MakeWriter<'a> for SinkMakeWriter {
    type Writer = LineBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        LineBuffer {
            control: Arc::clone(&self.control),
            target: self.target,
            buffer: Vec::new(),
        }
    }
}

/// Buffers formatted event bytes and hands complete lines to the control, so
/// scrubbing always sees whole lines.
struct LineBuffer {
    control: Arc<LogControl>,
    target: SinkTarget,
    buffer: Vec<u8>,
}

impl LineBuffer {
    fn drain_complete_lines(&mut self) {
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=position).collect();
            line.pop();
            let text = String::from_utf8_lossy(&line);
            self.control.write_line(self.target, &text);
        }
    }
}

impl io::Write for LineBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_complete_lines();
        Ok(())
    }
}

impl Drop for LineBuffer {
    fn drop(&mut self) {
        self.drain_complete_lines();
        if !self.buffer.is_empty() {
            let text = String::from_utf8_lossy(&self.buffer).into_owned();
            self.control.write_line(self.target, &text);
        }
    }
}

/// Initializes the global logging pipeline. Idempotent; never fails the
/// application — every degradation (unusable folder, unopenable file) is
/// reported and the sink continues on stderr alone.
///
/// The settings peek is deliberately lenient and read-only: an unreadable or
/// invalid config yields defaults here, and the driver later owns the real
/// load (including quarantine). Startup ordering per #33 §12 — tracing first,
/// then lock, then durable state.
pub fn init(default_log_dir: &Path, config_path: &Path) {
    if CONTROL.get().is_some() {
        return;
    }
    let settings = peek_settings(config_path);
    let mut scrubber = PathScrubber::new(cfg!(windows));
    scrubber.set_configured_folders(
        settings.last_input_folder.as_deref(),
        settings.output.separate_folder.as_deref(),
    );
    let mut notes: Vec<String> = Vec::new();
    let active_dir = resolve_directory(default_log_dir, settings.log_folder.as_deref(), &mut notes);
    let base_name = launch_file_name(unix_now_seconds());
    let file = match open_append(&active_dir.join(&base_name)) {
        Ok((file, _written)) => Some(file),
        Err(error) => {
            notes.push(format!(
                "failed to open a log file; continuing on stderr only: {error}"
            ));
            None
        }
    };
    let control = Arc::new(LogControl {
        state: Mutex::new(SinkState {
            scrub_enabled: settings.privacy.anonymize_logs,
            scrubber,
            default_dir: default_log_dir.to_path_buf(),
            configured_dir: settings.log_folder.clone(),
            active_dir: active_dir.clone(),
            base_name,
            file,
            written: 0,
            write_error_reported: false,
        }),
    });
    if CONTROL.set(Arc::clone(&control)).is_err() {
        return;
    }
    let file_targets = Targets::new()
        .with_default(LevelFilter::INFO)
        .with_target("crfty_core", LevelFilter::DEBUG)
        .with_target("crfty_engine", LevelFilter::DEBUG)
        .with_target("crfty_shell", LevelFilter::DEBUG)
        .with_target("ab_av1", LevelFilter::DEBUG);
    let subscriber = tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(SinkMakeWriter {
                    control: Arc::clone(&control),
                    target: SinkTarget::File,
                })
                .with_filter(file_targets),
        )
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(SinkMakeWriter {
                    control,
                    target: SinkTarget::Stderr,
                })
                .with_filter(LevelFilter::INFO),
        );
    // A pre-installed subscriber (tests) keeps ownership; the sink still
    // serves reconfigure and scrub.
    let _ = tracing::subscriber::set_global_default(subscriber);
    for note in notes {
        tracing::warn!("{note}");
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        directory = %active_dir.display(),
        "logging started"
    );
    prune_old_launches(&active_dir);
}

/// Applies changed privacy/log-folder settings to the live sink. A folder
/// switch opens the new file first and only then swaps, so a failure keeps
/// the current sink (the acceptance rule: switching can never lose it).
pub fn reconfigure(settings: &Settings) {
    let Some(control) = CONTROL.get() else {
        return;
    };
    let mut state = control.lock();
    state.scrub_enabled = settings.privacy.anonymize_logs;
    state.scrubber.set_configured_folders(
        settings.last_input_folder.as_deref(),
        settings.output.separate_folder.as_deref(),
    );
    if state.configured_dir == settings.log_folder {
        return;
    }
    state.configured_dir = settings.log_folder.clone();
    let target = settings
        .log_folder
        .clone()
        .unwrap_or_else(|| state.default_dir.clone());
    if target == state.active_dir {
        return;
    }
    let base_name = launch_file_name(unix_now_seconds());
    let opened = fs::create_dir_all(&target)
        .map_err(|error| error.to_string())
        .and_then(|()| open_append(&target.join(&base_name)).map_err(|error| error.to_string()));
    let message = match opened {
        Ok((file, written)) => {
            let previous = std::mem::replace(&mut state.active_dir, target);
            state.base_name = base_name;
            state.file = Some(file);
            state.written = written;
            state.write_error_reported = false;
            Ok(previous)
        }
        Err(error) => Err(error),
    };
    drop(state);
    match message {
        Ok(previous) => {
            tracing::info!(previous = %previous.display(), "log folder switched");
        }
        Err(error) => {
            tracing::error!("failed to switch log folder; keeping the current one: {error}");
        }
    }
}

/// Retroactively anonymizes every `*.log` file in the active log folder,
/// regardless of the live anonymization toggle (parity with V2's Scrub Logs
/// button). Rewrites are atomic (temp file + rename), which V2's in-place
/// rewrite was not; the active file is closed around its own rewrite and
/// reopened after, all under the sink lock so no line is written unscrubbed
/// in between.
pub fn scrub_log_files() -> Result<ScrubOutcome, String> {
    let Some(control) = CONTROL.get() else {
        return Err("logging is not initialized".to_owned());
    };
    let mut state = control.lock();
    let directory = state.active_dir.clone();
    let scrubber = state.scrubber.clone();
    let active_path = directory.join(&state.base_name);
    let had_file = state.file.take().is_some();
    let mut outcome = ScrubOutcome {
        total: 0,
        modified: 0,
        failed: 0,
    };
    let mut failures: Vec<String> = Vec::new();
    for path in log_files_in(&directory) {
        outcome.total += 1;
        match scrub_one_file(&path, &scrubber) {
            Ok(true) => outcome.modified += 1,
            Ok(false) => {}
            Err(error) => {
                outcome.failed += 1;
                failures.push(format!("{}: {error}", path.display()));
            }
        }
    }
    if had_file {
        match open_append(&active_path) {
            Ok((file, written)) => {
                state.file = Some(file);
                state.written = written;
            }
            Err(error) => failures.push(format!("failed to reopen the active log: {error}")),
        }
    }
    drop(state);
    for failure in failures {
        tracing::warn!("log scrub: {failure}");
    }
    tracing::info!(
        total = outcome.total,
        modified = outcome.modified,
        failed = outcome.failed,
        "log scrub finished"
    );
    Ok(outcome)
}

/// Scrubs one file; `Ok(true)` when the content changed and was atomically
/// replaced.
fn scrub_one_file(path: &Path, scrubber: &PathScrubber) -> Result<bool, io::Error> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let scrubbed = scrub_text(&text, scrubber);
    if scrubbed == text {
        return Ok(false);
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(scrubbed.as_bytes())?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(true)
}

/// Scrubs a whole file body line by line, preserving original line endings.
fn scrub_text(text: &str, scrubber: &PathScrubber) -> String {
    let mut output = String::with_capacity(text.len());
    for segment in text.split_inclusive('\n') {
        let body = segment.trim_end_matches(['\r', '\n']);
        let ending = segment.get(body.len()..).unwrap_or("");
        output.push_str(&scrubber.scrub_line(body));
        output.push_str(ending);
    }
    output
}

fn peek_settings(config_path: &Path) -> Settings {
    fs::read(config_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
        .unwrap_or_default()
}

fn resolve_directory(
    default_dir: &Path,
    configured: Option<&Path>,
    notes: &mut Vec<String>,
) -> PathBuf {
    if let Some(configured) = configured {
        match fs::create_dir_all(configured) {
            Ok(()) => return configured.to_path_buf(),
            Err(error) => notes.push(format!(
                "configured log folder is unusable, falling back to the default: {error}"
            )),
        }
    }
    if let Err(error) = fs::create_dir_all(default_dir) {
        notes.push(format!("failed to create the log folder: {error}"));
    }
    default_dir.to_path_buf()
}

/// Opens (creating if needed) a log file for appending and reports its
/// current length so the rotation budget includes pre-existing content.
fn open_append(path: &Path) -> Result<(fs::File, u64), io::Error> {
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let written = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    Ok((file, written))
}

fn log_files_in(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "log") && path.is_file()
        })
        .collect();
    paths.sort();
    paths
}

fn prune_old_launches(directory: &Path) {
    let names: Vec<String> = log_files_in(directory)
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let deletions = prune_selection(&names);
    if deletions.is_empty() {
        return;
    }
    let mut removed = 0u32;
    for name in &deletions {
        match fs::remove_file(directory.join(name)) {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!("failed to prune old log file: {error}"),
        }
    }
    tracing::debug!(removed, "pruned old launch logs");
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{ScrubOutcome, privacy::PathScrubber, scrub_one_file, scrub_text};

    #[test]
    fn scrub_text_preserves_line_endings() {
        let scrubber = PathScrubber::new(false);
        let text = "probing /mnt/media/sample.mkv\r\nplain line\nno trailing newline";
        let scrubbed = scrub_text(text, &scrubber);
        assert_eq!(
            scrubbed,
            "probing folder_d5127c96b35c/file_8d5a8c8c9e18.mkv\r\nplain line\nno trailing newline"
        );
    }

    #[test]
    fn scrub_one_file_rewrites_only_when_content_changes() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("crfty_test.log");
        std::fs::write(&path, "queued /mnt/media/sample.mkv\n").expect("fixture log");
        let scrubber = PathScrubber::new(false);
        assert!(scrub_one_file(&path, &scrubber).expect("first scrub"));
        let scrubbed = std::fs::read_to_string(&path).expect("scrubbed content");
        assert_eq!(
            scrubbed,
            "queued folder_d5127c96b35c/file_8d5a8c8c9e18.mkv\n"
        );
        // Second pass: idempotent, no rewrite.
        assert!(!scrub_one_file(&path, &scrubber).expect("second scrub"));
    }

    #[test]
    fn scrub_outcome_counts_are_plain_data() {
        let outcome = ScrubOutcome {
            total: 3,
            modified: 1,
            failed: 0,
        };
        assert_eq!(outcome.total, 3);
        assert_eq!(outcome.modified, 1);
    }
}
