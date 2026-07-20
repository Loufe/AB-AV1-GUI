//! Hardened archive extraction. Only the two manifest binaries are ever
//! written to disk, into a `bin/` directory the code names itself, with
//! permissions the code chooses itself — nothing in the archive decides a
//! path, a mode, or a link target. Every entry is still validated, so a
//! malicious layout anywhere in the archive rejects the whole archive.

use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use super::manifest::ArchiveKind;

const COPY_CHUNK_BYTES: usize = 64 * 1024;
/// Unix file-type mask and the symlink type bits, for zip entries that
/// carry unix modes.
const UNIX_TYPE_MASK: u32 = 0o170_000;
const UNIX_TYPE_SYMLINK: u32 = 0o120_000;

/// What to pull out of an archive and the guard rails to apply. Owned
/// strings so tests can craft arbitrary layouts; production copies the
/// compiled-in manifest.
#[derive(Debug, Clone)]
pub struct ExtractSpec {
    /// `/`-separated archive entry paths of the two binaries.
    pub ffmpeg_entry: String,
    pub ffprobe_entry: String,
    /// Zip-bomb guard: extraction aborts once this many decompressed bytes
    /// have been written.
    pub max_extracted_bytes: u64,
    pub kind: ArchiveKind,
}

#[derive(Debug)]
pub struct ExtractedBinaries {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Extracts the two spec binaries from `archive` into `destination/bin/`,
/// validating every entry in the archive against the layout rules.
pub fn extract_binaries(
    archive: &Path,
    spec: &ExtractSpec,
    destination: &Path,
) -> Result<ExtractedBinaries, String> {
    let prefix = top_level_prefix(spec)?;
    let bin_directory = destination.join("bin");
    std::fs::create_dir_all(&bin_directory)
        .map_err(|error| format!("failed to create the staged bin directory: {error}"))?;
    let file = std::fs::File::open(archive)
        .map_err(|error| format!("failed to open the downloaded archive: {error}"))?;
    let mut targets = ExtractTargets::new(spec, &bin_directory)?;
    match spec.kind {
        ArchiveKind::TarXz => extract_tar_xz(file, spec, &prefix, &mut targets)?,
        ArchiveKind::Zip => extract_zip(file, spec, &prefix, &mut targets)?,
    }
    targets.into_binaries()
}

/// Tracks the two wanted entries: where each lands and whether it was seen.
struct ExtractTargets {
    ffmpeg_entry: String,
    ffprobe_entry: String,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    ffmpeg_written: bool,
    ffprobe_written: bool,
}

impl ExtractTargets {
    fn new(spec: &ExtractSpec, bin_directory: &Path) -> Result<Self, String> {
        Ok(Self {
            ffmpeg_entry: spec.ffmpeg_entry.clone(),
            ffprobe_entry: spec.ffprobe_entry.clone(),
            ffmpeg: bin_directory.join(entry_file_name(&spec.ffmpeg_entry)?),
            ffprobe: bin_directory.join(entry_file_name(&spec.ffprobe_entry)?),
            ffmpeg_written: false,
            ffprobe_written: false,
        })
    }

    /// The on-disk path for a wanted entry name, or `None` for entries the
    /// extraction only validates.
    fn destination_for(&mut self, name: &str) -> Option<PathBuf> {
        if name == self.ffmpeg_entry {
            self.ffmpeg_written = true;
            Some(self.ffmpeg.clone())
        } else if name == self.ffprobe_entry {
            self.ffprobe_written = true;
            Some(self.ffprobe.clone())
        } else {
            None
        }
    }

