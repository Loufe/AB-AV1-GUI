//! Install-pipeline contract: a successful install activates atomically and
//! is discoverable; any failure — corrupt download, malicious archive,
//! cancellation — leaves the previous install and `current.json` untouched.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    io::Cursor,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use crfty_engine::vendor::{
    download::{Fetch, FetchStream},
    install::{InstallError, install},
    manifest::{ArchiveKind, VendorManifest},
};
use sha2::{Digest, Sha256};

const BUILD: &str = "ffmpeg-test-build";
const FFMPEG_ENTRY: &str = "ffmpeg-test-build/bin/ffmpeg";
const FFPROBE_ENTRY: &str = "ffmpeg-test-build/bin/ffprobe";

/// Builds a valid tar.xz vendor archive containing the two binaries.
fn archive_bytes(ffmpeg_contents: &[u8], ffprobe_contents: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, contents) in [
        (FFMPEG_ENTRY, ffmpeg_contents),
        (FFPROBE_ENTRY, ffprobe_contents),
    ] {
        let mut header = tar::Header::new_gnu();
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, contents).expect("append tar entry");
    }
    let tar_bytes = builder.into_inner().expect("finish tar");
    let mut compressed = Vec::new();
    lzma_rs::xz_compress(&mut Cursor::new(tar_bytes), &mut compressed).expect("compress tar");
    compressed
}

/// A manifest whose pinned hash matches `archive`, unless `pinned_hex`
/// overrides it to simulate corruption.
fn manifest(archive: &[u8], pinned_hex: Option<&str>) -> VendorManifest {
    let digest: String = Sha256::digest(archive)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let sha256_hex = pinned_hex.unwrap_or(&digest).to_owned();
    VendorManifest {
        tag: "test-tag",
        build: BUILD,
        url: "test://archive",
        sha256_hex: Box::leak(sha256_hex.into_boxed_str()),
        archive: ArchiveKind::TarXz,
        ffmpeg_entry: FFMPEG_ENTRY,
        ffprobe_entry: FFPROBE_ENTRY,
        max_extracted_bytes: 1024 * 1024,
    }
}

struct ArchiveFetch {
    archive: Vec<u8>,
}

impl Fetch for ArchiveFetch {
    fn fetch(&self, url: &str) -> Result<FetchStream, String> {
        if url != "test://archive" {
            return Err(format!("unexpected url {url}"));
        }
        Ok(FetchStream {
            total: Some(self.archive.len() as u64),
            reader: Box::new(Cursor::new(self.archive.clone())),
        })
    }
}

fn run_install(
    vendor_root: &Path,
    manifest: &VendorManifest,
    archive: Vec<u8>,
) -> Result<crfty_engine::vendor::discovery::InstalledMetadata, InstallError> {
    let cancelled = AtomicBool::new(false);
    install(
        vendor_root,
        manifest,
        &ArchiveFetch { archive },
        &mut |_received, _total| {},
        &cancelled,
    )
}

/// Snapshot of the state a failed install must not disturb.
fn seed_previous_install(vendor_root: &Path) -> (Vec<u8>, Vec<u8>) {
    let previous_bin = vendor_root
        .join("installs")
        .join("previous-build")
        .join("bin");
    std::fs::create_dir_all(&previous_bin).expect("create previous install");
    std::fs::write(previous_bin.join("ffmpeg"), b"previous ffmpeg").expect("write previous");
    std::fs::write(previous_bin.join("ffprobe"), b"previous ffprobe").expect("write previous");
    let record = br#"{
  "version": "previous-build",
  "ffmpeg": "installs/previous-build/bin/ffmpeg",
  "ffprobe": "installs/previous-build/bin/ffprobe",
  "ffmpeg_revision": "previous-build",
  "encoder_revision": "previous-build"
}"#;
    std::fs::write(vendor_root.join("current.json"), record).expect("write current.json");
    (record.to_vec(), b"previous ffmpeg".to_vec())
}

fn assert_previous_untouched(vendor_root: &Path, record: &[u8], ffmpeg: &[u8]) {
    assert_eq!(
        std::fs::read(vendor_root.join("current.json")).expect("read current.json"),
        record,
        "current.json must be untouched"
    );
    assert_eq!(
        std::fs::read(
            vendor_root
                .join("installs")
                .join("previous-build")
                .join("bin")
                .join("ffmpeg")
        )
        .expect("read previous binary"),
        ffmpeg,
        "the previous install must be untouched"
    );
}

