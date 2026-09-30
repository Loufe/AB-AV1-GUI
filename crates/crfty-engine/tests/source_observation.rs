#![forbid(unsafe_code)]

use std::{
    fs::{self, File, FileTimes, OpenOptions},
    io,
    time::{Duration, UNIX_EPOCH},
};

use crfty_engine::media::source_observation;

#[cfg(unix)]
use crfty_core::FileTimeNs;

#[cfg(unix)]
fn unix_change_time_ns(metadata: &fs::Metadata) -> Option<FileTimeNs> {
    use std::os::unix::fs::MetadataExt as _;

    let seconds = u64::try_from(metadata.ctime()).ok()?;
    let nanos = u64::try_from(metadata.ctime_nsec()).ok()?;
    seconds
        .checked_mul(1_000_000_000)?
        .checked_add(nanos)
        .map(FileTimeNs)
}

fn set_old_modified_time(file: &File) -> io::Result<()> {
    file.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(86_400)))
}

#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn same_size_write_changes_the_observed_source() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("source.mkv");
    fs::write(&path, b"before").expect("original source");
    set_old_modified_time(
        &OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("source handle"),
    )
    .expect("set initial modification time");
    let before = source_observation(&path).expect("initial observation");

    fs::write(&path, b"after!").expect("same-size rewrite");
    let after = source_observation(&path).expect("observation after rewrite");

    assert_eq!(before.destructive.size, after.destructive.size);
    assert_eq!(before.destructive.file_id, after.destructive.file_id);
    assert_ne!(
        before.destructive.modified_ns,
        after.destructive.modified_ns
    );
}

#[cfg(unix)]
#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn restored_modification_time_reflects_unix_change_time() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("source.mkv");
    fs::write(&path, b"before").expect("original source");
    let file = OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("source handle");
    set_old_modified_time(&file).expect("set initial modification time");
    let before = source_observation(&path).expect("initial observation");
    let before_metadata = file.metadata().expect("initial metadata");

    fs::write(&path, b"after!").expect("same-size rewrite");
    set_old_modified_time(&file).expect("restore modification time");
    let after = source_observation(&path).expect("observation after restoration");
    let after_metadata = file.metadata().expect("metadata after restoration");

    assert_eq!(before.destructive, after.destructive);
    assert_eq!(before.changed_ns, unix_change_time_ns(&before_metadata));
    assert_eq!(after.changed_ns, unix_change_time_ns(&after_metadata));
    assert!(before.changed_ns.is_some(), "Unix change time is required");
    // A coarse filesystem clock can report the same change time for both
    // operations; metadata observations cannot recover that lost ordering.
    if unix_change_time_ns(&before_metadata) != unix_change_time_ns(&after_metadata) {
        assert_ne!(before.changed_ns, after.changed_ns);
    }
}

#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn replacement_and_restoration_return_the_original_destructive_identity() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("source.mkv");
    let parked = directory.path().join("parked.mkv");
    let replacement = directory.path().join("replacement.mkv");
    fs::write(&path, b"original").expect("original source");
    fs::write(&replacement, b"new file").expect("replacement source");
    let before = source_observation(&path).expect("initial observation");

    fs::rename(&path, &parked).expect("park original source");
    fs::rename(&replacement, &path).expect("replace source");
    let during = source_observation(&path).expect("replacement observation");
    assert_ne!(before.destructive.file_id, during.destructive.file_id);

    fs::rename(&path, &replacement).expect("remove replacement from source path");
    fs::rename(&parked, &path).expect("restore original source");
    let restored = source_observation(&path).expect("restored observation");

    // Destructive identity can miss a temporary replacement when the original
    // object returns with the same ID, size, and modification time.
    assert_eq!(before.destructive, restored.destructive);
}

#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn hardlink_write_is_visible_from_the_original_path() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("source.mkv");
    let link = directory.path().join("linked.mkv");
    fs::write(&path, b"before").expect("original source");
    set_old_modified_time(
        &OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("source handle"),
    )
    .expect("set initial modification time");
    fs::hard_link(&path, &link).expect("hardlink source");
    let before = source_observation(&path).expect("initial observation");

    fs::write(&link, b"after!").expect("write through hardlink");
    let after = source_observation(&path).expect("observation after hardlink write");

    assert_eq!(before.destructive.file_id, after.destructive.file_id);
    assert_eq!(before.destructive.size, after.destructive.size);
    assert_ne!(
        before.destructive.modified_ns,
        after.destructive.modified_ns
    );
}

#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn lexical_path_alias_observes_the_same_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("source.mkv");
    let child = directory.path().join("child");
    fs::create_dir(&child).expect("alias directory");
    fs::write(&path, b"source").expect("source fixture");
    let alias = child.join("..").join("source.mkv");

    let direct = source_observation(&path).expect("direct observation");
    let through_alias = source_observation(&alias).expect("alias observation");

    assert_eq!(direct, through_alias);
}

#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn missing_source_reports_inspection_failure() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let missing = directory.path().join("missing.mkv");

    let error = source_observation(&missing).expect_err("missing source must fail inspection");

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}