    fn into_binaries(self) -> Result<ExtractedBinaries, String> {
        if !self.ffmpeg_written || !self.ffprobe_written {
            return Err("the archive does not contain the expected binaries".to_owned());
        }
        Ok(ExtractedBinaries {
            ffmpeg: self.ffmpeg,
            ffprobe: self.ffprobe,
        })
    }
}

fn extract_tar_xz(
    file: std::fs::File,
    spec: &ExtractSpec,
    prefix: &str,
    targets: &mut ExtractTargets,
) -> Result<(), String> {
    // lzma-rs decompresses whole streams, not readers, so the tar layer
    // works from a size-capped temporary file. The cap bounds the tar
    // stream itself, which in turn bounds every entry inside it.
    let mut compressed = std::io::BufReader::new(file);
    let decompressed = tempfile::tempfile()
        .map_err(|error| format!("failed to create the decompression file: {error}"))?;
    let mut capped = CappedWriter {
        inner: decompressed,
        remaining: spec.max_extracted_bytes,
    };
    lzma_rs::xz_decompress(&mut compressed, &mut capped)
        .map_err(|error| format!("failed to decompress the archive: {error:?}"))?;
    let mut tar_file = capped.inner;
    tar_file
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("failed to rewind the decompressed archive: {error}"))?;
    let mut archive = tar::Archive::new(tar_file);
    let mut seen = HashSet::new();
    let entries = archive
        .entries()
        .map_err(|error| format!("failed to read the archive: {error}"))?;
    let mut budget = spec.max_extracted_bytes;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| format!("failed to read an archive entry: {error}"))?;
        let name = String::from_utf8(entry.path_bytes().to_vec())
            .map_err(|_error| "an archive entry name is not valid UTF-8".to_owned())?;
        let kind = entry.header().entry_type();
        let is_directory = matches!(kind, tar::EntryType::Directory);
        if !is_directory && !matches!(kind, tar::EntryType::Regular) {
            return Err(format!(
                "the archive contains a forbidden entry type at {name}"
            ));
        }
        validate_entry_name(&name, is_directory, prefix, &mut seen)?;
        if is_directory {
            continue;
        }
        if let Some(destination) = targets.destination_for(name.trim_end_matches('/')) {
            write_binary(&mut entry, &destination, &mut budget)?;
        }
    }
    Ok(())
}

fn extract_zip(
    file: std::fs::File,
    spec: &ExtractSpec,
    prefix: &str,
    targets: &mut ExtractTargets,
) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("failed to read the archive: {error}"))?;
    let mut seen = HashSet::new();
    let mut budget = spec.max_extracted_bytes;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("failed to read an archive entry: {error}"))?;
        let name = String::from_utf8(entry.name_raw().to_vec())
            .map_err(|_error| "an archive entry name is not valid UTF-8".to_owned())?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & UNIX_TYPE_MASK == UNIX_TYPE_SYMLINK)
        {
            return Err(format!("the archive contains a symlink entry at {name}"));
        }
        let is_directory = entry.is_dir();
        validate_entry_name(&name, is_directory, prefix, &mut seen)?;
        if is_directory {
            continue;
        }
        if let Some(destination) = targets.destination_for(name.trim_end_matches('/')) {
            // Only wanted entries are ever decompressed; skipped entries
            // cost their central-directory record and nothing more.
            write_binary(&mut entry, &destination, &mut budget)?;
        }
    }
    Ok(())
}

/// Rejects every archive-controlled path shape: separators smuggled as
/// backslashes, absolute paths, `.`/`..` components, entries outside the
/// manifest's single top-level directory, and names that collide on a
/// case-insensitive filesystem.
fn validate_entry_name(
    name: &str,
    is_directory: bool,
    prefix: &str,
    seen: &mut HashSet<String>,
) -> Result<(), String> {
    if name.contains('\\') {
        return Err(format!(
            "an archive entry name contains a backslash: {name}"
        ));
    }
    let trimmed = if is_directory {
        name.trim_end_matches('/')
    } else {
        name
    };
    if trimmed.is_empty() || name.starts_with('/') {
        return Err(format!("an archive entry has an unusable path: {name}"));
    }
    let mut components = trimmed.split('/');
    if components.next() != Some(prefix) {
        return Err(format!(
            "an archive entry escapes the expected top-level directory: {name}"
        ));
    }
    for component in trimmed.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!("an archive entry traverses directories: {name}"));
        }
    }
    if !seen.insert(trimmed.to_lowercase()) {
        return Err(format!(
            "the archive contains case-colliding duplicate entries: {name}"
        ));
    }
    Ok(())
}

fn top_level_prefix(spec: &ExtractSpec) -> Result<String, String> {
    let prefix = spec
        .ffmpeg_entry
        .split('/')
        .next()
        .filter(|component| !component.is_empty())
        .ok_or_else(|| "the extract spec has no top-level directory".to_owned())?;
    if spec.ffprobe_entry.split('/').next() != Some(prefix) {
        return Err("the extract spec binaries disagree on the top-level directory".to_owned());
    }
    Ok(prefix.to_owned())
}

