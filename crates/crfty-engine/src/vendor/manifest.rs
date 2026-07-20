//! The compiled-in FFmpeg vendor manifest: the trust anchor for managed
//! installs. Downloads are authenticated by the pinned SHA-256 alone — the
//! manifest ships inside the binary, so tampering with it requires tampering
//! with the application itself. The pinned build is the same BtbN autobuild
//! the `media-contract.yml` workflow verifies native acceptance against.

use crfty_core::ToolRevisions;

use crate::ab_av1::AB_AV1_REVISION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarXz,
    Zip,
}

#[derive(Debug, Clone, Copy)]
pub struct VendorManifest {
    /// BtbN release tag the archive is published under.
    pub tag: &'static str,
    /// FFmpeg build identifier; doubles as the managed install's version
    /// directory name and its ffmpeg/encoder revision string.
    pub build: &'static str,
    pub url: &'static str,
    /// Lowercase hex SHA-256 of the archive.
    pub sha256_hex: &'static str,
    pub archive: ArchiveKind,
    /// Archive-relative entry paths of the two binaries, `/`-separated.
    pub ffmpeg_entry: &'static str,
    pub ffprobe_entry: &'static str,
    /// Zip-bomb guard: extraction aborts past this many written bytes.
    pub max_extracted_bytes: u64,
}

impl VendorManifest {
    /// Provenance recorded for analyses executed with this managed install.
    /// The checksummed archive pins the bundled SVT-AV1 build, so the FFmpeg
    /// build identifier is the encoder revision too.
    #[must_use]
    pub fn revisions(&self) -> ToolRevisions {
        ToolRevisions {
            ab_av1: AB_AV1_REVISION.to_owned(),
            ffmpeg: self.build.to_owned(),
            encoder: self.build.to_owned(),
        }
    }
}

const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[cfg(target_os = "linux")]
const CURRENT: VendorManifest = VendorManifest {
    tag: "autobuild-2026-07-19-13-12",
    build: "ffmpeg-n8.1.2-22-g94138f6973",
    url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-07-19-13-12/ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-8.1.tar.xz",
    sha256_hex: "166375e7f8b1f6963949a61a83ffffe858eba742f6326180b8ff3bc58b205c72",
    archive: ArchiveKind::TarXz,
    ffmpeg_entry: "ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-8.1/bin/ffmpeg",
    ffprobe_entry: "ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-8.1/bin/ffprobe",
    max_extracted_bytes: MAX_EXTRACTED_BYTES,
};

#[cfg(windows)]
const CURRENT: VendorManifest = VendorManifest {
    tag: "autobuild-2026-07-19-13-12",
    build: "ffmpeg-n8.1.2-22-g94138f6973",
    url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-07-19-13-12/ffmpeg-n8.1.2-22-g94138f6973-win64-gpl-8.1.zip",
    sha256_hex: "9db2860af5d1c536ed7fcb7ed84fa4ef80d188d1396d1cdf8cad180137510f3f",
    archive: ArchiveKind::Zip,
    ffmpeg_entry: "ffmpeg-n8.1.2-22-g94138f6973-win64-gpl-8.1/bin/ffmpeg.exe",
    ffprobe_entry: "ffmpeg-n8.1.2-22-g94138f6973-win64-gpl-8.1/bin/ffprobe.exe",
    max_extracted_bytes: MAX_EXTRACTED_BYTES,
};

/// The manifest for this build's target platform. `None` on platforms BtbN
/// publishes no build for: discovery then skips the managed tier and update
/// checks, and a vendor install reports a typed failure.
#[must_use]
pub const fn current() -> Option<&'static VendorManifest> {
    #[cfg(any(target_os = "linux", windows))]
    {
        Some(&CURRENT)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::current;

    #[test]
    fn manifest_is_internally_consistent() {
        let Some(manifest) = current() else {
            return;
        };
        assert_eq!(manifest.sha256_hex.len(), 64);
        assert!(
            manifest
                .sha256_hex
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        );
        assert!(manifest.url.starts_with(&format!(
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/{}/{}-",
            manifest.tag, manifest.build
        )));
        for entry in [manifest.ffmpeg_entry, manifest.ffprobe_entry] {
            assert!(
                entry.starts_with(&format!("{}-", manifest.build)),
                "{entry} is outside the build's top-level directory"
            );
            assert!(!entry.contains(".."));
        }
        assert!(manifest.max_extracted_bytes > 0);
        let revisions = manifest.revisions();
        assert_eq!(revisions.ffmpeg, manifest.build);
        assert_eq!(revisions.encoder, manifest.build);
        assert_eq!(revisions.ab_av1, crate::ab_av1::AB_AV1_REVISION);
    }
}
