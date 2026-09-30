//! Real-filesystem contract for source observations: file ID, size, and
//! modification time come from one object, Unix change time is recorded as
//! assessment evidence, and undetectable mutation classes are asserted as
//! the documented limits of ADR-019 rather than as detections.
#![forbid(unsafe_code)]

use std::{
    fs::{self, File, FileTimes, OpenOptions},
    io,
    time::{Duration, UNIX_EPOCH},
};

use crfty_engine::media::source_observation;

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

#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn restored_modification_time_hides_a_same_size_write_from_destructive_identity() {
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
    #[cfg(unix)]
    wait_for_change_time_tick(directory.path(), &before);

    fs::write(&path, b"after!").expect("same-size rewrite");
    set_old_modified_time(
        &OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("restoring handle"),
    )
    .expect("restore modification time");
    let after = source_observation(&path).expect("observation after restoration");

    assert_eq!(before.destructive, after.destructive);
    #[cfg(unix)]
    {
        assert!(before.changed_ns.is_some(), "Unix change time is required");
        assert_ne!(before.changed_ns, after.changed_ns);
    }
    #[cfg(windows)]
    {
        assert_eq!(before.changed_ns, None);
        assert_eq!(before, after);
    }
}

/// Coarse kernel clocks can stamp consecutive operations with one change
/// time; wait until a fresh write lands on a later tick than `before`.
#[cfg(unix)]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn wait_for_change_time_tick(
    directory: &std::path::Path,
    before: &crfty_engine::media::SourceObservation,
) {
    let clock = directory.join("clock.probe");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        fs::write(&clock, b"tick").expect("clock probe");
        let probe = source_observation(&clock).expect("clock observation");
        if probe.changed_ns > before.changed_ns {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "change time never advanced"
        );
        std::thread::sleep(Duration::from_millis(5));
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

#[cfg(unix)]
#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn unreadable_file_remains_observable_on_unix() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("output.mkv");
    fs::write(&path, b"output").expect("output fixture");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("remove permissions");

    let observation = source_observation(&path).expect("stat needs no read permission");

    assert_eq!(observation.destructive.size, 6);
}

#[cfg(unix)]
#[test]
#[expect(clippy::expect_used, reason = "filesystem fixture assertions")]
fn fifo_observation_returns_without_a_writer() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("pipe.mkv");
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed");

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(source_observation(&path).map(|_| ()));
    });

    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("observing a FIFO must not wait for a writer")
        .expect("FIFO observation");
}
