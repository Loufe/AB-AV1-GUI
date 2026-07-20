//! Media tool discovery. Interim implementation until the vendor subsystem
//! (issue #43) lands: explicit `CRFTY_FFMPEG`/`CRFTY_FFPROBE` paths win, then
//! a PATH search. Discovery is infallible by design — a missing tool is a
//! reportable fact, not a startup error, so the durable engine always starts.

use std::path::PathBuf;

use crfty_core::{MediaTool, ToolAvailability, ToolRevisions, ToolSource};

use crate::ab_av1::MediaTools;

#[derive(Debug, Clone)]
pub enum ToolDiscovery {
    Available {
        tools: MediaTools,
        source: ToolSource,
    },
    Missing {
        missing: Vec<MediaTool>,
        detail: String,
    },
}

impl ToolDiscovery {
    pub fn tools(&self) -> Option<&MediaTools> {
        match self {
            Self::Available { tools, .. } => Some(tools),
            Self::Missing { .. } => None,
        }
    }

    pub fn availability(&self, revisions: ToolRevisions) -> ToolAvailability {
        match self {
            Self::Available { source, .. } => ToolAvailability::Available {
                source: *source,
                revisions,
            },
            Self::Missing { missing, detail } => ToolAvailability::Missing {
                missing: missing.clone(),
                detail: detail.clone(),
            },
        }
    }
}

pub fn discover_media_tools() -> ToolDiscovery {
    let ffmpeg = discover_tool("CRFTY_FFMPEG", "ffmpeg");
    let ffprobe = discover_tool("CRFTY_FFPROBE", "ffprobe");
    match (ffmpeg, ffprobe) {
        (Ok(ffmpeg), Ok(ffprobe)) => ToolDiscovery::Available {
            source: if ffmpeg.1 == ToolSource::Explicit || ffprobe.1 == ToolSource::Explicit {
                ToolSource::Explicit
            } else {
                ToolSource::System
            },
            tools: MediaTools {
                ffmpeg: ffmpeg.0,
                ffprobe: ffprobe.0,
            },
        },
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
            ToolDiscovery::Missing {
                missing,
                detail: details.join("; "),
            }
        }
    }
}

fn discover_tool(env_var: &str, binary: &str) -> Result<(PathBuf, ToolSource), String> {
    if let Some(configured) = std::env::var_os(env_var) {
        let path = PathBuf::from(configured);
        return if path.is_file() {
            Ok((path, ToolSource::Explicit))
        } else {
            Err(format!(
                "{env_var} is set but does not point at a file: {}",
                path.display()
            ))
        };
    }
    let file_name = if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_owned()
    };
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|directory| directory.join(&file_name))
        .find(|candidate| candidate.is_file())
        .map(|path| (path, ToolSource::System))
        .ok_or_else(|| format!("{binary} was not found via {env_var} or PATH"))
}
