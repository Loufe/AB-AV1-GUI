use std::{
    ffi::OsString,
    fmt,
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
};

use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::fs::File;

use crfty_core::{
    ArtifactIdentity, ArtifactObservation, FileSystemFacts, OutputDelta, OutputRecoveryAction,
    OutputState, OutputTransaction, Replacement, RunId, recover_output,
};

use crfty_core::ContentKey;
use serde::Deserialize;

use crate::blake2b::Blake2b128;

const CONTENT_KEY_SCHEMA: &[u8] = b"ck1\0";
const CONTENT_KEY_TEXT_PREFIX: &str = "ck1:";
const HEX_CHARACTERS_PER_BYTE: usize = 2;
const SAMPLE_ALIGNMENT_BYTES: u64 = 4 * 1024;
const WHOLE_FILE_LIMIT: u64 = 4 * 1024 * 1024;
const EDGE_SAMPLE: usize = 256 * 1024;
const MIDDLE_SAMPLE: usize = 64 * 1024;
const MIN_VERIFIED_OUTPUT_SIZE: u64 = 1024;
const QUARTER_SAMPLE_NUMERATORS: [u64; 3] = [1, 2, 3];
const SAMPLE_QUARTERS: u64 = 4;
const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

pub trait ArtifactInspector {
    fn inspect(&self, path: &Path) -> io::Result<ArtifactIdentity>;

    fn verify_output(&self, path: &Path) -> io::Result<ArtifactIdentity>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentSyncSupport {
    Supported,
    Unsupported,
}

#[must_use]
pub const fn parent_sync_support() -> ParentSyncSupport {
    if cfg!(unix) {
        ParentSyncSupport::Supported
    } else {
        ParentSyncSupport::Unsupported
    }
}

#[derive(Debug)]
pub struct OutputError {
    context: &'static str,
    source: io::Error,
}

impl OutputError {
    fn new(context: &'static str, source: io::Error) -> Self {
        Self { context, source }
    }
}

impl fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub struct OutputManager<I> {
    inspector: I,
}

impl<I: ArtifactInspector> OutputManager<I> {
    pub fn new(inspector: I) -> Self {
        Self { inspector }
    }

    pub fn prepare(
        &self,
        run_id: RunId,
        input: &Path,
        final_path: &Path,
        replacement: Replacement,
    ) -> Result<OutputTransaction, OutputError> {
        if replacement == Replacement::RetireOriginal && input == final_path {
            return Err(OutputError::new(
                "same-path replacement cannot retire the original separately",
                io::Error::new(io::ErrorKind::InvalidInput, "invalid replacement mode"),
            ));
        }
        let input_identity = self
            .inspector
            .inspect(input)
            .map_err(|error| OutputError::new("failed to inspect input", error))?;
        let final_preimage = observe(&self.inspector, final_path)
            .map_err(|error| OutputError::new("failed to inspect output destination", error))?
            .into_identity();
        let staging = staging_path(final_path, run_id)?;
        let staging_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| {
                OutputError::new("failed to create staging file exclusively", error)
            })?;
        staging_file
            .sync_all()
            .map_err(|error| OutputError::new("failed to synchronize new staging file", error))?;
        drop(staging_file);
        let initial_staging_identity = self
            .inspector
            .inspect(&staging)
            .map_err(|error| OutputError::new("failed to inspect new staging file", error))?;
        Ok(OutputTransaction {
            run_id,
            input: input.to_path_buf(),
            input_identity,
            staging,
            initial_staging_identity,
            final_path: final_path.to_path_buf(),
            final_preimage,
            replacement,
            state: OutputState::Started,
        })
    }

