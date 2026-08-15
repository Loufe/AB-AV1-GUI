//! Generation-scoped Analysis discovery and Basic Scan execution.
//!
//! One dedicated worker owns traversal order. New requests replace any
//! pending request and cancel the active one, while the core reducer remains
//! the final stale-generation gate. Native paths never cross the IPC model.

mod basic_scan;
mod discovery;
mod runtime;

pub(crate) use self::runtime::AnalysisRuntime;

use std::{
    fmt,
    path::PathBuf,
    sync::{Condvar, Mutex, MutexGuard},
};

use crfty_core::AnalysisRowId;

use crate::driver::SubmitError;

#[derive(Debug)]
pub enum AnalysisError {
    EmptyRoots,
    MissingTools,
    ShuttingDown,
    Submit(SubmitError),
    Rejected(String),
    UnexpectedReply,
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRoots => {
                formatter.write_str("Analysis discovery requires at least one root")
            }
            Self::MissingTools => formatter.write_str("ffprobe is unavailable"),
            Self::ShuttingDown => formatter.write_str("Analysis runtime is shutting down"),
            Self::Submit(error) => write!(formatter, "failed to submit Analysis command: {error}"),
            Self::Rejected(reason) => formatter.write_str(reason),
            Self::UnexpectedReply => {
                formatter.write_str("driver returned an unexpected Analysis reply")
            }
        }
    }
}

impl std::error::Error for AnalysisError {}

impl From<SubmitError> for AnalysisError {
    fn from(error: SubmitError) -> Self {
        Self::Submit(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnalysisNativeRow {
    pub(crate) path: PathBuf,
    pub(crate) source_root: PathBuf,
    pub(crate) parent: Option<AnalysisRowId>,
    pub(crate) kind: NativeEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeEntryKind {
    Folder,
    File,
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

/// Fixture helpers shared by the discovery and runtime test modules.
#[cfg(test)]
pub(super) mod test_support {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crfty_core::VideoExtension;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[expect(clippy::expect_used, reason = "fixture setup")]
    pub(super) fn test_directory(label: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "crfty-analysis-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("fixture directory");
        path
    }

    pub(super) fn extensions() -> BTreeSet<VideoExtension> {
        [
            VideoExtension::Mp4,
            VideoExtension::Mkv,
            VideoExtension::Avi,
            VideoExtension::Wmv,
        ]
        .into_iter()
        .collect()
    }
}
