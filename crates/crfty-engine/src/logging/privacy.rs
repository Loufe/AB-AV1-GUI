//! Pure log-line anonymization: BLAKE2b path hashing plus pattern-based
//! scrubbing of path-shaped text.
//!
//! Ports the V2 semantics (`main:src/privacy.py`) with three deliberate
//! changes: normalization is purely textual (no filesystem or drive-mapping
//! calls, so scrubbing a log line never touches the OS), already-anonymized
//! `file_`/`folder_` tokens are recognized and left alone (V2 re-hashed them
//! on every retroactive scrub), and the configured-folder placeholders are
//! actually wired to settings (dead code in V2). The hash itself is
//! byte-identical to V2 — BLAKE2b with a 16-byte digest, hex-encoded and
//! truncated to 12 characters — so V2 and V3 logs remain cross-referenceable.

use std::{fmt::Write as _, path::Path, sync::LazyLock};

use blake2::{
    Blake2bVar,
    digest::{Update, VariableOutput},
};
use regex::Regex;

/// Digest width matching V2's `hashlib.blake2b(..., digest_size=16)`.
const HASH_DIGEST_BYTES: usize = 16;
/// Hex characters kept from the digest, matching V2's 12-character truncation.
const HASH_HEX_CHARS: usize = 12;
/// Longest extension (including the dot) still treated as a file extension by
/// the match classifier, matching V2's `_MAX_EXTENSION_LENGTH`.
const MAX_EXTENSION_LENGTH: usize = 5;

/// Detection patterns, applied in order over each line. The V2 originals used
/// lookbehind for the delimited-filename pattern; the `regex` crate has no
/// lookbehind, so that pattern captures the delimiter as group 1 and restores
/// it in the replacement.
struct PatternSource {
    pattern: &'static str,
    /// Group 1 is a delimiter to preserve; the path is group 2.
    delimited: bool,
}

const PATTERN_SOURCES: &[PatternSource] = &[
    // Windows drive paths: C:\... or C:/...
    PatternSource {
        pattern: r#"[A-Za-z]:[\\/][^\s"'<>|*?\n]+"#,
        delimited: false,
    },
    // UNC paths: \\server\share\...
    PatternSource {
        pattern: r#"\\\\[^\s"'<>|*?\n]+"#,
        delimited: false,
    },
    // Unix absolute paths with common roots.
    PatternSource {
        pattern: r#"/(?:home|Users|mnt|media|var|tmp|opt|usr|root|srv|run|data)[^\s"'<>|*?\n]*"#,
        delimited: false,
    },
    // Video filenames that may contain spaces, after a delimiter.
    PatternSource {
        pattern: r#"(: |-> |= |"|')([^<>|*?\n"']+\.(?i:mp4|mkv|avi|wmv|mov|webm))"#,
        delimited: true,
    },
    // Fallback: video filenames without spaces.
    PatternSource {
        pattern: r#"(?i)[^\s"'<>|*?\n/\\]+\.(?:mp4|mkv|avi|wmv|mov|webm)"#,
        delimited: false,
    },
];

/// Compiled alongside a test asserting every source compiled; a pattern that
/// fails to compile is a programming error surfaced there, not a runtime
/// panic in the logging path.
static PATTERNS: LazyLock<Vec<(Regex, bool)>> = LazyLock::new(|| {
    PATTERN_SOURCES
        .iter()
        .filter_map(|source| {
            Regex::new(source.pattern)
                .ok()
                .map(|regex| (regex, source.delimited))
        })
        .collect()
});

/// Matches output this module already produced, so scrubbing is idempotent:
/// a second pass over an anonymized line changes nothing.
static ANONYMIZED_TOKEN: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"^(?:(?:\[input_folder\]|\[output_folder\]|\[unknown\]|folder_[0-9a-f]{12})/)?(?:file_[0-9a-f]{12}(?:\.[^./\\]*)?|folder_[0-9a-f]{12})$",
    )
    .ok()
});

