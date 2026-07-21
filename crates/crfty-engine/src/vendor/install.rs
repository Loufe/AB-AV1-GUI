//! Staged atomic vendor installs. Everything mutable happens under
//! `staging/`; the previous install and `current.json` are untouched until
//! the new install is complete, synced, and renamed into place. Any
//! interruption at any point leaves only staging debris (cleaned by the next
//! discovery) and never a half-replaced tool set (ADR-010).

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use super::{
    discovery::{CURRENT_FILE_NAME, INSTALLS_DIR_NAME, InstalledMetadata, STAGING_DIR_NAME},
    download::{self, DownloadError, Fetch},
    extract::{self, ExtractSpec},
    manifest::VendorManifest,
};
use crate::filesystem;

#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    Cancelled,
    Failed(String),
}

/// Phase progress emitted while an install runs. `Downloading` fires per
/// chunk (the caller throttles); `Installing` fires once, when the verified
/// archive moves into extraction and promotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallProgress {
    Downloading { received: u64, total: Option<u64> },
    Installing,
}

impl From<DownloadError> for InstallError {
    fn from(error: DownloadError) -> Self {
        match error {
            DownloadError::Cancelled => Self::Cancelled,
            DownloadError::Failed(detail) => Self::Failed(detail),
        }
    }
}

/// Downloads, verifies, extracts, and atomically activates the manifest's
/// FFmpeg build under `vendor_root`. `cancelled` aborts between download
/// chunks and between phases — never mid-promote.
pub fn install(
    vendor_root: &Path,
    manifest: &VendorManifest,
    fetch: &dyn Fetch,
    progress: &mut dyn FnMut(InstallProgress),
    cancelled: &AtomicBool,
) -> Result<InstalledMetadata, InstallError> {
    let staging = vendor_root.join(STAGING_DIR_NAME);
    std::fs::create_dir_all(&staging)
        .map_err(|error| failed(format!("failed to create the staging directory: {error}")))?;
    let archive = download::download_verified(
        fetch,
        manifest.url,
        manifest.sha256_hex,
        &staging,
        &mut |received, total| progress(InstallProgress::Downloading { received, total }),
        cancelled,
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    progress(InstallProgress::Installing);
    let staged = tempfile::Builder::new()
        .prefix("install-")
        .tempdir_in(&staging)
        .map_err(|error| failed(format!("failed to create the staging install: {error}")))?;
    let spec = ExtractSpec {
        ffmpeg_entry: manifest.ffmpeg_entry.to_owned(),
        ffprobe_entry: manifest.ffprobe_entry.to_owned(),
        max_extracted_bytes: manifest.max_extracted_bytes,
        kind: manifest.archive,
    };
    let binaries = extract::extract_binaries(archive.path(), &spec, staged.path())
        .map_err(InstallError::Failed)?;
    drop(archive);
    if cancelled.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    let metadata = promote(vendor_root, manifest, &binaries, staged)?;
    prune_older_installs(vendor_root, manifest.build);
    Ok(metadata)
}

/// Moves the staged install into `installs/<version>/` and durably replaces
/// `current.json`. The one non-additive step — removing a same-version
/// directory left by a broken earlier install — happens before the record
/// points anywhere new, so a crash inside it still leaves the previous
/// version active.
fn promote(
    vendor_root: &Path,
    manifest: &VendorManifest,
    binaries: &extract::ExtractedBinaries,
    staged: tempfile::TempDir,
) -> Result<InstalledMetadata, InstallError> {
    let installs = vendor_root.join(INSTALLS_DIR_NAME);
    std::fs::create_dir_all(&installs)
        .map_err(|error| failed(format!("failed to create the installs directory: {error}")))?;
    let target = installs.join(manifest.build);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|error| {
            failed(format!(
                "failed to clear a broken same-version install: {error}"
            ))
        })?;
    }
    let metadata = InstalledMetadata {
        version: manifest.build.to_owned(),
        ffmpeg: relative_binary_path(manifest.build, &binaries.ffmpeg)?,
        ffprobe: relative_binary_path(manifest.build, &binaries.ffprobe)?,
        ffmpeg_revision: manifest.build.to_owned(),
        encoder_revision: manifest.build.to_owned(),
    };
    let staged_path = staged.keep();
    if let Err(error) = std::fs::rename(&staged_path, &target) {
        let _best_effort = std::fs::remove_dir_all(&staged_path);
        return Err(failed(format!(
            "failed to activate the staged install: {error}"
        )));
    }
    filesystem::sync_parent(&target)
        .map_err(|error| failed(format!("failed to sync the installs directory: {error}")))?;
    let serialized = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| failed(format!("failed to serialize the install record: {error}")))?;
    let mut record = tempfile::NamedTempFile::new_in(vendor_root)
        .map_err(|error| failed(format!("failed to stage the install record: {error}")))?;
    record
        .write_all(&serialized)
        .map_err(|error| failed(format!("failed to write the install record: {error}")))?;
    record
        .as_file()
        .sync_all()
        .map_err(|error| failed(format!("failed to sync the install record: {error}")))?;
    let current = vendor_root.join(CURRENT_FILE_NAME);
    record
        .persist(&current)
        .map_err(|error| failed(format!("failed to publish the install record: {error}")))?;
    filesystem::sync_parent(&current)
        .map_err(|error| failed(format!("failed to sync the vendor directory: {error}")))?;
    Ok(metadata)
}

/// The vendor-root-relative path recorded for a staged binary:
/// `installs/<version>/bin/<file name>`.
fn relative_binary_path(version: &str, staged_binary: &Path) -> Result<PathBuf, InstallError> {
    let file_name = staged_binary
        .file_name()
        .ok_or_else(|| failed("a staged binary has no file name".to_owned()))?;
    Ok(PathBuf::from(INSTALLS_DIR_NAME)
        .join(version)
        .join("bin")
        .join(file_name))
}

/// Best-effort cleanup of superseded installs, only after the new one is
/// durably active. A locked or busy old install (e.g. still executing a
/// probe) is left for a later install to prune.
fn prune_older_installs(vendor_root: &Path, keep_version: &str) {
    let installs = vendor_root.join(INSTALLS_DIR_NAME);
    let entries = match std::fs::read_dir(&installs) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!("failed to list vendor installs for pruning: {error}");
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.file_name().to_str() == Some(keep_version) {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(entry.path()) {
            tracing::warn!("failed to prune a superseded vendor install: {error}");
        }
    }
}

fn failed(detail: String) -> InstallError {
    InstallError::Failed(detail)
}