#[test]
fn install_activates_the_build_atomically() {
    let vendor = tempfile::tempdir().expect("create vendor root");
    let archive = archive_bytes(b"new ffmpeg", b"new ffprobe");
    let manifest = manifest(&archive, None);
    let metadata = run_install(vendor.path(), &manifest, archive).expect("install succeeds");
    assert_eq!(metadata.version, BUILD);
    assert_eq!(metadata.ffmpeg_revision, BUILD);
    assert_eq!(metadata.encoder_revision, BUILD);
    let ffmpeg = vendor.path().join(&metadata.ffmpeg);
    let ffprobe = vendor.path().join(&metadata.ffprobe);
    assert_eq!(std::fs::read(&ffmpeg).expect("read ffmpeg"), b"new ffmpeg");
    assert_eq!(
        std::fs::read(&ffprobe).expect("read ffprobe"),
        b"new ffprobe"
    );
    let recorded: serde_json::Value = serde_json::from_slice(
        &std::fs::read(vendor.path().join("current.json")).expect("read current.json"),
    )
    .expect("parse current.json");
    assert_eq!(recorded["version"], BUILD);
    assert_eq!(recorded["ffmpeg_revision"], BUILD);
    // The staging area holds nothing once the install is active.
    let staging_entries = std::fs::read_dir(vendor.path().join("staging"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(staging_entries, 0);
}

#[test]
fn corrupt_download_never_touches_the_active_install() {
    let vendor = tempfile::tempdir().expect("create vendor root");
    let (record, previous_ffmpeg) = seed_previous_install(vendor.path());
    let archive = archive_bytes(b"new ffmpeg", b"new ffprobe");
    let wrong_pin = "0".repeat(64);
    let manifest = manifest(&archive, Some(&wrong_pin));
    let result = run_install(vendor.path(), &manifest, archive);
    assert!(
        matches!(result, Err(InstallError::Failed(ref detail)) if detail.contains("checksum")),
        "{result:?}"
    );
    assert_previous_untouched(vendor.path(), &record, &previous_ffmpeg);
    assert!(
        !vendor.path().join("installs").join(BUILD).exists(),
        "no new install directory may appear"
    );
}

#[test]
fn malicious_archive_never_touches_the_active_install() {
    let vendor = tempfile::tempdir().expect("create vendor root");
    let (record, previous_ffmpeg) = seed_previous_install(vendor.path());
    // Valid download, wrong layout: the ffprobe binary is missing.
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.as_mut_bytes()[..FFMPEG_ENTRY.len()].copy_from_slice(FFMPEG_ENTRY.as_bytes());
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(6);
    header.set_cksum();
    builder
        .append(&header, &b"ffmpeg"[..])
        .expect("append entry");
    let tar_bytes = builder.into_inner().expect("finish tar");
    let mut archive = Vec::new();
    lzma_rs::xz_compress(&mut Cursor::new(tar_bytes), &mut archive).expect("compress tar");
    let manifest = manifest(&archive, None);
    let result = run_install(vendor.path(), &manifest, archive);
    assert!(
        matches!(result, Err(InstallError::Failed(ref detail)) if detail.contains("does not contain")),
        "{result:?}"
    );
    assert_previous_untouched(vendor.path(), &record, &previous_ffmpeg);
    // The failed staging attempt cleaned up after itself.
    let staging_entries = std::fs::read_dir(vendor.path().join("staging"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(staging_entries, 0);
}

#[test]
fn cancellation_before_the_download_leaves_nothing_behind() {
    let vendor = tempfile::tempdir().expect("create vendor root");
    let (record, previous_ffmpeg) = seed_previous_install(vendor.path());
    let archive = archive_bytes(b"new ffmpeg", b"new ffprobe");
    let manifest = manifest(&archive, None);
    let cancelled = AtomicBool::new(false);
    cancelled.store(true, Ordering::Relaxed);
    let result = install(
        vendor.path(),
        &manifest,
        &ArchiveFetch { archive },
        &mut |_received, _total| {},
        &cancelled,
    );
    assert_eq!(result.unwrap_err(), InstallError::Cancelled);
    assert_previous_untouched(vendor.path(), &record, &previous_ffmpeg);
}

#[test]
fn same_version_reinstall_replaces_a_broken_install() {
    let vendor = tempfile::tempdir().expect("create vendor root");
    let broken_bin = vendor.path().join("installs").join(BUILD).join("bin");
    std::fs::create_dir_all(&broken_bin).expect("create broken install");
    std::fs::write(broken_bin.join("ffmpeg"), b"corrupt garbage").expect("write broken binary");
    let archive = archive_bytes(b"repaired ffmpeg", b"repaired ffprobe");
    let manifest = manifest(&archive, None);
    let metadata = run_install(vendor.path(), &manifest, archive).expect("reinstall succeeds");
    assert_eq!(
        std::fs::read(vendor.path().join(&metadata.ffmpeg)).expect("read ffmpeg"),
        b"repaired ffmpeg"
    );
}

#[test]
fn superseded_installs_are_pruned_after_activation() {
    let vendor = tempfile::tempdir().expect("create vendor root");
    seed_previous_install(vendor.path());
    let archive = archive_bytes(b"new ffmpeg", b"new ffprobe");
    let manifest = manifest(&archive, None);
    run_install(vendor.path(), &manifest, archive).expect("install succeeds");
    assert!(
        !vendor
            .path()
            .join("installs")
            .join("previous-build")
            .exists(),
        "the superseded install is pruned"
    );
    assert!(vendor.path().join("installs").join(BUILD).exists());
}
