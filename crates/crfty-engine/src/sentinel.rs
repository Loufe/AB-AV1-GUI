//! Crash sentinel: a marker file whose presence at boot means the previous
//! run died without closing durable state cleanly (OBS's pattern, #33 §12).
//!
//! Armed immediately after the data-directory lock is acquired and disarmed
//! when the driver exits cleanly. Only the lock holder ever touches the file,
//! so presence at arm time is exactly "the last lock holder never reached a
//! clean driver shutdown". Every I/O failure degrades to a log line: crash
//! reporting must never block startup or shutdown.

use std::path::{Path, PathBuf};

pub const SENTINEL_FILE_NAME: &str = "crfty.sentinel";

#[derive(Debug)]
pub(crate) struct CrashSentinel {
    path: PathBuf,
    armed: bool,
    previous_run_abnormal: bool,
}

impl CrashSentinel {
    /// Arms the sentinel for this run, recording whether the previous run
    /// left it behind (an abnormal shutdown). The old file's contents — our
    /// own version/pid stamp, never a user path — are logged before the
    /// rewrite so the log identifies which run died.
    pub fn arm(data_dir: &Path) -> Self {
        let path = data_dir.join(SENTINEL_FILE_NAME);
        let previous_run_abnormal = path.exists();
        if previous_run_abnormal {
            match std::fs::read_to_string(&path) {
                Ok(contents) => tracing::warn!(
                    "previous run ended abnormally; sentinel was left by {}",
                    contents.trim()
                ),
                Err(error) => {
                    tracing::warn!(
                        "previous run ended abnormally; sentinel is unreadable: {error}"
                    );
                }
            }
        }
        let stamp = format!(
            "crfty {} pid {}\n",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
        let armed = match std::fs::write(&path, stamp) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    "failed to write the crash sentinel; an abnormal end of this run will go \
                     unreported: {error}"
                );
                false
            }
        };
        Self {
            path,
            armed,
            previous_run_abnormal,
        }
    }

    /// Whether the previous run left its sentinel behind at arm time.
    pub fn previous_run_abnormal(&self) -> bool {
        self.previous_run_abnormal
    }

    /// Removes the sentinel: this run's durable state closed cleanly. Called
    /// only on the clean shutdown path — a panic must leave the file behind
    /// for the next boot to find.
    pub fn disarm(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(
                "failed to remove the crash sentinel; the next start will report a false abnormal \
                 shutdown: {error}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CrashSentinel, SENTINEL_FILE_NAME};

    #[test]
    fn a_clean_arm_disarm_cycle_reports_no_abnormal_shutdown() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut sentinel = CrashSentinel::arm(directory.path());
        assert!(!sentinel.previous_run_abnormal());
        assert!(directory.path().join(SENTINEL_FILE_NAME).exists());
        sentinel.disarm();
        assert!(!directory.path().join(SENTINEL_FILE_NAME).exists());

        // The next run starts clean.
        let next = CrashSentinel::arm(directory.path());
        assert!(!next.previous_run_abnormal());
    }

    #[test]
    fn a_leftover_sentinel_is_reported_as_abnormal_and_rearmed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let crashed = CrashSentinel::arm(directory.path());
        assert!(!crashed.previous_run_abnormal());
        // The previous run "crashes": its sentinel is never disarmed.
        drop(crashed);

        let mut sentinel = CrashSentinel::arm(directory.path());
        assert!(sentinel.previous_run_abnormal());
        assert!(directory.path().join(SENTINEL_FILE_NAME).exists());
        sentinel.disarm();
        assert!(!directory.path().join(SENTINEL_FILE_NAME).exists());
    }

    #[test]
    fn disarm_is_idempotent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut sentinel = CrashSentinel::arm(directory.path());
        sentinel.disarm();
        sentinel.disarm();
        assert!(!directory.path().join(SENTINEL_FILE_NAME).exists());
    }
}
