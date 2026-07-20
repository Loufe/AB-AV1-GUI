use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crfty_core::{
    DurableDelta, DurableState, JournalEnvelope, JournalReplay, JournalSequence, UnixMillis,
    encode_record, encode_snapshot, replay,
};
use tempfile::NamedTempFile;

use crate::filesystem::{parent_directory, sync_parent};

/// Antivirus and indexing services on Windows briefly hold newly written
/// files open, which makes the atomic replace fail with a sharing violation.
/// A short bounded retry absorbs that; a real failure still surfaces.
const PERSIST_ATTEMPTS: u32 = 5;
const PERSIST_BACKOFF: Duration = Duration::from_millis(20);

#[derive(Debug)]
pub struct JournalError {
    context: &'static str,
    source: io::Error,
}

impl JournalError {
    fn new(context: &'static str, source: io::Error) -> Self {
        Self { context, source }
    }

    #[cfg(test)]
    pub(crate) fn injected(source: io::Error) -> Self {
        Self::new("injected journal failure", source)
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
pub struct DurabilityToken {
    _private: (),
}

impl DurabilityToken {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }
}

pub struct JournalWriter {
    path: PathBuf,
    /// `None` only between closing the old generation and opening the new one
    /// during compaction, or after a failed compaction whose reopen also
    /// failed — in which case the next append fails and the driver goes fatal.
    file: Option<File>,
    next_sequence: JournalSequence,
    bytes_len: u64,
}

impl JournalWriter {
    pub fn open(path: impl AsRef<Path>) -> Result<(Self, JournalReplay), JournalError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| JournalError::new("failed to create journal directory", error))?;
        }
        let mut file = open_append(&path)
            .map_err(|error| JournalError::new("failed to open journal", error))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| JournalError::new("failed to seek journal", error))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| JournalError::new("failed to read journal", error))?;
        let replay = replay(&bytes);
        let mut bytes_len = u64::try_from(bytes.len()).map_err(|_| {
            JournalError::new(
                "journal length exceeds addressable range",
                io::Error::from(io::ErrorKind::InvalidData),
            )
        })?;
        // A torn tail is the expected crash-during-append residue and must be
        // truncated before the next append: appending after the partial record
        // would merge both into one unparseable line, and the journal would
        // load as corrupt on the following start. A corrupt journal is left
        // byte-identical for archival and explicit acknowledgment.
        if replay.corruption.is_none() && replay.ignored_torn_tail {
            let prefix = u64::try_from(replay.valid_prefix_len).map_err(|_| {
                JournalError::new(
                    "torn tail offset exceeds file range",
                    io::Error::from(io::ErrorKind::InvalidData),
                )
            })?;
            file.set_len(prefix)
                .map_err(|error| JournalError::new("failed to truncate torn tail", error))?;
            file.sync_all()
                .map_err(|error| JournalError::new("failed to synchronize truncation", error))?;
            bytes_len = prefix;
        }
        let writer = Self {
            path,
            file: Some(file),
            next_sequence: replay.next_sequence,
            bytes_len,
        };
        Ok((writer, replay))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current on-disk journal size, tracked without stat calls so the
    /// driver's idle-tick compaction check stays free.
    #[must_use]
    pub fn journal_bytes(&self) -> u64 {
        self.bytes_len
    }

    pub fn append_batch(
        &mut self,
        deltas: &[DurableDelta],
    ) -> Result<(Vec<JournalEnvelope>, DurabilityToken), JournalError> {
        if deltas.is_empty() {
            return Ok((Vec::new(), DurabilityToken::new()));
        }
        let envelope = JournalEnvelope {
            sequence: self.next_sequence,
            deltas: deltas.to_vec(),
        };
        let encoded = encode_record(&envelope).map_err(|error| {
            JournalError::new(
                "failed to encode journal batch",
                io::Error::new(io::ErrorKind::InvalidData, error),
            )
        })?;
        let file = self.file.as_mut().ok_or_else(|| {
            JournalError::new(
                "journal handle lost during compaction",
                io::Error::from(io::ErrorKind::NotConnected),
            )
        })?;
        file.write_all(&encoded)
            .map_err(|error| JournalError::new("failed to append journal batch", error))?;
        file.sync_all()
            .map_err(|error| JournalError::new("failed to synchronize journal", error))?;
        self.next_sequence =
            JournalSequence(self.next_sequence.0.checked_add(1).ok_or_else(|| {
                JournalError::new(
                    "journal sequence overflow",
                    io::Error::new(io::ErrorKind::InvalidData, "sequence overflow"),
                )
            })?);
        self.bytes_len = self
            .bytes_len
            .saturating_add(u64::try_from(encoded.len()).unwrap_or(u64::MAX));
        Ok((vec![envelope], DurabilityToken::new()))
    }

    /// Replace the journal with a single snapshot line of the folded state
    /// (#33 §10). Runs only at the driver's writer barrier: the current batch
    /// is finished and no append can race this. Sequence numbering continues —
    /// the first record after the snapshot carries the same sequence the next
    /// append would have carried before compaction.
    ///
    /// On any failure the old generation stays authoritative: the temp file is
    /// discarded, the original journal is untouched (or reopened), and the
    /// error is returned for logging so the driver retries at a later idle
    /// barrier instead of going fatal.
    pub fn compact(
        &mut self,
        state: &DurableState,
        app_version: &str,
        compacted_at: UnixMillis,
    ) -> Result<(), JournalError> {
        let encoded = encode_snapshot(app_version, compacted_at, self.next_sequence, state)
            .map_err(|error| {
                JournalError::new(
                    "failed to encode journal snapshot",
                    io::Error::new(io::ErrorKind::InvalidData, error),
                )
            })?;
        let parent = parent_directory(&self.path);
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|error| JournalError::new("failed to create snapshot temp file", error))?;
        temporary
            .write_all(&encoded)
            .map_err(|error| JournalError::new("failed to write journal snapshot", error))?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|error| JournalError::new("failed to synchronize journal snapshot", error))?;
        // Windows refuses to replace a file another handle has open, so the
        // old generation's handle must close before the atomic swap.
        self.file = None;
        if let Err(error) = persist_with_retry(temporary, &self.path) {
            // The replace never happened, so the old journal is still intact;
            // reacquire its handle and report the failure for a later retry.
            self.file = open_append(&self.path)
                .map_err(|reopen| {
                    JournalError::new("failed to reopen journal after failed compaction", reopen)
                })
                .map(Some)?;
            return Err(JournalError::new(
                "failed to replace journal with snapshot",
                error,
            ));
        }
        sync_parent(&self.path)
            .map_err(|error| JournalError::new("failed to synchronize journal directory", error))?;
        self.file = Some(
            open_append(&self.path)
                .map_err(|error| JournalError::new("failed to reopen compacted journal", error))?,
        );
        self.bytes_len = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
        Ok(())
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
}

fn persist_with_retry(mut temporary: NamedTempFile, path: &Path) -> Result<(), io::Error> {
    let mut attempt = 1_u32;
    loop {
        match temporary.persist(path) {
            Ok(_) => return Ok(()),
            Err(error) if attempt < PERSIST_ATTEMPTS => {
                temporary = error.file;
                std::thread::sleep(PERSIST_BACKOFF.saturating_mul(attempt));
                attempt = attempt.saturating_add(1);
            }
            Err(error) => return Err(error.error),
        }
    }
}
