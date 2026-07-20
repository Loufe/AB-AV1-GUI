//! Media tool discovery. Precedence per tool: explicit `CRFTY_FFMPEG`/
//! `CRFTY_FFPROBE` paths, then the managed vendor install, then a PATH
//! search. An explicit path that does not point at a file is fail-closed:
//! discovery reports the tool missing rather than silently substituting a
//! different binary for the one the user pinned. Discovery is infallible by
//! design — a missing tool is a reportable fact, not a startup error, so the
//! durable engine always starts.
//!
//! Revision provenance: managed installs carry their revisions in metadata
//! written at install time (no process spawn); system and explicit tools are
//! probed via ffprobe's JSON version document, with the probed FFmpeg version
//! standing in for the encoder revision — conservatively, any FFmpeg change
//! invalidates cached analyses (see ADR-008).

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use crfty_core::{MediaTool, ToolAvailability, ToolRevisions, ToolSource};
use serde::{Deserialize, Serialize};

use super::{manifest, probe};
use crate::ab_av1::{AB_AV1_REVISION, MediaTools};

const FFMPEG_VARIABLE: &str = "CRFTY_FFMPEG";
const FFPROBE_VARIABLE: &str = "CRFTY_FFPROBE";

/// Layout of the vendor root. `current.json` atomically names the active
/// install; entries under `staging/` belong to in-flight downloads and are
/// stale debris from a crashed process whenever discovery runs.
pub(crate) const CURRENT_FILE_NAME: &str = "current.json";
pub(crate) const STAGING_DIR_NAME: &str = "staging";

/// The managed install record stored in `current.json`. Paths are relative
/// to the vendor root and must stay inside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InstalledMetadata {
    pub version: String,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub ffmpeg_revision: String,
    pub encoder_revision: String,
}

/// The tool set a session executes with, snapshotted once at session start.
#[derive(Debug, Clone)]
pub struct CurrentTools {
    pub media: MediaTools,
    pub source: ToolSource,
    pub revisions: ToolRevisions,
}

#[derive(Debug, Clone)]
pub enum DiscoveredTools {
    Available(CurrentTools),
    Missing {
        missing: Vec<MediaTool>,
        detail: String,
    },
}

impl DiscoveredTools {
    #[must_use]
    pub fn current(&self) -> Option<&CurrentTools> {
        match self {
            Self::Available(current) => Some(current),
            Self::Missing { .. } => None,
        }
    }

