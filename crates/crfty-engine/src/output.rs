use std::{
    ffi::OsString,
    fmt,
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
};

use blake2::{
    Blake2bVar,
    digest::{Update, VariableOutput},
};
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::fs::File;

use crfty_core::{
    ArtifactIdentity, DestructiveIdentity, DestructiveObservation, FileSystemFacts, FileSystemId,
    OutputDelta, OutputRecoveryAction, OutputState, OutputTransaction, Replacement, RunId,
    recover_output,
};

use crfty_core::ContentKey;
use serde::Deserialize;

const CONTENT_KEY_SCHEMA: &[u8] = b"ck1";
const CONTENT_KEY_TEXT_PREFIX: &str = "ck1:";
const CONTENT_KEY_DIGEST_BYTES: usize = 16;
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
    fn inspect_file(&self, path: &Path) -> io::Result<DestructiveIdentity>;

    fn inspect_media(&self, path: &Path) -> io::Result<ArtifactIdentity>;

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
        let input_identity = self
            .inspector
            .inspect_file(input)
            .map_err(|error| OutputError::new("failed to inspect input", error))?;
        let final_preimage = observe_file(&self.inspector, final_path)
            .map_err(|error| OutputError::new("failed to inspect output destination", error))?
            .into_identity();
        if replacement == Replacement::RetireOriginal
            && final_preimage.as_ref() == Some(&input_identity)
        {
            return Err(OutputError::new(
                "same-file replacement cannot retire the original separately",
                io::Error::new(io::ErrorKind::InvalidInput, "invalid replacement mode"),
            ));
        }
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
            .inspect_file(&staging)
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
            .inspect_file(&transaction.staging)
            .map_err(|error| OutputError::new("failed to inspect abandoned staging", error))?;
        Ok(OutputDelta::AbandonStagingIntent {
            run_id: transaction.run_id,
            staging_identity,
        })
    }

    pub fn facts(&self, transaction: &OutputTransaction) -> Result<FileSystemFacts, OutputError> {
        let staging = observe_file(&self.inspector, &transaction.staging)
            .map_err(|error| OutputError::new("failed to inspect staging path", error))?;
        let final_path = observe_file(&self.inspector, &transaction.final_path)
            .map_err(|error| OutputError::new("failed to inspect final path", error))?;
        let needs_staging_artifact = matches!(transaction.state, OutputState::Ready { .. });
        let needs_final_artifact = matches!(
            transaction.state,
            OutputState::Ready { .. }
                | OutputState::Committed { .. }
                | OutputState::RetireIntent { .. }
                | OutputState::Retired { .. }
        );
        Ok(FileSystemFacts {
            staging_artifact: inspect_present_media(
                &self.inspector,
                &transaction.staging,
                &staging,
                needs_staging_artifact,
            )
            .map_err(|error| OutputError::new("failed to verify staging artifact", error))?,
            final_artifact: inspect_present_media(
                &self.inspector,
                &transaction.final_path,
                &final_path,
                needs_final_artifact,
            )
            .map_err(|error| OutputError::new("failed to verify final artifact", error))?,
            staging,
            final_path,
            original: observe_file(&self.inspector, &transaction.input)
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
                expected_content,
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
                if final_identity.content_key != expected_content
                    || final_identity.destructive.size != expected_staging.size
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
    fn into_identity(self) -> Option<DestructiveIdentity>;
}

impl ObservationExt for DestructiveObservation {
    fn into_identity(self) -> Option<DestructiveIdentity> {
        match self {
            Self::Absent => None,
            Self::Present(identity) => Some(identity),
        }
    }
}

fn observe_file<I: ArtifactInspector>(
    inspector: &I,
    path: &Path,
) -> io::Result<DestructiveObservation> {
    match inspector.inspect_file(path) {
        Ok(identity) => Ok(DestructiveObservation::Present(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DestructiveObservation::Absent),
        Err(error) => Err(error),
    }
}

fn inspect_present_media<I: ArtifactInspector>(
    inspector: &I,
    path: &Path,
    observation: &DestructiveObservation,
    required: bool,
) -> io::Result<Option<ArtifactIdentity>> {
    if required && matches!(observation, DestructiveObservation::Present(_)) {
        inspector.inspect_media(path).map(Some)
    } else {
        Ok(None)
    }
}

fn require_identity<I: ArtifactInspector>(
    inspector: &I,
    path: &Path,
    expected: &DestructiveIdentity,
    message: &'static str,
) -> Result<(), OutputError> {
    require_observation(inspector, path, Some(expected), message)
}

fn require_observation<I: ArtifactInspector>(
    inspector: &I,
    path: &Path,
    expected: Option<&DestructiveIdentity>,
    message: &'static str,
) -> Result<(), OutputError> {
    let actual = observe_file(inspector, path)
        .map_err(|error| OutputError::new("failed to revalidate filesystem path", error))?;
    let matches = match (actual, expected) {
        (DestructiveObservation::Absent, None) => true,
        (DestructiveObservation::Present(actual), Some(expected)) => &actual == expected,
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
    fn inspect_file(&self, path: &Path) -> io::Result<DestructiveIdentity> {
        destructive_identity(path)
    }

    fn inspect_media(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        byte_artifact_identity(path)
    }

    fn verify_output(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        let identity = byte_artifact_identity(path)?;
        if identity.destructive.size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "output is empty",
            ));
        }
        Ok(identity)
    }
}

fn byte_artifact_identity(path: &Path) -> io::Result<ArtifactIdentity> {
    let bytes = std::fs::read(path)?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in &bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(ArtifactIdentity {
        content_key: ContentKey(format!("fnv1a64:{hash:016x}")),
        destructive: destructive_identity(path)?,
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
    fn inspect_file(&self, path: &Path) -> io::Result<DestructiveIdentity> {
        destructive_identity(path)
    }

    fn inspect_media(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        let header = self.probe(path)?;
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
    let before_identity = identity_from_metadata(path, &before)?;
    let mut file = std::fs::File::open(path)?;
    let mut digest = Blake2bVar::new(CONTENT_KEY_DIGEST_BYTES)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    digest.update(CONTENT_KEY_SCHEMA);
    digest.update(&before.len().to_le_bytes());
    digest.update(&header.duration_ms.to_le_bytes());
    digest.update(&(header.codec.len() as u64).to_le_bytes());
    digest.update(header.codec.as_bytes());
    digest.update(&header.width.to_le_bytes());
    digest.update(&header.height.to_le_bytes());

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
    let after_identity = identity_from_metadata(path, &after)?;
    if before_identity != after_identity {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "artifact changed while its identity was computed",
        ));
    }
    let mut bytes = [0_u8; CONTENT_KEY_DIGEST_BYTES];
    digest
        .finalize_variable(&mut bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut encoded = String::with_capacity(
        CONTENT_KEY_TEXT_PREFIX.len() + bytes.len() * HEX_CHARACTERS_PER_BYTE,
    );
    encoded.push_str(CONTENT_KEY_TEXT_PREFIX);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").map_err(|error| io::Error::other(error.to_string()))?;
    }
    Ok(ArtifactIdentity {
        content_key: ContentKey(encoded),
        destructive: after_identity,
    })
}

fn hash_region(
    digest: &mut Blake2bVar,
    file: &mut std::fs::File,
    offset: u64,
    length: usize,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)?;
    digest.update(&bytes);
    Ok(())
}

fn destructive_identity(path: &Path) -> io::Result<DestructiveIdentity> {
    let metadata = std::fs::metadata(path)?;
    identity_from_metadata(path, &metadata)
}

fn identity_from_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> io::Result<DestructiveIdentity> {
    let file_id = match file_id::get_file_id(path)? {
        file_id::FileId::Inode {
            device_id,
            inode_number,
        } => FileSystemId::Unix {
            device: device_id,
            inode: inode_number,
        },
        file_id::FileId::LowRes {
            volume_serial_number,
            file_index,
        } => FileSystemId::WindowsLowResolution {
            volume_serial: volume_serial_number,
            file_index,
        },
        file_id::FileId::HighRes {
            volume_serial_number,
            file_id,
        } => FileSystemId::WindowsHighResolution {
            volume_serial: volume_serial_number,
            file_id,
        },
    };
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    Ok(DestructiveIdentity {
        file_id,
        size: metadata.len(),
        modified_ns,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{MediaHeader, sampled_identity};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn ck1_matches_independent_golden_fixtures() {
        let directory = test_directory("ck1-golden");
        let small = directory.join("small.bin");
        fs::write(&small, b"test").expect("small fixture");
        let small_identity = sampled_identity(
            &small,
            &MediaHeader {
                codec: "av1".to_owned(),
                width: 16,
                height: 9,
                duration_ms: 1_000,
            },
        )
        .expect("small content key");
        assert_eq!(
            small_identity.content_key.0,
            "ck1:23ba2a7ea690c5618f0d5e2ef5413c3e"
        );

        let large = directory.join("large.bin");
        let bytes: Vec<_> = (0..(4 * 1024 * 1024 + 12_345))
            .map(|index| u8::try_from(index % 251).expect("bounded byte"))
            .collect();
        fs::write(&large, bytes).expect("large fixture");
        let large_identity = sampled_identity(
            &large,
            &MediaHeader {
                codec: "h264".to_owned(),
                width: 1_920,
                height: 1_080,
                duration_ms: 98_765,
            },
        )
        .expect("large content key");
        assert_eq!(
            large_identity.content_key.0,
            "ck1:58736d4906c208fb16d7e4e3febba397"
        );
        fs::remove_dir_all(directory).expect("remove fixture directory");
    }

    fn test_directory(label: &str) -> std::path::PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crfty-{label}-{}-{sequence}", std::process::id()));
        fs::create_dir(&path).expect("fixture directory");
        path
    }
}
