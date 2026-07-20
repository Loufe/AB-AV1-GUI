//! Verified vendor downloads. The stream is hashed while it is written and
//! the SHA-256 must match the compiled-in manifest before the archive is
//! usable — TLS protects transport, but the pinned digest is the trust
//! anchor that authenticates content (ADR-008).

use std::{
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Stall timeout. The blocking client re-arms this on every body read, so
/// it bounds silence, not the whole transfer — a slow link finishing a
/// large archive is progress, a silent socket is not.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_CHUNK_BYTES: usize = 128 * 1024;

/// A byte stream for a URL plus its total size when the source knows it.
pub struct FetchStream {
    pub total: Option<u64>,
    pub reader: Box<dyn Read>,
}

/// Transport abstraction: production fetches over HTTPS, tests serve
/// crafted archives from memory or local files without any network.
pub trait Fetch {
    fn fetch(&self, url: &str) -> Result<FetchStream, String>;
}

pub struct HttpFetch {
    client: reqwest::blocking::Client,
}

impl HttpFetch {
    pub fn new() -> Result<Self, String> {
        // reqwest is built without a default TLS provider; ring is installed
        // process-wide here. An Err means a provider is already installed,
        // which is exactly the state this call wants.
        let _already_installed = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("crfty/", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(STALL_TIMEOUT)
            .build()
            .map_err(|error| format!("failed to build the download client: {error}"))?;
        Ok(Self { client })
    }
}

impl Fetch for HttpFetch {
    fn fetch(&self, url: &str) -> Result<FetchStream, String> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| format!("the download request failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("the download server answered {status}"));
        }
        Ok(FetchStream {
            total: response.content_length(),
            reader: Box::new(response),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    Failed(String),
}

/// Streams `url` into a temporary file under `staging`, reporting
/// `(received, total)` after every chunk and honouring `cancelled` between
/// chunks. Returns the file only when the digest matches; the tempfile
/// deletes itself on any earlier drop.
pub fn download_verified(
    fetch: &dyn Fetch,
    url: &str,
    expected_sha256_hex: &str,
    staging: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancelled: &AtomicBool,
) -> Result<NamedTempFile, DownloadError> {
    let stream = fetch.fetch(url).map_err(DownloadError::Failed)?;
    let mut reader = stream.reader;
    let mut file = NamedTempFile::new_in(staging).map_err(|error| {
        DownloadError::Failed(format!("failed to create a staging file: {error}"))
    })?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut buffer = vec![0_u8; DOWNLOAD_CHUNK_BYTES];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let count = reader.read(&mut buffer).map_err(|error| {
            DownloadError::Failed(format!("the download stream failed: {error}"))
        })?;
        if count == 0 {
            break;
        }
        let Some(chunk) = buffer.get(..count) else {
            return Err(DownloadError::Failed(
                "the download stream returned an impossible chunk size".to_owned(),
            ));
        };
        file.write_all(chunk).map_err(|error| {
            DownloadError::Failed(format!("failed to write the download: {error}"))
        })?;
        hasher.update(chunk);
        received = received.saturating_add(count as u64);
        progress(received, stream.total);
    }
    let digest = hex_lower(&hasher.finalize());
    if !digest.eq_ignore_ascii_case(expected_sha256_hex) {
        return Err(DownloadError::Failed(format!(
            "the download does not match the pinned checksum (got {digest})"
        )));
    }
    file.as_file()
        .sync_all()
        .map_err(|error| DownloadError::Failed(format!("failed to sync the download: {error}")))?;
    Ok(file)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push_str(&format!("{byte:02x}"));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::hex_lower;

    #[test]
    fn renders_lowercase_hex() {
        assert_eq!(hex_lower(&[0x00, 0xab, 0x0f]), "00ab0f");
    }
}