/// BLAKE2b-16 hash of `value`, hex-truncated to 12 characters — byte-identical
/// to V2's `privacy.compute_hash`.
pub(crate) fn compute_hash(value: &str) -> String {
    let Ok(mut digest) = Blake2bVar::new(HASH_DIGEST_BYTES) else {
        // 16 bytes is always a valid BLAKE2b width; guarded by a unit test.
        return "invalid-hash".to_owned();
    };
    digest.update(value.as_bytes());
    let mut output = [0u8; HASH_DIGEST_BYTES];
    if digest.finalize_variable(&mut output).is_err() {
        return "invalid-hash".to_owned();
    }
    let mut hex = String::with_capacity(HASH_HEX_CHARS);
    for byte in output.iter().take(HASH_HEX_CHARS / 2) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Anonymizes path-shaped text in log lines. Holds the platform case-folding
/// choice and the configured-folder placeholders, both fixed per instance so
/// scrubbing stays a pure function of the line.
#[derive(Debug, Clone)]
pub(crate) struct PathScrubber {
    fold_case: bool,
    /// Normalized folder → placeholder label (e.g. `[input_folder]`).
    placeholders: Vec<(String, &'static str)>,
}

impl PathScrubber {
    pub(crate) fn new(fold_case: bool) -> Self {
        Self {
            fold_case,
            placeholders: Vec::new(),
        }
    }

    /// Installs the configured-folder placeholders from settings. Paths equal
    /// to a configured folder render as their label instead of a hash.
    pub(crate) fn set_configured_folders(
        &mut self,
        input_folder: Option<&Path>,
        output_folder: Option<&Path>,
    ) {
        self.placeholders.clear();
        if let Some(folder) = input_folder {
            self.placeholders
                .push((self.normalize(&folder.to_string_lossy()), "[input_folder]"));
        }
        if let Some(folder) = output_folder {
            self.placeholders
                .push((self.normalize(&folder.to_string_lossy()), "[output_folder]"));
        }
    }

    /// Applies every detection pattern over one line (no trailing newline)
    /// and replaces matches with hashed equivalents.
    pub(crate) fn scrub_line(&self, line: &str) -> String {
        let mut scrubbed = line.to_owned();
        for (pattern, delimited) in PATTERNS.iter() {
            scrubbed = pattern
                .replace_all(&scrubbed, |captures: &regex::Captures<'_>| {
                    if *delimited {
                        let delimiter = captures.get(1).map_or("", |m| m.as_str());
                        let path = captures.get(2).map_or("", |m| m.as_str());
                        format!("{delimiter}{}", self.anonymize_match(path))
                    } else {
                        let path = captures.get(0).map_or("", |m| m.as_str());
                        self.anonymize_match(path)
                    }
                })
                .into_owned();
        }
        scrubbed
    }

    /// Textual normalization for hashing: forward slashes, case-folded on
    /// Windows, trailing separator trimmed. V2 additionally resolved mapped
    /// drives and relative paths through the OS; log scrubbing works on text
    /// that may not exist on this machine, so V3 stays textual by design.
    fn normalize(&self, text: &str) -> String {
        let mut normalized = text.replace('\\', "/");
        if self.fold_case {
            normalized.make_ascii_lowercase();
        }
        while normalized.len() > 1 && normalized.ends_with('/') {
            normalized.pop();
        }
        normalized
    }

    /// Port of V2's `_anonymize_path_match`: extension → file or path,
    /// otherwise folder.
    fn anonymize_match(&self, matched: &str) -> String {
        if ANONYMIZED_TOKEN
            .as_ref()
            .is_some_and(|token| token.is_match(matched))
        {
            return matched.to_owned();
        }
        let has_directory = matched.contains('/') || matched.contains('\\');
        let extension = file_extension(basename(matched));
        match extension {
            Some(extension) if extension.len() <= MAX_EXTENSION_LENGTH => {
                if has_directory {
                    self.anonymize_path(matched)
                } else {
                    self.anonymize_file(matched)
                }
            }
            _ => self.anonymize_folder(matched),
        }
    }

    fn anonymize_folder(&self, folder: &str) -> String {
        if folder.is_empty() {
            return "[unknown]".to_owned();
        }
        let normalized = self.normalize(folder);
        for (configured, label) in &self.placeholders {
            if &normalized == configured {
                return (*label).to_owned();
            }
        }
        format!("folder_{}", compute_hash(&normalized))
    }

    fn anonymize_file(&self, filename: &str) -> String {
        if filename.is_empty() {
            return "file_unknown".to_owned();
        }
        let normalized = self.normalize(filename);
        let name = basename(&normalized);
        let (stem, extension) = match file_extension(name) {
            Some(extension) => (
                name.get(..name.len() - extension.len()).unwrap_or(name),
                extension,
            ),
            None => (name, ""),
        };
        format!("file_{}{extension}", compute_hash(stem))
    }

    fn anonymize_path(&self, path: &str) -> String {
        if path.is_empty() {
            return "[unknown]/file_unknown".to_owned();
        }
        let normalized = self.normalize(path);
        let (folder, file) = match normalized.rsplit_once('/') {
            Some((folder, file)) => (folder, file),
            None => ("", normalized.as_str()),
        };
        format!(
            "{}/{}",
            self.anonymize_folder(folder),
            self.anonymize_file(file)
        )
    }
}

/// Last path segment, accepting both separators (matched text may still
/// carry backslashes).
fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The extension including its dot, per Python `os.path.splitext`: the last
/// dot of the basename, unless it is the leading character.
fn file_extension(name: &str) -> Option<&str> {
    let index = name.rfind('.')?;
    if index == 0 {
        return None;
    }
    name.get(index..)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ANONYMIZED_TOKEN, PATTERN_SOURCES, PATTERNS, PathScrubber, compute_hash};

    /// Vectors generated from the V2 oracle:
    /// `hashlib.blake2b(value, digest_size=16).hexdigest()[:12]`.
    #[test]
    fn hash_matches_the_python_oracle() {
        assert_eq!(compute_hash("vacation video"), "355457aba2b4");
        assert_eq!(compute_hash("c:/users/someone/videos"), "cffde3ba23f7");
        assert_eq!(compute_hash("movie"), "2b3bd5fa4119");
        assert_eq!(compute_hash("my film 2024"), "61e6dc28cb20");
        assert_eq!(compute_hash(""), "cae66941d9ef");
    }

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(PATTERNS.len(), PATTERN_SOURCES.len());
        assert!(ANONYMIZED_TOKEN.is_some());
    }

    #[test]
    fn windows_paths_are_hashed_with_case_folding() {
        let scrubber = PathScrubber::new(true);
        assert_eq!(
            scrubber.scrub_line(r"Converting C:\Users\Someone\Videos\movie.mp4 now"),
            "Converting folder_cffde3ba23f7/file_2b3bd5fa4119.mp4 now"
        );
    }

    #[test]
    fn unc_paths_are_hashed() {
        let scrubber = PathScrubber::new(true);
        assert_eq!(
            scrubber.scrub_line(r"reading \\server\share\clip.wmv"),
            "reading folder_be4ae07bf1df/file_9e64679e2f6c.wmv"
        );
    }

    #[test]
    fn unix_paths_are_hashed_without_case_folding() {
        let scrubber = PathScrubber::new(false);
        assert_eq!(
            scrubber.scrub_line("probing /mnt/media/sample.mkv failed"),
            "probing folder_d5127c96b35c/file_8d5a8c8c9e18.mkv failed"
        );
    }

    #[test]
    fn pathless_folders_are_hashed_as_folders() {
        let scrubber = PathScrubber::new(false);
        // No extension: the whole match is treated as a folder (V2 dispatch).
        assert_eq!(
            scrubber.scrub_line("scanning /mnt/media"),
            "scanning folder_d5127c96b35c"
        );
    }

    #[test]
    fn delimited_filenames_with_spaces_are_hashed_and_keep_the_delimiter() {
        let scrubber = PathScrubber::new(true);
        assert_eq!(
            scrubber.scrub_line(r#"input: "my film 2024.mkv" queued"#),
            r#"input: "file_61e6dc28cb20.mkv" queued"#
        );
    }

    #[test]
    fn bare_video_filenames_are_hashed() {
        let scrubber = PathScrubber::new(true);
        assert_eq!(
            scrubber.scrub_line("finished movie.mp4"),
            "finished file_2b3bd5fa4119.mp4"
        );
    }

    #[test]
    fn configured_folders_render_as_placeholders() {
        let mut scrubber = PathScrubber::new(true);
        scrubber.set_configured_folders(Some(Path::new(r"C:\Videos\In")), None);
        assert_eq!(
            scrubber.scrub_line(r"queued C:\Videos\In\movie.mp4"),
            "queued [input_folder]/file_2b3bd5fa4119.mp4"
        );
    }

    #[test]
    fn scrubbing_is_idempotent() {
        let mut scrubber = PathScrubber::new(true);
        scrubber.set_configured_folders(Some(Path::new(r"C:\Videos\In")), None);
        let lines = [
            r"Converting C:\Users\Someone\Videos\movie.mp4 now",
            r"queued C:\Videos\In\movie.mp4",
            r#"input: "my film 2024.mkv" queued"#,
            "probing /mnt/media/sample.mkv failed",
        ];
        for line in lines {
            let once = scrubber.scrub_line(line);
            assert_eq!(scrubber.scrub_line(&once), once, "double scrub of {line}");
        }
    }

    #[test]
    fn lines_without_paths_pass_through_unchanged() {
        let scrubber = PathScrubber::new(true);
        let line = "worker finished: 3 succeeded, 1 skipped (ratio 0.62)";
        assert_eq!(scrubber.scrub_line(line), line);
    }

    #[test]
    fn empty_inputs_use_the_unknown_labels() {
        let scrubber = PathScrubber::new(false);
        assert_eq!(scrubber.anonymize_folder(""), "[unknown]");
        assert_eq!(scrubber.anonymize_file(""), "file_unknown");
        assert_eq!(scrubber.anonymize_path(""), "[unknown]/file_unknown");
    }
}