    #[must_use]
    pub fn availability(&self) -> ToolAvailability {
        match self {
            Self::Available(current) => ToolAvailability::Available {
                source: current.source,
                revisions: current.revisions.clone(),
            },
            Self::Missing { missing, detail } => ToolAvailability::Missing {
                missing: missing.clone(),
                detail: detail.clone(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryReport {
    pub tools: DiscoveredTools,
    /// Strictly: a managed install exists and its version differs from the
    /// compiled-in manifest. Computed by local comparison only — discovery
    /// never touches the network.
    pub update_available: bool,
}

/// Inputs discovery reads from the process environment, injectable so tests
/// exercise the precedence matrix without process-global mutation.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryEnvironment {
    pub ffmpeg_override: Option<std::ffi::OsString>,
    pub ffprobe_override: Option<std::ffi::OsString>,
    pub search_path: Option<std::ffi::OsString>,
}

impl DiscoveryEnvironment {
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            ffmpeg_override: std::env::var_os(FFMPEG_VARIABLE),
            ffprobe_override: std::env::var_os(FFPROBE_VARIABLE),
            search_path: std::env::var_os("PATH"),
        }
    }
}

pub fn discover(vendor_root: &Path) -> DiscoveryReport {
    discover_with(vendor_root, &DiscoveryEnvironment::from_process())
}

pub fn discover_with(vendor_root: &Path, environment: &DiscoveryEnvironment) -> DiscoveryReport {
    clean_stale_staging(vendor_root);
    let managed = match load_managed(vendor_root) {
        Ok(managed) => managed,
        Err(detail) => {
            eprintln!("managed vendor install is unusable, falling back: {detail}");
            None
        }
    };
    let update_available = managed.as_ref().is_some_and(|install| {
        manifest::current().is_some_and(|manifest| install.metadata.version != manifest.build)
    });
    let ffmpeg = resolve_tool(
        "ffmpeg",
        FFMPEG_VARIABLE,
        environment.ffmpeg_override.as_deref(),
        managed.as_ref().map(|install| install.ffmpeg.as_path()),
        environment.search_path.as_deref(),
    );
    let ffprobe = resolve_tool(
        "ffprobe",
        FFPROBE_VARIABLE,
        environment.ffprobe_override.as_deref(),
        managed.as_ref().map(|install| install.ffprobe.as_path()),
        environment.search_path.as_deref(),
    );
    let ((ffmpeg, ffmpeg_source), (ffprobe, ffprobe_source)) = match (ffmpeg, ffprobe) {
        (Ok(ffmpeg), Ok(ffprobe)) => (ffmpeg, ffprobe),
        (ffmpeg, ffprobe) => {
            let mut missing = Vec::new();
            let mut details = Vec::new();
            if let Err(detail) = ffmpeg {
                missing.push(MediaTool::Ffmpeg);
                details.push(detail);
            }
            if let Err(detail) = ffprobe {
                missing.push(MediaTool::Ffprobe);
                details.push(detail);
            }
            return DiscoveryReport {
                tools: DiscoveredTools::Missing {
                    missing,
                    detail: details.join("; "),
                },
                update_available,
            };
        }
    };
    let source = if ffmpeg_source == ToolSource::Explicit || ffprobe_source == ToolSource::Explicit
    {
        ToolSource::Explicit
    } else if ffmpeg_source == ToolSource::Managed && ffprobe_source == ToolSource::Managed {
        ToolSource::Managed
    } else {
        ToolSource::System
    };
    let revisions = if source == ToolSource::Managed {
        // Unreachable None: both tools resolved Managed, so `managed` exists.
        match managed {
            Some(install) => ToolRevisions {
                ab_av1: AB_AV1_REVISION.to_owned(),
                ffmpeg: install.metadata.ffmpeg_revision,
                encoder: install.metadata.encoder_revision,
            },
            None => {
                return DiscoveryReport {
                    tools: DiscoveredTools::Missing {
                        missing: vec![MediaTool::Ffmpeg, MediaTool::Ffprobe],
                        detail: "managed tools resolved without install metadata".to_owned(),
                    },
                    update_available,
                };
            }
        }
    } else {
        match probe::ffprobe_version(&ffprobe) {
            Ok(version) => ToolRevisions {
                ab_av1: AB_AV1_REVISION.to_owned(),
                ffmpeg: version.clone(),
                encoder: version,
            },
            Err(detail) => {
                // Fail-closed: tools whose provenance cannot be established
                // are not usable tools.
                return DiscoveryReport {
                    tools: DiscoveredTools::Missing {
                        missing: vec![MediaTool::Ffprobe],
                        detail,
                    },
                    update_available,
                };
            }
        }
    };
    DiscoveryReport {
        tools: DiscoveredTools::Available(CurrentTools {
            media: MediaTools { ffmpeg, ffprobe },
            source,
            revisions,
        }),
        update_available,
    }
}

struct ManagedInstall {
    metadata: InstalledMetadata,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

/// Reads the managed install record. `Ok(None)` means no install exists; an
/// unreadable or inconsistent record is an error the caller logs before
/// falling back to the next tier — only explicit paths are fail-closed.
fn load_managed(vendor_root: &Path) -> Result<Option<ManagedInstall>, String> {
    let current = vendor_root.join(CURRENT_FILE_NAME);
    let contents = match std::fs::read(&current) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to read the install record: {error}")),
    };
    let metadata: InstalledMetadata = serde_json::from_slice(&contents)
        .map_err(|error| format!("the install record is not valid JSON: {error}"))?;
    let ffmpeg = contained_install_path(vendor_root, &metadata.ffmpeg)?;
    let ffprobe = contained_install_path(vendor_root, &metadata.ffprobe)?;
    if metadata.version.is_empty()
        || metadata.ffmpeg_revision.is_empty()
        || metadata.encoder_revision.is_empty()
    {
        return Err("the install record has empty version fields".to_owned());
    }
    for binary in [&ffmpeg, &ffprobe] {
        if !binary.is_file() {
            return Err("the install record names a binary that does not exist".to_owned());
        }
    }
    Ok(Some(ManagedInstall {
        metadata,
        ffmpeg,
        ffprobe,
    }))
}

/// Joins a metadata-relative path to the vendor root, rejecting records that
/// escape it: `current.json` is data, not an instruction to run arbitrary
/// binaries elsewhere on disk.
fn contained_install_path(vendor_root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
        || relative.as_os_str().is_empty()
    {
        return Err("the install record path escapes the vendor directory".to_owned());
    }
    Ok(vendor_root.join(relative))
}

fn resolve_tool(
    binary: &str,
    variable: &str,
    explicit: Option<&OsStr>,
    managed: Option<&Path>,
    search_path: Option<&OsStr>,
) -> Result<(PathBuf, ToolSource), String> {
    if let Some(configured) = explicit {
        let path = PathBuf::from(configured);
        return if path.is_file() {
            Ok((path, ToolSource::Explicit))
        } else {
            Err(format!(
                "{variable} is set but does not point at a file: {}",
                path.display()
            ))
        };
    }
    if let Some(managed) = managed {
        return Ok((managed.to_path_buf(), ToolSource::Managed));
    }
    let file_name = if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_owned()
    };
    search_path
        .map(|paths| std::env::split_paths(paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|directory| directory.join(&file_name))
        .find(|candidate| candidate.is_file())
        .map(|path| (path, ToolSource::System))
        .ok_or_else(|| format!("{binary} was not found via {variable}, a managed install, or PATH"))
}

/// Removes leftover download staging from crashed runs. Best-effort: another
/// live process may hold entries open (no single-instance lock yet — #33);
/// its install then fails typed and restartable rather than corrupting ours.
fn clean_stale_staging(vendor_root: &Path) {
    let staging = vendor_root.join(STAGING_DIR_NAME);
    match std::fs::remove_dir_all(&staging) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("failed to clean stale vendor staging: {error}"),
    }
}
