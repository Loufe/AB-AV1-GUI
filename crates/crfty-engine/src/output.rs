use std::{
    ffi::OsString,
    fmt,
    fs::OpenOptions,
    io,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::fs::File;

use crfty_core::{
    ArtifactIdentity, ContentKey, DestructiveIdentity, DestructiveObservation, FileSystemFacts,
    OutputDelta, OutputRecoveryAction, OutputState, OutputTransaction, Replacement, RunId,
    recover_output,
};

const MIN_VERIFIED_OUTPUT_SIZE: u64 = 1024;

use crate::media::{MediaInspector, destructive_identity};

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

    #[must_use]
    pub fn is_destination_exists(&self) -> bool {
        self.context == "output destination already exists"
            && self.source.kind() == io::ErrorKind::AlreadyExists
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
        overwrite_existing: bool,
    ) -> Result<OutputTransaction, OutputError> {
        let input_identity = self
            .inspector
            .inspect_file(input)
            .map_err(|error| OutputError::new("failed to inspect input", error))?;
        let final_preimage = observe_file(&self.inspector, final_path)
            .map_err(|error| OutputError::new("failed to inspect output destination", error))?
            .into_identity();
        if !overwrite_existing && final_path != input && final_preimage.is_some() {
            return Err(OutputError::new(
                "output destination already exists",
                io::Error::new(io::ErrorKind::AlreadyExists, "overwrite is disabled"),
            ));
        }
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
    media: MediaInspector,
}

impl MediaArtifactInspector {
    pub fn new(ffprobe: PathBuf) -> Self {
        Self {
            media: MediaInspector::new(ffprobe),
        }
    }
}

impl ArtifactInspector for MediaArtifactInspector {
    fn inspect_file(&self, path: &Path) -> io::Result<DestructiveIdentity> {
        destructive_identity(path)
    }

    fn inspect_media(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        self.media.inspect_artifact(path)
    }

    fn verify_output(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        self.media.verify_av1(path, MIN_VERIFIED_OUTPUT_SIZE)
    }
}