    pub fn mark_ready(&self, transaction: &OutputTransaction) -> Result<OutputDelta, OutputError> {
        if transaction.state != OutputState::Started {
            return Err(OutputError::new(
                "output transaction is not in started state",
                io::Error::new(io::ErrorKind::InvalidInput, "invalid output state"),
            ));
        }
        let staging = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&transaction.staging)
            .map_err(|error| OutputError::new("failed to open staging file", error))?;
        staging
            .sync_all()
            .map_err(|error| OutputError::new("failed to synchronize staging output", error))?;
        let staging_identity = self
            .inspector
            .verify_output(&transaction.staging)
            .map_err(|error| OutputError::new("staging output verification failed", error))?;
        Ok(OutputDelta::OutputReady {
            run_id: transaction.run_id,
            staging_identity,
        })
    }

    pub fn discard_unjournaled(&self, transaction: &OutputTransaction) -> Result<(), OutputError> {
        if transaction.state != OutputState::Started {
            return Err(OutputError::new(
                "unjournaled output is not in started state",
                io::Error::new(io::ErrorKind::InvalidInput, "invalid output state"),
            ));
        }
        require_identity(
            &self.inspector,
            &transaction.staging,
            &transaction.initial_staging_identity,
            "new staging file changed before journal acknowledgement",
        )?;
        std::fs::remove_file(&transaction.staging).map_err(|error| {
            OutputError::new("failed to remove unjournaled staging file", error)
        })?;
        sync_parent(&transaction.staging)
    }

    pub fn abandon_intent(
        &self,
        transaction: &OutputTransaction,
    ) -> Result<OutputDelta, OutputError> {
        if transaction.state != OutputState::Started {
            return Err(OutputError::new(
                "output transaction cannot be abandoned from its current state",
                io::Error::new(io::ErrorKind::InvalidInput, "invalid output state"),
            ));
        }
        let staging_identity = self
            .inspector
            .inspect(&transaction.staging)
            .map_err(|error| OutputError::new("failed to inspect abandoned staging", error))?;
        Ok(OutputDelta::AbandonStagingIntent {
            run_id: transaction.run_id,
            staging_identity,
        })
    }

    pub fn facts(&self, transaction: &OutputTransaction) -> Result<FileSystemFacts, OutputError> {
        Ok(FileSystemFacts {
            staging: observe(&self.inspector, &transaction.staging)
                .map_err(|error| OutputError::new("failed to inspect staging path", error))?,
            final_path: observe(&self.inspector, &transaction.final_path)
                .map_err(|error| OutputError::new("failed to inspect final path", error))?,
            original: observe(&self.inspector, &transaction.input)
                .map_err(|error| OutputError::new("failed to inspect original path", error))?,
        })
    }

    pub fn recover_once(
        &self,
        transaction: &OutputTransaction,
    ) -> Result<Option<OutputDelta>, OutputError> {
        let facts = self.facts(transaction)?;
        let action = recover_output(transaction, &facts);
        self.execute(transaction, action)
    }

    fn execute(
        &self,
        transaction: &OutputTransaction,
        action: OutputRecoveryAction,
    ) -> Result<Option<OutputDelta>, OutputError> {
        match action {
            OutputRecoveryAction::None => Ok(None),
            OutputRecoveryAction::Append(delta) => Ok(Some(delta)),
            OutputRecoveryAction::Conflict(conflict) => Ok(Some(OutputDelta::Conflict {
                run_id: transaction.run_id,
                reason: conflict.reason,
            })),
            OutputRecoveryAction::DeleteStaging { path, expected } => {
                require_identity(&self.inspector, &path, &expected, "staging file changed")?;
                std::fs::remove_file(&path).map_err(|error| {
                    OutputError::new("failed to remove abandoned staging file", error)
                })?;
                sync_parent(&path)?;
                Ok(Some(OutputDelta::Abandoned {
                    run_id: transaction.run_id,
                }))
            }
            OutputRecoveryAction::Promote {
                staging,
                final_path,
                expected_staging,
                expected_final,
            } => {
                require_identity(
                    &self.inspector,
                    &staging,
                    &expected_staging,
                    "staging file changed before promotion",
                )?;
                require_observation(
                    &self.inspector,
                    &final_path,
                    expected_final.as_ref(),
                    "destination changed before promotion",
                )?;
                std::fs::rename(&staging, &final_path)
                    .map_err(|error| OutputError::new("failed to promote staging output", error))?;
                sync_parent(&final_path)?;
                let final_identity =
                    self.inspector.verify_output(&final_path).map_err(|error| {
                        OutputError::new("promoted output verification failed", error)
                    })?;
                if final_identity.content_key != expected_staging.content_key
                    || final_identity.size != expected_staging.size
                {
                    return Err(OutputError::new(
                        "promoted output identity differs from staging",
                        io::Error::new(io::ErrorKind::InvalidData, "content identity changed"),
                    ));
                }
                Ok(Some(OutputDelta::OutputCommitted {
                    run_id: transaction.run_id,
                    final_identity,
                }))
            }
            OutputRecoveryAction::DeleteOriginal {
                path,
                expected_original,
                expected_final,
            } => {
                require_identity(
                    &self.inspector,
                    &transaction.final_path,
                    &expected_final,
                    "output changed before original retirement",
                )?;
                require_identity(
                    &self.inspector,
                    &path,
                    &expected_original,
                    "original changed before retirement",
                )?;
                std::fs::remove_file(&path)
                    .map_err(|error| OutputError::new("failed to retire original", error))?;
                sync_parent(&path)?;
                Ok(Some(OutputDelta::OriginalRetired {
                    run_id: transaction.run_id,
                }))
            }
        }
    }
}

