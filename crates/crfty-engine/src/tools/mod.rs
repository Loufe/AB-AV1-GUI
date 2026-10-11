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
    pub ab_av1: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl From<&LocatedTools> for MediaTools {
    fn from(located: &LocatedTools) -> Self {
        Self {
            ab_av1: located.ab_av1.path.clone(),
            ffmpeg: located.ffmpeg.path.clone(),
            ffprobe: located.ffprobe.path.clone(),
        }
    }
}

/// Identity of the inspected executable bytes, independent of its installation path.
pub fn executable_revision(path: &std::path::Path) -> std::io::Result<String> {
    use blake2::{Blake2b512, Digest};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut digest = Blake2b512::new();
    let mut bytes = [0_u8; 8192];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        if let Some(bytes) = bytes.get(..count) {
            digest.update(bytes);
        }
    }
    Ok(format!("blake2b:{:x}", digest.finalize()))
}
