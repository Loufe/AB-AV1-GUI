//! Engine-owned data-directory lock.
//!
//! One exclusive OS lock on a dedicated file in the data directory enforces
//! the single-writer rule as a fact about the machine, not the process: a
//! second instance fails to acquire it and reports [`DataLockError::AlreadyHeld`]
//! instead of racing the journal. The lock lives on its own file rather than
//! the journal handle so the journal can be closed and reopened (compaction's
//! writer barrier) without ever releasing exclusivity.
//!
//! The held OS lock is the signal, never the file's existence: the lock file
//! is left behind on exit and a leftover file from a crashed process locks
//! cleanly on the next start.

use std::{
    fmt,
    fs::{File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
};

pub(crate) const LOCK_FILE_NAME: &str = "crfty.lock";

/// Holds the exclusive data-directory lock. The OS releases the lock when
/// this value drops (or the process dies), so ownership tracks lifetime.
#[derive(Debug)]
pub(crate) struct DataLock {
    _file: File,
}

#[derive(Debug)]
pub(crate) enum DataLockError {
    /// Another process holds the lock: a second instance of the application.
    AlreadyHeld { path: PathBuf },
    Io {
        context: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for DataLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyHeld { path } => write!(
                formatter,
                "another instance holds the data lock at {}",
                path.display()
            ),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl std::error::Error for DataLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AlreadyHeld { .. } => None,
            Self::Io { source, .. } => Some(source),
        }
    }
}

impl DataLock {
    pub(crate) fn acquire(directory: &Path) -> Result<Self, DataLockError> {
        std::fs::create_dir_all(directory).map_err(|source| DataLockError::Io {
            context: "failed to create data directory",
            source,
        })?;
        let path = directory.join(LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|source| DataLockError::Io {
                context: "failed to open data lock file",
                source,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(DataLockError::AlreadyHeld { path }),
            Err(TryLockError::Error(source)) => Err(DataLockError::Io {
                context: "failed to lock data directory",
                source,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DataLock, DataLockError};

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let held = DataLock::acquire(directory.path()).expect("first acquisition");
        assert!(matches!(
            DataLock::acquire(directory.path()),
            Err(DataLockError::AlreadyHeld { .. })
        ));
        drop(held);
        let _reacquired = DataLock::acquire(directory.path()).expect("reacquisition after drop");
        assert!(directory.path().join(super::LOCK_FILE_NAME).is_file());
    }

    #[test]
    fn stale_lock_file_from_a_dead_process_locks_cleanly() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join(super::LOCK_FILE_NAME), b"stale")
            .expect("stale lock file");
        let _held = DataLock::acquire(directory.path()).expect("acquisition over stale file");
    }
}
