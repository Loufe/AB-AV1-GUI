//! Media tool discovery. Precedence per tool: the `CRFTY_FFMPEG` /
//! `CRFTY_FFPROBE` environment override, then the Settings path, then a
//! search of `PATH`. An explicit tier that does not point at a file is
//! fail-closed: discovery reports the tool missing rather than substituting
//! a different binary for the one the operator chose. Discovery is
//! infallible by design (a missing tool is a reportable fact, not a startup
//! error) and spawns nothing: whether the located binaries actually work is
//! the capability probe's question, asked at session start (ADR-023).

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use crfty_core::{
    LocatedTool, LocatedTools, MediaTool, ToolAvailability, ToolLocationFailure, ToolPathSettings,
    ToolSource, ToolVerification,
};

pub const AB_AV1_VARIABLE: &str = "CRFTY_AB_AV1";

pub const FFMPEG_VARIABLE: &str = "CRFTY_FFMPEG";
pub const FFPROBE_VARIABLE: &str = "CRFTY_FFPROBE";

/// Inputs discovery reads from the process environment, injectable so tests
/// exercise the precedence matrix without process-global mutation.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryEnvironment {
    pub ab_av1_override: Option<OsString>,
    pub ffmpeg_override: Option<OsString>,
    pub ffprobe_override: Option<OsString>,
    pub search_path: Option<OsString>,
}

impl DiscoveryEnvironment {
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            ab_av1_override: std::env::var_os(AB_AV1_VARIABLE),
            ffmpeg_override: std::env::var_os(FFMPEG_VARIABLE),
            ffprobe_override: std::env::var_os(FFPROBE_VARIABLE),
            search_path: std::env::var_os("PATH"),
        }
    }
}

#[must_use]
pub fn discover(
    environment: &DiscoveryEnvironment,
    configured: &ToolPathSettings,
) -> ToolAvailability {
    let ab_av1 = locate(
        MediaTool::AbAv1,
        environment.ab_av1_override.as_deref(),
        configured.ab_av1.as_deref(),
        environment.search_path.as_deref(),
    );
    let ffmpeg = locate(
        MediaTool::Ffmpeg,
        environment.ffmpeg_override.as_deref(),
        configured.ffmpeg.as_deref(),
        environment.search_path.as_deref(),
    );
    let ffprobe = locate(
        MediaTool::Ffprobe,
        environment.ffprobe_override.as_deref(),
        configured.ffprobe.as_deref(),
        environment.search_path.as_deref(),
    );
    match (ab_av1, ffmpeg, ffprobe) {
        (Ok(ab_av1), Ok(ffmpeg), Ok(ffprobe)) => ToolAvailability::Located {
            tools: LocatedTools {
                ab_av1,
                ffmpeg,
                ffprobe,
            },
            verification: ToolVerification::Pending,
        },
        (ab_av1, ffmpeg, ffprobe) => ToolAvailability::Missing {
            failures: [ab_av1.err(), ffmpeg.err(), ffprobe.err()]
                .into_iter()
                .flatten()
                .collect(),
        },
    }
}

fn locate(
    tool: MediaTool,
    environment_override: Option<&OsStr>,
    configured: Option<&Path>,
    search_path: Option<&OsStr>,
) -> Result<LocatedTool, ToolLocationFailure> {
    if let Some(path) = environment_override {
        let path = PathBuf::from(path);
        return if path.is_file() {
            Ok(LocatedTool {
                source: ToolSource::Environment,
                path,
            })
        } else {
            Err(ToolLocationFailure::EnvironmentPathIsNotAFile { tool, path })
        };
    }
    if let Some(path) = configured {
        return if path.is_file() {
            Ok(LocatedTool {
                source: ToolSource::Settings,
                path: path.to_path_buf(),
            })
        } else {
            Err(ToolLocationFailure::SettingsPathIsNotAFile {
                tool,
                path: path.to_path_buf(),
            })
        };
    }
    let file_name = if cfg!(windows) {
        format!("{}.exe", tool.name())
    } else {
        tool.name().to_owned()
    };
    search_path
        .map(|paths| std::env::split_paths(paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|directory| directory.join(&file_name))
        .find(|candidate| candidate.is_file())
        .map(|path| LocatedTool {
            source: ToolSource::SearchPath,
            path,
        })
        .ok_or(ToolLocationFailure::NotOnSearchPath { tool })
}
