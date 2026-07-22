//! Filesystem discovery for queue adds.
//!
//! Expands user-selected files and folders into concrete video files plus
//! their enqueue facts (path hash and destructive identity). Analysis Level 0
//! deliberately uses the generation-scoped `analysis` runtime instead: queue adds need an eager
//! complete list and enqueue identities, while Analysis must stream native
//! path rows without hashing.

use std::{
    collections::{BTreeSet, VecDeque},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use crfty_core::{DestructiveIdentity, PathHash, TimestampReliability, VideoExtension};

use crate::media;

/// One discovered file with its enqueue facts. Facts are best-effort: a
/// failed stat or canonicalization yields `None` and the add fails open —
/// the reducer enqueues the item and claim-time inspection is authoritative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub path_hash: Option<PathHash>,
    pub identity: Option<DestructiveIdentity>,
    pub timestamp_reliability: TimestampReliability,
    /// The added folder this file was discovered under; `None` for files
    /// passed directly. Feeds `OutputTarget::SeparateFolder`'s
    /// relative-tree layout.
    pub source_root: Option<PathBuf>,
}

/// Expands each input into video files. Files pass through unfiltered — an
/// explicit selection outranks the extension filter — while folders are
/// walked breadth-first with a case-insensitive extension filter. Traversal
/// never follows symlinks (cycle safety; a symlinked file is another path's
/// identity) and never aborts: unreadable entries are logged and skipped.
/// Order is deterministic: inputs in argument order, each folder's files in
/// breadth-first name order.
#[must_use]
pub fn expand_inputs(
    inputs: &[PathBuf],
    extensions: &BTreeSet<VideoExtension>,
) -> Vec<ScannedFile> {
    let mut files = Vec::new();
    for input in inputs {
        match fs::metadata(input) {
            Ok(metadata) if metadata.is_dir() => expand_folder(input, extensions, &mut files),
            Ok(_) => files.push(scanned(input.clone(), None)),
            Err(error) => {
                tracing::warn!("skipping unreadable input {}: {error}", input.display());
            }
        }
    }
    files
}

fn expand_folder(root: &Path, extensions: &BTreeSet<VideoExtension>, files: &mut Vec<ScannedFile>) {
    let mut pending = VecDeque::from([root.to_path_buf()]);
    while let Some(directory) = pending.pop_front() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(
                    "skipping unreadable directory {}: {error}",
                    directory.display()
                );
                continue;
            }
        };
        let mut children: Vec<(PathBuf, fs::FileType)> = Vec::new();
        for entry in entries {
            match entry.and_then(|entry| Ok((entry.path(), entry.file_type()?))) {
                Ok(child) => children.push(child),
                Err(error) => {
                    tracing::warn!(
                        "skipping unreadable entry in {}: {error}",
                        directory.display()
                    );
                }
            }
        }
        children.sort_by(|left, right| left.0.cmp(&right.0));
        for (path, file_type) in children {
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push_back(path);
            } else if matches_extension(&path, extensions) {
                files.push(scanned(path, Some(root.to_path_buf())));
            }
        }
    }
}

fn matches_extension(path: &Path, extensions: &BTreeSet<VideoExtension>) -> bool {
    let Some(extension) = path.extension().and_then(OsStr::to_str) else {
        return false;
    };
    extensions
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate.as_extension()))
}

