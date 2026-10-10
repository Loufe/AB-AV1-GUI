use std::{
    ffi::OsString,
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
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
pub enum JournalError {
    Io {
        context: &'static str,
        source: io::Error,
    },
    CompactionRecovery {
        replacement: io::Error,
        reopen: io::Error,
    },
}

impl JournalError {
    fn new(context: &'static str, source: io::Error) -> Self {
        Self::Io { context, source }
    }

    #[cfg(test)]
    pub(crate) fn injected(source: io::Error) -> Self {
        Self::new("injected journal failure", source)
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
            Self::CompactionRecovery {
                replacement,
                reopen,
            } => write!(
                formatter,
                "failed to replace journal with snapshot: {replacement}; failed to reopen journal after failed compaction: {reopen}"
            ),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::CompactionRecovery { replacement, .. } => Some(replacement),
        }
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
            // `set_len` needs write access, and Windows append-mode handles
            // carry only FILE_APPEND_DATA — truncate through a dedicated
            // write handle, then reopen for appending.
            drop(file);
            let truncate = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(|error| {
                    JournalError::new("failed to open journal for truncation", error)
                })?;
            truncate
                .set_len(prefix)
                .map_err(|error| JournalError::new("failed to truncate torn tail", error))?;
            truncate
                .sync_all()
                .map_err(|error| JournalError::new("failed to synchronize truncation", error))?;
            drop(truncate);
            file = open_append(&path)
                .map_err(|error| JournalError::new("failed to reopen truncated journal", error))?;
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

    /// Replace the journal with a single snapshot line of the folded state.
    /// Runs only at the driver's writer barrier: the current batch
    /// is finished and no append can race this. Sequence numbering continues:
    /// the first record after the snapshot carries the same sequence the next
    /// append would have carried before compaction.
    ///
    /// Failed replacement leaves the previous generation authoritative. Its
    /// handle is reopened before another append; if that also fails, both
    /// causes survive and the writer refuses further appends without a handle.
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
            return Err(self.reopen_after_failed_compaction(error));
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

    fn reopen_after_failed_compaction(&mut self, replacement: io::Error) -> JournalError {
        // The failed replacement left the previous generation authoritative;
        // another append is possible only after its handle is restored.
        match open_append(&self.path) {
            Ok(file) => {
                self.file = Some(file);
                JournalError::new("failed to replace journal with snapshot", replacement)
            }
            Err(reopen) => JournalError::CompactionRecovery {
                replacement,
                reopen,
            },
        }
    }

    /// Recover from acknowledged corruption: archive the corrupt journal to a
    /// `<name>.corrupt-<timestamp>` sibling, then compact the valid-prefix
    /// state into a fresh generation. Returns the archive path.
    ///
    /// The archive is a copy, never a rename. If the compaction that follows
    /// fails, the corrupt original must still be at the journal path —
    /// `compact`'s failure path reopens that path with create semantics, so a
    /// rename would have it materialize an *empty* journal, which is data
    /// loss. Copy-then-replace is also crash-safe: a crash after the copy
    /// leaves the corrupt journal in place, the next start degrades again,
    /// and the acknowledgement is simply retried.
    pub(crate) fn recover_corrupt(
        &mut self,
        state: &DurableState,
        app_version: &str,
        recovered_at: UnixMillis,
    ) -> Result<PathBuf, JournalError> {
        let archive = corrupt_archive_path(&self.path)?;
        // Windows refuses to replace a file another handle has open, so the
        // archival copy's handles must close before `compact` swaps the
        // snapshot over the journal path.
        {
            let mut source = File::open(&self.path)
                .map_err(|error| JournalError::new("failed to open journal for archival", error))?;
            let mut destination = File::create(&archive)
                .map_err(|error| JournalError::new("failed to create corruption archive", error))?;
            io::copy(&mut source, &mut destination)
                .map_err(|error| JournalError::new("failed to copy corruption archive", error))?;
            destination.sync_all().map_err(|error| {
                JournalError::new("failed to synchronize corruption archive", error)
            })?;
        }
        sync_parent(&self.path)
            .map_err(|error| JournalError::new("failed to synchronize journal directory", error))?;
        self.compact(state, app_version, recovered_at)?;
        Ok(archive)
    }
}

/// Mirror of the config store's `.invalid-<nanos>` quarantine naming: the
/// archive sits beside the journal, unmistakably stamped, never overwritten.
fn corrupt_archive_path(path: &Path) -> Result<PathBuf, JournalError> {
    let file_name = path.file_name().ok_or_else(|| {
        JournalError::new(
            "journal path has no file name",
            io::Error::new(io::ErrorKind::InvalidInput, "missing file name"),
        )
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            JournalError::new(
                "system clock is before the Unix epoch",
                io::Error::other(error),
            )
        })?
        .as_nanos();
    let mut archive_name = OsString::from(file_name);
    archive_name.push(format!(".corrupt-{timestamp}"));
    Ok(path.with_file_name(archive_name))
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

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, io, path::PathBuf};

    use crfty_core::{
        AnalysisIntent, DurableDelta, Operation, OutputTarget, OverwriteDecision, QueueItem,
        QueueItemId, QueueItemState, replay,
    };

    use super::{JournalError, JournalWriter};

    fn queued(id: u64) -> DurableDelta {
        DurableDelta::QueueAdded {
            item: QueueItem {
                id: QueueItemId(id),
                input: PathBuf::from(format!("fixture-{id}.mkv")),
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                overwrite: OverwriteDecision::FollowSettings,
                state: QueueItemState::Queued,
            },
        }
    }

    #[test]
    fn successful_reopen_after_failed_replacement_preserves_the_writer()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("journal.jsonl");
        let (mut writer, _) = JournalWriter::open(&path)?;
        writer.append_batch(&[queued(1)])?;
        let before = fs::read(&path)?;
        let before_len = writer.journal_bytes();
        let before_sequence = writer.next_sequence;
        writer.file = None;

        let error = writer.reopen_after_failed_compaction(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "replacement rejected",
        ));

        assert!(matches!(&error, JournalError::Io { source, .. }
            if source.kind() == io::ErrorKind::PermissionDenied));
        assert_eq!(
            error.source().map(ToString::to_string),
            Some("replacement rejected".to_owned())
        );
        assert_eq!(fs::read(&path)?, before);
        assert_eq!(writer.journal_bytes(), before_len);
        assert_eq!(writer.next_sequence, before_sequence);
        writer.append_batch(&[queued(2)])?;
        drop(writer);

        let (_, restarted) = JournalWriter::open(&path)?;
        assert!(restarted.corruption.is_none());
        assert_eq!(restarted.state.queue.len(), 2);
        assert_eq!(restarted.next_sequence.0, 2);
        assert_eq!(replay(&before).state.queue.len(), 1);
        Ok(())
    }
}