trait ObservationExt {
    fn into_identity(self) -> Option<ArtifactIdentity>;
}

impl ObservationExt for ArtifactObservation {
    fn into_identity(self) -> Option<ArtifactIdentity> {
        match self {
            Self::Absent => None,
            Self::Present(identity) => Some(identity),
        }
    }
}

fn observe<I: ArtifactInspector>(inspector: &I, path: &Path) -> io::Result<ArtifactObservation> {
    match inspector.inspect(path) {
        Ok(identity) => Ok(ArtifactObservation::Present(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ArtifactObservation::Absent),
        Err(error) => Err(error),
    }
}

fn require_identity<I: ArtifactInspector>(
    inspector: &I,
    path: &Path,
    expected: &ArtifactIdentity,
    message: &'static str,
) -> Result<(), OutputError> {
    require_observation(inspector, path, Some(expected), message)
}

fn require_observation<I: ArtifactInspector>(
    inspector: &I,
    path: &Path,
    expected: Option<&ArtifactIdentity>,
    message: &'static str,
) -> Result<(), OutputError> {
    let actual = observe(inspector, path)
        .map_err(|error| OutputError::new("failed to revalidate filesystem path", error))?;
    let matches = match (actual, expected) {
        (ArtifactObservation::Absent, None) => true,
        (ArtifactObservation::Present(actual), Some(expected)) => &actual == expected,
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(OutputError::new(
            message,
            io::Error::new(io::ErrorKind::InvalidData, "artifact identity mismatch"),
        ))
    }
}

fn staging_path(final_path: &Path, run_id: RunId) -> Result<PathBuf, OutputError> {
    let Some(file_name) = final_path.file_name() else {
        return Err(OutputError::new(
            "final path has no file name",
            io::Error::new(io::ErrorKind::InvalidInput, "missing file name"),
        ));
    };
    let mut staging_name = OsString::from(".");
    staging_name.push(file_name);
    staging_name.push(format!(".crfty-{}.part", run_id.0));
    Ok(final_path.with_file_name(staging_name))
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), OutputError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| OutputError::new("failed to synchronize parent directory", error))?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<(), OutputError> {
    Ok(())
}

/// Deterministic byte inspector for tests; not a production media verifier.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixtureByteInspector;

impl ArtifactInspector for FixtureByteInspector {
    fn inspect(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        byte_identity(path)
    }

    fn verify_output(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        let identity = byte_identity(path)?;
        if identity.size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "output is empty",
            ));
        }
        Ok(identity)
    }
}

fn byte_identity(path: &Path) -> io::Result<ArtifactIdentity> {
    let bytes = std::fs::read(path)?;
    let metadata = std::fs::metadata(path)?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in &bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    Ok(ArtifactIdentity {
        content_key: ContentKey(format!("fnv1a64:{hash:016x}")),
        size: metadata.len(),
        modified_ns,
        file_id: platform_file_id(&metadata),
    })
}

#[derive(Debug, Clone)]
pub struct MediaArtifactInspector {
    ffprobe: PathBuf,
}

impl MediaArtifactInspector {
    pub fn new(ffprobe: PathBuf) -> Self {
        Self { ffprobe }
    }