fn scanned(path: PathBuf, source_root: Option<PathBuf>) -> ScannedFile {
    let path_hash = match media::path_hash(&path) {
        Ok(hash) => Some(hash),
        Err(error) => {
            tracing::warn!("failed to hash path {}: {error}", path.display());
            None
        }
    };
    let identity = match media::destructive_identity(&path) {
        Ok(identity) => Some(identity),
        Err(error) => {
            tracing::warn!("failed to identify file {}: {error}", path.display());
            None
        }
    };
    let timestamp_reliability = identity
        .as_ref()
        .map_or(TimestampReliability::Unknown, |identity| {
            media::timestamp_reliability(identity, std::time::SystemTime::now())
        });
    ScannedFile {
        path,
        path_hash,
        identity,
        timestamp_reliability,
        source_root,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use crfty_core::VideoExtension;

    use super::expand_inputs;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_directory(label: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crfty-{label}-{}-{sequence}", std::process::id()));
        fs::create_dir(&path).expect("fixture directory");
        path
    }

    fn all_extensions() -> std::collections::BTreeSet<VideoExtension> {
        [
            VideoExtension::Mp4,
            VideoExtension::Mkv,
            VideoExtension::Avi,
            VideoExtension::Wmv,
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn folders_expand_breadth_first_in_sorted_order_with_facts() {
        let root = test_directory("scan-bfs");
        fs::write(root.join("b.mkv"), b"b").expect("b.mkv");
        fs::write(root.join("a.mp4"), b"aa").expect("a.mp4");
        fs::write(root.join("notes.txt"), b"skip").expect("notes.txt");
        fs::create_dir(root.join("zebra")).expect("zebra");
        fs::write(root.join("zebra").join("c.avi"), b"ccc").expect("c.avi");
        fs::create_dir(root.join("alpha")).expect("alpha");
        fs::write(root.join("alpha").join("d.wmv"), b"dddd").expect("d.wmv");
        fs::create_dir(root.join("alpha").join("empty")).expect("empty");

        let files = expand_inputs(std::slice::from_ref(&root), &all_extensions());

        let paths: Vec<_> = files.iter().map(|file| file.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                root.join("a.mp4"),
                root.join("b.mkv"),
                root.join("alpha").join("d.wmv"),
                root.join("zebra").join("c.avi"),
            ]
        );
        for file in &files {
            assert_eq!(file.source_root.as_deref(), Some(root.as_path()));
            assert!(file.path_hash.is_some());
        }
        let sizes: Vec<_> = files
            .iter()
            .map(|file| file.identity.as_ref().expect("identity").size)
            .collect();
        assert_eq!(sizes, vec![2, 1, 4, 3]);
        fs::remove_dir_all(root).expect("remove fixture directory");
    }

    #[test]
    fn extension_filter_is_case_insensitive_and_honors_the_configured_set() {
        let root = test_directory("scan-case");
        fs::write(root.join("upper.MKV"), b"u").expect("upper.MKV");
        fs::write(root.join("mixed.Mp4"), b"m").expect("mixed.Mp4");
        fs::write(root.join("plain.avi"), b"p").expect("plain.avi");
        fs::write(root.join("noext"), b"n").expect("noext");

        let only_mkv = [VideoExtension::Mkv].into_iter().collect();
        let files = expand_inputs(std::slice::from_ref(&root), &only_mkv);
        let paths: Vec<_> = files.iter().map(|file| file.path.clone()).collect();
        assert_eq!(paths, vec![root.join("upper.MKV")]);
        fs::remove_dir_all(root).expect("remove fixture directory");
    }

    #[test]
    fn direct_file_inputs_bypass_the_filter_and_carry_no_source_root() {
        let root = test_directory("scan-direct");
        let file = root.join("explicit.webm");
        fs::write(&file, b"webm").expect("explicit.webm");

        let files = expand_inputs(std::slice::from_ref(&file), &all_extensions());
        assert_eq!(files.len(), 1);
        let scanned = files.first().expect("one file");
        assert_eq!(scanned.path, file);
        assert_eq!(scanned.source_root, None);
        assert!(scanned.path_hash.is_some());
        assert_eq!(scanned.identity.as_ref().expect("identity").size, 4);
        fs::remove_dir_all(root).expect("remove fixture directory");
    }

    #[test]
    fn empty_folders_and_missing_inputs_yield_nothing() {
        let root = test_directory("scan-empty");
        let missing = root.join("does-not-exist");
        assert_eq!(
            expand_inputs(&[root.clone(), missing], &all_extensions()),
            Vec::new()
        );
        fs::remove_dir_all(root).expect("remove fixture directory");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directories_and_files_are_not_followed() {
        let root = test_directory("scan-symlink");
        let real = test_directory("scan-symlink-target");
        fs::write(real.join("real.mkv"), b"r").expect("real.mkv");
        fs::write(root.join("kept.mkv"), b"k").expect("kept.mkv");
        std::os::unix::fs::symlink(&real, root.join("loop")).expect("dir symlink");
        std::os::unix::fs::symlink(real.join("real.mkv"), root.join("alias.mkv"))
            .expect("file symlink");

        let files = expand_inputs(std::slice::from_ref(&root), &all_extensions());
        let paths: Vec<_> = files.iter().map(|file| file.path.clone()).collect();
        assert_eq!(paths, vec![root.join("kept.mkv")]);
        fs::remove_dir_all(root).expect("remove fixture directory");
        fs::remove_dir_all(real).expect("remove target directory");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directories_are_skipped_without_aborting_the_scan() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_directory("scan-unreadable");
        fs::write(root.join("visible.mkv"), b"v").expect("visible.mkv");
        let secret = root.join("secret");
        fs::create_dir(&secret).expect("secret directory");
        fs::write(secret.join("hidden.mkv"), b"h").expect("hidden.mkv");
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read_dir(&secret).is_ok() {
            // Running privileged: an unreadable directory is unrepresentable.
            fs::set_permissions(&secret, fs::Permissions::from_mode(0o755)).expect("chmod back");
            fs::remove_dir_all(root).expect("remove fixture directory");
            return;
        }

        let files = expand_inputs(std::slice::from_ref(&root), &all_extensions());
        let paths: Vec<_> = files.iter().map(|file| file.path.clone()).collect();
        assert_eq!(paths, vec![root.join("visible.mkv")]);

        fs::set_permissions(&secret, fs::Permissions::from_mode(0o755)).expect("chmod back");
        fs::remove_dir_all(root).expect("remove fixture directory");
    }
}
