use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::RunId;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContentKey(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIdentity {
    pub content_key: ContentKey,
    pub size: u64,
    pub modified_ns: Option<u128>,
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactObservation {
    Absent,
    Present(ArtifactIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Replacement {
    KeepOriginal,
    RetireOriginal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTransaction {
    pub run_id: RunId,
    pub input: PathBuf,
    pub input_identity: ArtifactIdentity,
    pub staging: PathBuf,
    pub initial_staging_identity: ArtifactIdentity,
    pub final_path: PathBuf,
    pub final_preimage: Option<ArtifactIdentity>,
    pub replacement: Replacement,
    pub state: OutputState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputState {
    Started,
    Ready { staging_identity: ArtifactIdentity },
    Committed { final_identity: ArtifactIdentity },
    RetireIntent { final_identity: ArtifactIdentity },
    Retired { final_identity: ArtifactIdentity },
    AbandonIntent { staging_identity: ArtifactIdentity },
    Abandoned,
    Conflict { reason: String },
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputDelta {
    EncodeStarted {
        transaction: Box<OutputTransaction>,
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
        staging_identity: ArtifactIdentity,
    },
    Abandoned {
        run_id: RunId,
    },
    Conflict {
        run_id: RunId,
        reason: String,
    },
}

impl OutputDelta {
    pub(crate) fn fold_into(&self, outputs: &mut BTreeMap<RunId, OutputTransaction>) {
        match self {
            Self::EncodeStarted { transaction } => {
                outputs.insert(transaction.run_id, transaction.as_ref().clone());
            }
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
            Self::Conflict { run_id, reason } => update_state(
                outputs,
                *run_id,
                OutputState::Conflict {
                    reason: reason.clone(),
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
    pub staging: ArtifactObservation,
    pub final_path: ArtifactObservation,
    pub original: ArtifactObservation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryConflict {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRecoveryAction {
    None,
    Append(OutputDelta),
    DeleteStaging {
        path: PathBuf,
        expected: ArtifactIdentity,
    },
    Promote {
        staging: PathBuf,
        final_path: PathBuf,
        expected_staging: ArtifactIdentity,
        expected_final: Option<ArtifactIdentity>,
    },
    DeleteOriginal {
        path: PathBuf,
        expected_original: ArtifactIdentity,
        expected_final: ArtifactIdentity,
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
    staging_identity: &ArtifactIdentity,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match &facts.staging {
        ArtifactObservation::Absent => OutputRecoveryAction::Append(OutputDelta::Abandoned {
            run_id: transaction.run_id,
        }),
        ArtifactObservation::Present(actual) if actual == staging_identity => {
            OutputRecoveryAction::DeleteStaging {
                path: transaction.staging.clone(),
                expected: staging_identity.clone(),
            }
        }
        ArtifactObservation::Present(_) => conflict("staging changed after abandonment intent"),
    }
}

fn recover_started(
    transaction: &OutputTransaction,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match &facts.staging {
        ArtifactObservation::Absent => OutputRecoveryAction::Append(OutputDelta::Abandoned {
            run_id: transaction.run_id,
        }),
        ArtifactObservation::Present(identity)
            if identity == &transaction.initial_staging_identity =>
        {
            OutputRecoveryAction::DeleteStaging {
                path: transaction.staging.clone(),
                expected: identity.clone(),
            }
        }
        ArtifactObservation::Present(_) => conflict("uncommitted staging identity changed"),
    }
}

fn recover_ready(
    transaction: &OutputTransaction,
    staging_identity: &ArtifactIdentity,
    facts: &FileSystemFacts,
) -> OutputRecoveryAction {
    match (&facts.staging, &facts.final_path) {
        (ArtifactObservation::Absent, ArtifactObservation::Present(final_identity))
            if final_identity.content_key == staging_identity.content_key =>
        {
            OutputRecoveryAction::Append(OutputDelta::OutputCommitted {
                run_id: transaction.run_id,
                final_identity: final_identity.clone(),
            })
        }
        (ArtifactObservation::Present(staging), final_observation)
            if staging == staging_identity
                && observation_matches_preimage(final_observation, &transaction.final_preimage) =>
        {
            OutputRecoveryAction::Promote {
                staging: transaction.staging.clone(),
                final_path: transaction.final_path.clone(),
                expected_staging: staging_identity.clone(),
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
    if facts.final_path != ArtifactObservation::Present(final_identity.clone()) {
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
    if facts.final_path != ArtifactObservation::Present(final_identity.clone()) {
        return conflict("output changed before original retirement");
    }
    match &facts.original {
        ArtifactObservation::Absent => OutputRecoveryAction::Append(OutputDelta::OriginalRetired {
            run_id: transaction.run_id,
        }),
        ArtifactObservation::Present(original) if original == &transaction.input_identity => {
            OutputRecoveryAction::DeleteOriginal {
                path: transaction.input.clone(),
                expected_original: original.clone(),
                expected_final: final_identity.clone(),
            }
        }
        ArtifactObservation::Present(_) => conflict("original changed before retirement"),
    }
}

fn observation_matches_preimage(
    observation: &ArtifactObservation,
    preimage: &Option<ArtifactIdentity>,
) -> bool {
    match (observation, preimage) {
        (ArtifactObservation::Absent, None) => true,
        (ArtifactObservation::Present(actual), Some(expected)) => actual == expected,
        _ => false,
    }
}

fn conflict(reason: &str) -> OutputRecoveryAction {
    OutputRecoveryAction::Conflict(RecoveryConflict {
        reason: reason.to_owned(),
    })
}
