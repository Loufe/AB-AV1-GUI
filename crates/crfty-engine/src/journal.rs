use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use crfty_core::{
    DurableDelta, JOURNAL_SCHEMA_VERSION, JournalEnvelope, JournalReplay, JournalSequence,
    encode_record, replay,
};

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
    file: File,
    next_sequence: JournalSequence,
}

impl JournalWriter {
    pub fn open(path: impl AsRef<Path>) -> Result<(Self, JournalReplay), JournalError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| JournalError::new("failed to create journal directory", error))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|error| JournalError::new("failed to open journal", error))?;
        file.try_lock()
            .map_err(|error| JournalError::new("failed to lock journal", error.into()))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| JournalError::new("failed to seek journal", error))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| JournalError::new("failed to read journal", error))?;
        let replay = replay(&bytes);
        let writer = Self {
            path,
            file,
            next_sequence: replay.next_sequence,
        };
        Ok((writer, replay))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append_batch(
        &mut self,
        deltas: &[DurableDelta],
    ) -> Result<(Vec<JournalEnvelope>, DurabilityToken), JournalError> {
        if deltas.is_empty() {
            return Ok((Vec::new(), DurabilityToken::new()));
        }
        let envelope = JournalEnvelope {
            schema_version: JOURNAL_SCHEMA_VERSION,
            sequence: self.next_sequence,
            deltas: deltas.to_vec(),
        };
        let encoded = encode_record(&envelope).map_err(|error| {
            JournalError::new(
                "failed to encode journal batch",
                io::Error::new(io::ErrorKind::InvalidData, error),
            )
        })?;
        self.file
            .write_all(&encoded)
            .map_err(|error| JournalError::new("failed to append journal batch", error))?;
        self.file
            .sync_all()
            .map_err(|error| JournalError::new("failed to synchronize journal", error))?;
        self.next_sequence =
            JournalSequence(self.next_sequence.0.checked_add(1).ok_or_else(|| {
                JournalError::new(
                    "journal sequence overflow",
                    io::Error::new(io::ErrorKind::InvalidData, "sequence overflow"),
                )
            })?);
        Ok((vec![envelope], DurabilityToken::new()))
    }
}
