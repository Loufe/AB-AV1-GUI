use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{FileTimeNs, RunId};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct ContentKey(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum FileSystemId {
    Unix {
        #[specta(type = crate::JsNumber)]
        device: u64,
        #[specta(type = crate::JsNumber)]
        inode: u64,
    },
    WindowsLowResolution {
        volume_serial: u32,
        #[specta(type = crate::JsNumber)]
        file_index: u64,
    },
    WindowsHighResolution {
        #[specta(type = crate::JsNumber)]
        volume_serial: u64,
        // ReFS 128-bit file ids overflow a JavaScript number, so the wire
        // representation is a string; the frontend treats it as opaque.
        #[serde(with = "u128_string")]
        #[specta(type = String)]
        file_id: u128,
    },
}

mod u128_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse::<u128>().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DestructiveIdentity {
    pub file_id: FileSystemId,
    #[specta(type = crate::JsNumber)]
    pub size: u64,
    pub modified_ns: Option<FileTimeNs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ArtifactIdentity {
    pub content_key: ContentKey,
    pub destructive: DestructiveIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestructiveObservation {
    Absent,
    Present(DestructiveIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum Replacement {
    KeepOriginal,
    RetireOriginal,
}

/// Why an output transaction settled as a conflict. `IdentityMismatch` means
/// observed files no longer match the ledger; `InspectionFailed` means the
/// filesystem could not be inspected or acted on to find out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum ConflictKind {
    IdentityMismatch,
    InspectionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct OutputTransaction {
    pub run_id: RunId,
    pub input: PathBuf,
    pub input_identity: DestructiveIdentity,
    pub staging: PathBuf,
    pub final_path: PathBuf,
    pub final_preimage: Option<DestructiveIdentity>,
    pub replacement: Replacement,
    pub state: OutputState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum OutputState {
    Started,
    StagingCreated {
        initial: DestructiveIdentity,
    },
    Ready {
        staging_identity: ArtifactIdentity,
    },
    Committed {
        final_identity: ArtifactIdentity,
    },
    RetireIntent {
        final_identity: ArtifactIdentity,
    },
    Retired {
        final_identity: ArtifactIdentity,
    },
    AbandonIntent {
        staging_identity: DestructiveIdentity,
    },
    Abandoned,
    Conflict {
        kind: ConflictKind,
        detail: String,
    },
}

impl OutputTransaction {
    #[must_use]
    pub fn is_settled(&self) -> bool {
        matches!(
            (&self.replacement, &self.state),
            (_, OutputState::Conflict { .. })
                | (_, OutputState::Abandoned)
                | (Replacement::KeepOriginal, OutputState::Committed { .. })
                | (Replacement::RetireOriginal, OutputState::Retired { .. })
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum OutputDelta {
    OutputStarted {
        transaction: Box<OutputTransaction>,
    },
    /// Records the created (or, for an in-transaction encode retry,
    /// recreated) staging artifact. A repeated `StagingCreated` moves the
    /// journaled pin — which `OutputReady` verifies by file id — to the
    /// recreated file after the failed attempt's adapter cleanup deleted it.
    /// A settled transaction refuses this, so retry-after-abandonment stays
    /// unrepresentable.
    StagingCreated {
        run_id: RunId,
        initial: DestructiveIdentity,
    },
    OutputReady {
        run_id: RunId,
        staging_identity: ArtifactIdentity,
    },
    OutputCommitted {
        run_id: RunId,
        final_identity: ArtifactIdentity,
    },
    RetireOriginalIntent {
        run_id: RunId,
    },
    OriginalRetired {
        run_id: RunId,
    },
    AbandonStagingIntent {
        run_id: RunId,
        staging_identity: DestructiveIdentity,
    },
    Abandoned {
        run_id: RunId,
    },
    Conflict {
        run_id: RunId,
        kind: ConflictKind,
        detail: String,
    },
}

impl OutputDelta {
    pub(crate) fn fold_into(&self, outputs: &mut BTreeMap<RunId, OutputTransaction>) {
        match self {
            Self::OutputStarted { transaction } => {
                outputs.insert(transaction.run_id, transaction.as_ref().clone());
            }
            Self::StagingCreated { run_id, initial } => update_state(
                outputs,
                *run_id,
                OutputState::StagingCreated {
                    initial: initial.clone(),
                },
            ),
            Self::OutputReady {
                run_id,
                staging_identity,
            } => update_state(
                outputs,
                *run_id,
                OutputState::Ready {
                    staging_identity: staging_identity.clone(),
                },
            ),
            Self::OutputCommitted {
                run_id,
                final_identity,
            } => update_state(
                outputs,
                *run_id,
                OutputState::Committed {
                    final_identity: final_identity.clone(),
                },
            ),
            Self::RetireOriginalIntent { run_id } => {
                let final_identity = outputs.get(run_id).and_then(|transaction| {
                    if let OutputState::Committed { final_identity } = &transaction.state {
                        Some(final_identity.clone())
                    } else {
                        None
                    }
                });
                if let Some(final_identity) = final_identity {
                    update_state(
                        outputs,
                        *run_id,
                        OutputState::RetireIntent { final_identity },
                    );
                }
            }
            Self::OriginalRetired { run_id } => {
                let final_identity = outputs
                    .get(run_id)
                    .and_then(|transaction| match &transaction.state {
                        OutputState::RetireIntent { final_identity } => {
                            Some(final_identity.clone())
                        }
                        _ => None,
                    });
                if let Some(final_identity) = final_identity {
                    update_state(outputs, *run_id, OutputState::Retired { final_identity });
                }
            }
            Self::AbandonStagingIntent {
                run_id,
                staging_identity,
            } => update_state(
                outputs,
                *run_id,
                OutputState::AbandonIntent {
                    staging_identity: staging_identity.clone(),
                },
            ),
            Self::Abandoned { run_id } => update_state(outputs, *run_id, OutputState::Abandoned),
            Self::Conflict {
                run_id,
                kind,
                detail,
            } => update_state(
                outputs,
                *run_id,
                OutputState::Conflict {
                    kind: *kind,
                    detail: detail.clone(),
                },
            ),
        }
    }
}

fn update_state(
    outputs: &mut BTreeMap<RunId, OutputTransaction>,
    run_id: RunId,
    state: OutputState,
) {
    if let Some(transaction) = outputs.get_mut(&run_id) {
        transaction.state = state;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSystemFacts {
    pub staging: DestructiveObservation,
    pub final_path: DestructiveObservation,
    pub original: DestructiveObservation,
    pub staging_artifact: Option<ArtifactIdentity>,
    pub final_artifact: Option<ArtifactIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryConflict {
    pub kind: ConflictKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRecoveryAction {
    None,
    Append(OutputDelta),
    DeleteStaging {
        path: PathBuf,
        expected: DestructiveIdentity,
    },
    Promote {
        staging: PathBuf,
        final_path: PathBuf,
        expected_staging: DestructiveIdentity,
        expected_content: ContentKey,
        expected_final: Option<DestructiveIdentity>,
    },
    DeleteOriginal {
        path: PathBuf,
        expected_original: DestructiveIdentity,
        expected_final: DestructiveIdentity,
    },
    Conflict(RecoveryConflict),
}

#[must_use]
pub fn recover_output(
    transaction: &OutputTransaction,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match &transaction.state {
        OutputState::Started => recover_started(transaction, facts),
        OutputState::StagingCreated { initial } => {
            recover_staging_created(transaction, initial, facts)
        }
        OutputState::Ready { staging_identity } => {
            recover_ready(transaction, staging_identity, facts)
        }
        OutputState::Committed { final_identity } => {
            recover_committed(transaction, final_identity, facts)
        }
        OutputState::RetireIntent { final_identity } => {
            recover_retire_intent(transaction, final_identity, facts)
        }
        OutputState::AbandonIntent { staging_identity } => {
            recover_abandon_intent(transaction, staging_identity, facts)
        }
        OutputState::Retired { .. } | OutputState::Abandoned | OutputState::Conflict { .. } => {
            OutputRecoveryAction::None
        }
    }
}

fn recover_abandon_intent(
    transaction: &OutputTransaction,
    staging_identity: &DestructiveIdentity,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match &facts.staging {
        DestructiveObservation::Absent => OutputRecoveryAction::Append(OutputDelta::Abandoned {
            run_id: transaction.run_id,
        }),
        DestructiveObservation::Present(actual) if actual == staging_identity => {
            OutputRecoveryAction::DeleteStaging {
                path: transaction.staging.clone(),
                expected: staging_identity.clone(),
            }
        }
        DestructiveObservation::Present(_) => conflict("staging changed after abandonment intent"),
    }
}

fn recover_started(
    transaction: &OutputTransaction,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match &facts.staging {
        DestructiveObservation::Absent => OutputRecoveryAction::Append(OutputDelta::Abandoned {
            run_id: transaction.run_id,
        }),
        // A present staging file in `Started` means the crash landed between
        // file creation and the `StagingCreated` journal record, so no identity
        // was ever recorded. Ownership rests on the staging filename embedding
        // this run id: run ids are unique within a journal, so the file is ours
        // to abandon under the identity observed now.
        DestructiveObservation::Present(identity) => {
            OutputRecoveryAction::Append(OutputDelta::AbandonStagingIntent {
                run_id: transaction.run_id,
                staging_identity: identity.clone(),
            })
        }
    }
}

fn recover_staging_created(
    transaction: &OutputTransaction,
    initial: &DestructiveIdentity,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match &facts.staging {
        DestructiveObservation::Absent => OutputRecoveryAction::Append(OutputDelta::Abandoned {
            run_id: transaction.run_id,
        }),
        DestructiveObservation::Present(identity) if identity.file_id == initial.file_id => {
            OutputRecoveryAction::Append(OutputDelta::AbandonStagingIntent {
                run_id: transaction.run_id,
                staging_identity: identity.clone(),
            })
        }
        DestructiveObservation::Present(_) => conflict("uncommitted staging identity changed"),
    }
}

fn recover_ready(
    transaction: &OutputTransaction,
    staging_identity: &ArtifactIdentity,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match (&facts.staging, &facts.final_path, &facts.final_artifact) {
        (
            DestructiveObservation::Absent,
            DestructiveObservation::Present(final_destructive),
            Some(final_identity),
        ) if final_identity.content_key == staging_identity.content_key
            && &final_identity.destructive == final_destructive =>
        {
            OutputRecoveryAction::Append(OutputDelta::OutputCommitted {
                run_id: transaction.run_id,
                final_identity: final_identity.clone(),
            })
        }
        (DestructiveObservation::Present(staging), final_observation, _)
            if staging == &staging_identity.destructive
                && facts.staging_artifact.as_ref() == Some(staging_identity)
                && observation_matches_preimage(final_observation, &transaction.final_preimage) =>
        {
            OutputRecoveryAction::Promote {
                staging: transaction.staging.clone(),
                final_path: transaction.final_path.clone(),
                expected_staging: staging_identity.destructive.clone(),
                expected_content: staging_identity.content_key.clone(),
                expected_final: transaction.final_preimage.clone(),
            }
        }
        _ => conflict("output files do not match the recorded ready state"),
    }
}

fn recover_committed(
    transaction: &OutputTransaction,
    final_identity: &ArtifactIdentity,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    if facts.final_path != DestructiveObservation::Present(final_identity.destructive.clone())
        || facts.final_artifact.as_ref() != Some(final_identity)
    {
        return conflict("committed output identity changed");
    }
    match transaction.replacement {
        Replacement::KeepOriginal => OutputRecoveryAction::None,
        Replacement::RetireOriginal => {
            OutputRecoveryAction::Append(OutputDelta::RetireOriginalIntent {
                run_id: transaction.run_id,
            })
        }
    }
}

fn recover_retire_intent(
    transaction: &OutputTransaction,
    final_identity: &ArtifactIdentity,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    if facts.final_path != DestructiveObservation::Present(final_identity.destructive.clone())
        || facts.final_artifact.as_ref() != Some(final_identity)
    {
        return conflict("output changed before original retirement");
    }
    match &facts.original {
        DestructiveObservation::Absent => {
            OutputRecoveryAction::Append(OutputDelta::OriginalRetired {
                run_id: transaction.run_id,
            })
        }
        DestructiveObservation::Present(original) if original == &transaction.input_identity => {
            OutputRecoveryAction::DeleteOriginal {
                path: transaction.input.clone(),
                expected_original: original.clone(),
                expected_final: final_identity.destructive.clone(),
            }
        }
        DestructiveObservation::Present(_) => conflict("original changed before retirement"),
    }
}

fn observation_matches_preimage(
    observation: &DestructiveObservation,
    preimage: &Option<DestructiveIdentity>,
) -> bool {
    match (observation, preimage) {
        (DestructiveObservation::Absent, None) => true,
        (DestructiveObservation::Present(actual), Some(expected)) => actual == expected,
        _ => false,
    }
}

fn conflict(detail: &str) -> OutputRecoveryAction {
    // Every conflict recover_output itself detects is an identity mismatch:
    // the observed files no longer match the ledger. Inspection failures are
    // reported by the engine, which cannot see this far.
    OutputRecoveryAction::Conflict(RecoveryConflict {
        kind: ConflictKind::IdentityMismatch,
        detail: detail.to_owned(),
    })
}
