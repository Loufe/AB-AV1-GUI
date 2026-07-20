//! Verified-download contract: corrupt, truncated, and cancelled downloads
//! never yield a usable archive, and nothing survives on disk after failure.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    io::Cursor,
    sync::atomic::{AtomicBool, Ordering},
};

use crfty_engine::vendor::download::{DownloadError, Fetch, FetchStream, download_verified};
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Serves one in-memory body for any URL, with a configurable advertised
/// total so truncation is representable.
struct StaticFetch {
    body: Vec<u8>,
    total: Option<u64>,
}

impl Fetch for StaticFetch {
    fn fetch(&self, _url: &str) -> Result<FetchStream, String> {
        Ok(FetchStream {
            total: self.total,
            reader: Box::new(Cursor::new(self.body.clone())),
        })
    }
}

struct FailingFetch;

impl Fetch for FailingFetch {
    fn fetch(&self, url: &str) -> Result<FetchStream, String> {
        Err(format!("no route to {url}"))
    }
}

fn staging() -> tempfile::TempDir {
    tempfile::tempdir().expect("create staging directory")
}

fn staging_entry_count(staging: &tempfile::TempDir) -> usize {
    std::fs::read_dir(staging.path())
        .expect("list staging")
        .count()
}

#[test]
fn matching_download_lands_verified_with_progress() {
    let body = vec![0xa5_u8; 300 * 1024];
    let fetch = StaticFetch {
        body: body.clone(),
        total: Some(body.len() as u64),
    };
    let staging = staging();
    let mut updates = Vec::new();
    let cancelled = AtomicBool::new(false);
    let file = download_verified(
        &fetch,
        "test://archive",
        &sha256_hex(&body),
        staging.path(),
        &mut |received, total| updates.push((received, total)),
        &cancelled,
    )
    .expect("download succeeds");
    assert_eq!(std::fs::read(file.path()).expect("read download"), body);
    assert!(file.path().starts_with(staging.path()));
    let (final_received, final_total) = *updates.last().expect("progress was reported");
    assert_eq!(final_received, body.len() as u64);
    assert_eq!(final_total, Some(body.len() as u64));
    assert!(
        updates
            .iter()
            .all(|(received, _)| *received <= body.len() as u64),
        "received counts never overshoot"
    );
}

#[test]
fn checksum_mismatch_is_rejected_and_leaves_no_file() {
    let body = b"not the pinned archive".to_vec();
    let fetch = StaticFetch { body, total: None };
    let staging = staging();
    let cancelled = AtomicBool::new(false);
    let result = download_verified(
        &fetch,
        "test://archive",
        &sha256_hex(b"the pinned archive"),
        staging.path(),
        &mut |_received, _total| {},
        &cancelled,
    );
    match result {
        Err(DownloadError::Failed(detail)) => assert!(detail.contains("checksum"), "{detail}"),
        other => panic!("expected a checksum failure, got {other:?}"),
    }
    assert_eq!(staging_entry_count(&staging), 0);
}

#[test]
fn truncated_download_fails_the_checksum() {
    let full = vec![0x42_u8; 64 * 1024];
    let pinned = sha256_hex(&full);
    let truncated = full[..full.len() / 2].to_vec();
    let fetch = StaticFetch {
        body: truncated,
        total: Some(full.len() as u64),
    };
    let staging = staging();
    let cancelled = AtomicBool::new(false);
    let result = download_verified(
        &fetch,
        "test://archive",
        &pinned,
        staging.path(),
        &mut |_received, _total| {},
        &cancelled,
    );
    assert!(
        matches!(result, Err(DownloadError::Failed(ref detail)) if detail.contains("checksum")),
        "{result:?}"
    );
    assert_eq!(staging_entry_count(&staging), 0);
}

#[test]
fn cancellation_stops_the_download() {
    let body = vec![0_u8; 1024];
    let pinned = sha256_hex(&body);
    let fetch = StaticFetch { body, total: None };
    let staging = staging();
    let cancelled = AtomicBool::new(false);
    cancelled.store(true, Ordering::Relaxed);
    let result = download_verified(
        &fetch,
        "test://archive",
        &pinned,
        staging.path(),
        &mut |_received, _total| {},
        &cancelled,
    );
    assert_eq!(result.unwrap_err(), DownloadError::Cancelled);
    assert_eq!(staging_entry_count(&staging), 0);
}

#[test]
fn transport_failure_is_a_typed_error() {
    let staging = staging();
    let cancelled = AtomicBool::new(false);
    let result = download_verified(
        &FailingFetch,
        "test://archive",
        &sha256_hex(b"anything"),
        staging.path(),
        &mut |_received, _total| {},
        &cancelled,
    );
    assert!(
        matches!(result, Err(DownloadError::Failed(ref detail)) if detail.contains("no route")),
        "{result:?}"
    );
}