    fn probe(&self, path: &Path) -> io::Result<MediaHeader> {
        let output = Command::new(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,width,height:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ffprobe rejected media artifact",
            ));
        }
        let probe: ProbeDocument = serde_json::from_slice(&output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let stream = probe.streams.into_iter().next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "media has no video stream")
        })?;
        let duration = probe
            .format
            .and_then(|format| format.duration)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "duration is missing"))?
            .parse::<f64>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if !duration.is_finite() || duration <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duration is not positive and finite",
            ));
        }
        Ok(MediaHeader {
            codec: stream.codec_name,
            width: stream.width,
            height: stream.height,
            duration_ms: (duration * MILLISECONDS_PER_SECOND).round() as u64,
        })
    }
}

impl ArtifactInspector for MediaArtifactInspector {
    fn inspect(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        let metadata = std::fs::metadata(path)?;
        let header = if metadata.len() == 0 {
            MediaHeader::default()
        } else {
            self.probe(path)?
        };
        sampled_identity(path, &header)
    }

    fn verify_output(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        let metadata = std::fs::metadata(path)?;
        if metadata.len() < MIN_VERIFIED_OUTPUT_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "output is too small to be a valid video",
            ));
        }
        let header = self.probe(path)?;
        if !header.codec.eq_ignore_ascii_case("av1") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "output video codec is not AV1",
            ));
        }
        sampled_identity(path, &header)
    }
}

#[derive(Debug, Default)]
struct MediaHeader {
    codec: String,
    width: u32,
    height: u32,
    duration_ms: u64,
}

#[derive(Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

fn sampled_identity(path: &Path, header: &MediaHeader) -> io::Result<ArtifactIdentity> {
    let before = std::fs::metadata(path)?;
    let mut file = std::fs::File::open(path)?;
    let mut digest = Blake2b128::new();
    digest.update(CONTENT_KEY_SCHEMA);
    digest.update(&before.len().to_le_bytes());
    digest.update(&header.duration_ms.to_le_bytes());
    digest.update(&header.width.to_le_bytes());
    digest.update(&header.height.to_le_bytes());
    digest.update(&(header.codec.len() as u64).to_le_bytes());
    digest.update(header.codec.as_bytes());

    if before.len() <= WHOLE_FILE_LIMIT {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        digest.update(&bytes);
    } else {
        hash_region(&mut digest, &mut file, 0, EDGE_SAMPLE)?;
        for numerator in QUARTER_SAMPLE_NUMERATORS {
            let raw = before.len().saturating_mul(numerator) / SAMPLE_QUARTERS;
            let offset = raw / SAMPLE_ALIGNMENT_BYTES * SAMPLE_ALIGNMENT_BYTES;
            hash_region(&mut digest, &mut file, offset, MIDDLE_SAMPLE)?;
        }
        hash_region(
            &mut digest,
            &mut file,
            before.len().saturating_sub(EDGE_SAMPLE as u64),
            EDGE_SAMPLE,
        )?;
    }
    let after = std::fs::metadata(path)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "artifact changed while its identity was computed",
        ));
    }
    let bytes = digest.finalize();
    let mut encoded = String::with_capacity(
        CONTENT_KEY_TEXT_PREFIX.len() + bytes.len() * HEX_CHARACTERS_PER_BYTE,
    );
    encoded.push_str(CONTENT_KEY_TEXT_PREFIX);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").map_err(|error| io::Error::other(error.to_string()))?;
    }
    let modified_ns = after
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    Ok(ArtifactIdentity {
        content_key: ContentKey(encoded),
        size: after.len(),
        modified_ns,
        file_id: platform_file_id(&after),
    })
}

fn hash_region(
    digest: &mut Blake2b128,
    file: &mut std::fs::File,
    offset: u64,
    length: usize,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0_u8; length];
    let read = file.read(&mut bytes)?;
    digest.update(&offset.to_le_bytes());
    digest.update(&(read as u64).to_le_bytes());
    digest.update(bytes.get(..read).unwrap_or_default());
    Ok(())
}

#[cfg(unix)]
fn platform_file_id(metadata: &std::fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn platform_file_id(_metadata: &std::fs::Metadata) -> Option<String> {
    None
}
