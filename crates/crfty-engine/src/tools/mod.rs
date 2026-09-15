//! The media tool boundary: locating the operator-supplied FFmpeg and
//! ffprobe binaries and verifying what they can do (ADR-023). Discovery
//! reports filesystem facts and the probe reports process facts; the reducer
//! owns all gating on both.

pub mod discovery;
pub(crate) mod probe;

use std::path::PathBuf;

use crfty_core::LocatedTools;

/// Absolute paths to the media binaries a worker executes. Adapters (ab-av1,
/// remux, media inspection) consume these; discovery owns them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl From<&LocatedTools> for MediaTools {
    fn from(located: &LocatedTools) -> Self {
        Self {
            ffmpeg: located.ffmpeg.path.clone(),
            ffprobe: located.ffprobe.path.clone(),
        }
    }
}