fn entry_file_name(entry: &str) -> Result<String, String> {
    entry
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .map(str::to_owned)
        .ok_or_else(|| format!("the extract spec entry has no file name: {entry}"))
}

/// Writes one binary with a mode this code chooses, syncing it durably —
/// the install promote renames the directory these land in, and a rename
/// must never publish unsynced bytes.
fn write_binary(reader: &mut dyn Read, destination: &Path, budget: &mut u64) -> Result<(), String> {
    let mut file = std::fs::File::create(destination)
        .map_err(|error| format!("failed to create a staged binary: {error}"))?;
    copy_capped(reader, &mut file, budget)?;
    file.sync_all()
        .map_err(|error| format!("failed to sync a staged binary: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("failed to set staged binary permissions: {error}"))?;
    }
    Ok(())
}

fn copy_capped(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    budget: &mut u64,
) -> Result<(), String> {
    let mut buffer = vec![0_u8; COPY_CHUNK_BYTES];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("failed to read an archive entry: {error}"))?;
        if count == 0 {
            return Ok(());
        }
        let Some(remaining) = budget.checked_sub(count as u64) else {
            return Err("the archive expands past the extraction size limit".to_owned());
        };
        *budget = remaining;
        let Some(chunk) = buffer.get(..count) else {
            return Err("the archive entry returned an impossible chunk size".to_owned());
        };
        writer
            .write_all(chunk)
            .map_err(|error| format!("failed to write a staged binary: {error}"))?;
    }
}

/// A writer that fails once more than its budget has been written; backs
/// the xz decompression stage of the zip-bomb guard.
struct CappedWriter<W: Write> {
    inner: W,
    remaining: u64,
}

impl<W: Write> Write for CappedWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let Some(remaining) = self.remaining.checked_sub(buffer.len() as u64) else {
            return Err(std::io::Error::other(
                "the archive expands past the extraction size limit",
            ));
        };
        self.remaining = remaining;
        self.inner.write(buffer).inspect(|&written| {
            // Partial writes return unused budget so accounting stays exact.
            let unused = buffer.len().saturating_sub(written) as u64;
            self.remaining = self.remaining.saturating_add(unused);
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{ExtractSpec, entry_file_name, top_level_prefix, validate_entry_name};
    use crate::vendor::manifest::ArchiveKind;

    fn spec(ffmpeg: &str, ffprobe: &str) -> ExtractSpec {
        ExtractSpec {
            ffmpeg_entry: ffmpeg.to_owned(),
            ffprobe_entry: ffprobe.to_owned(),
            max_extracted_bytes: 1024,
            kind: ArchiveKind::TarXz,
        }
    }

    #[test]
    fn accepts_the_expected_layout() {
        let mut seen = HashSet::new();
        assert!(validate_entry_name("build/", true, "build", &mut seen).is_ok());
        assert!(validate_entry_name("build/bin/ffmpeg", false, "build", &mut seen).is_ok());
        assert!(validate_entry_name("build/doc/LICENSE", false, "build", &mut seen).is_ok());
    }

    #[test]
    fn rejects_malicious_entry_names() {
        for name in [
            "../evil",
            "build/../../evil",
            "/etc/passwd",
            "build//gap",
            "build/./sneaky",
            "other/bin/ffmpeg",
            "build\\bin\\ffmpeg",
            "",
        ] {
            let mut seen = HashSet::new();
            assert!(
                validate_entry_name(name, false, "build", &mut seen).is_err(),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_case_colliding_duplicates() {
        let mut seen = HashSet::new();
        assert!(validate_entry_name("build/bin/ffmpeg", false, "build", &mut seen).is_ok());
        assert!(validate_entry_name("build/bin/FFmpeg", false, "build", &mut seen).is_err());
    }

    #[test]
    fn spec_prefix_and_file_names_are_derived_safely() {
        let expected = spec("build/bin/ffmpeg", "build/bin/ffprobe");
        assert_eq!(top_level_prefix(&expected), Ok("build".to_owned()));
        assert_eq!(entry_file_name("build/bin/ffmpeg"), Ok("ffmpeg".to_owned()));
        assert!(top_level_prefix(&spec("build/bin/ffmpeg", "other/bin/ffprobe")).is_err());
        assert!(entry_file_name("build/bin/").is_err());
    }
}
