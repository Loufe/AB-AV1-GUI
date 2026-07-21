//! Desktop actions on the user's behalf: open a path with its default
//! program and reveal it in the system file manager.
//!
//! These act on frontend-chosen paths and touch no domain state, so they
//! bypass the reducer entirely — the shell calls them directly. Both check
//! existence first: the queue routinely outlives its files (converted
//! replacements, user deletions), and a stale row must produce a clean
//! rejection instead of whatever the desktop does with a dangling path.

use std::path::{Path, PathBuf};

/// Why a desktop action did not happen. `Missing` is the caller's problem
/// (a stale path — reject it); `Failed` is the desktop's (surface it).
#[derive(Debug)]
pub enum OsActionError {
    Missing { path: PathBuf },
    Failed { message: String },
}

/// Opens a file or folder with the operating system's default program.
pub fn open_path(path: &Path) -> Result<(), OsActionError> {
    checked(path)?;
    opener::open(path).map_err(|error| failed("open", path, &error))
}

/// Reveals a path selected in the system file manager.
pub fn reveal_path(path: &Path) -> Result<(), OsActionError> {
    checked(path)?;
    opener::reveal(path).map_err(|error| failed("reveal", path, &error))
}

/// Opens a URL in the default browser. Callers pass engine-known URLs (the
/// GitHub release page) — never webview-chosen ones — so no existence or
/// scheme check applies.
pub fn open_url(url: &str) -> Result<(), OsActionError> {
    opener::open(url).map_err(|error| {
        tracing::warn!("failed to open {url}: {error}");
        OsActionError::Failed {
            message: format!("failed to open {url}: {error}"),
        }
    })
}

fn checked(path: &Path) -> Result<(), OsActionError> {
    if path.exists() {
        Ok(())
    } else {
        Err(OsActionError::Missing {
            path: path.to_path_buf(),
        })
    }
}

fn failed(action: &str, path: &Path, error: &opener::OpenError) -> OsActionError {
    tracing::warn!("failed to {action} {}: {error}", path.display());
    OsActionError::Failed {
        message: format!("failed to {action} {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{OsActionError, open_path, reveal_path};

    // Only the missing-path guard is testable: succeeding would launch real
    // desktop programs, so the opener calls themselves stay uncovered here.
    #[test]
    fn missing_paths_are_rejected_before_reaching_the_desktop() {
        let path = Path::new("/nonexistent/crfty-os-actions-test");
        assert!(matches!(
            open_path(path),
            Err(OsActionError::Missing { path: reported }) if reported == path
        ));
        assert!(matches!(
            reveal_path(path),
            Err(OsActionError::Missing { path: reported }) if reported == path
        ));
    }
}
