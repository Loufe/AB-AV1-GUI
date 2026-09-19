use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, Serializer};

use crate::{
    AnalysisRowStatus, ContentKey, DestructiveIdentity, ImportPath, MediaObservation, PathHash,
    VideoMeta,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct AnalysisGenerationId(#[specta(type = crate::JsNumber)] pub u64);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct AnalysisRowId(#[specta(type = crate::JsNumber)] pub u64);

/// The complete public identity of an Analysis row. Row ids are allocated
/// only within one generation and must never be accepted on their own.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct AnalysisRowRef {
    pub generation: AnalysisGenerationId,
    pub row_id: AnalysisRowId,
}

/// Presentation text derived from a native path. `lossy` is explicit so the
/// UI never mistakes a replacement-character rendering for a reversible path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisDisplayText {
    pub text: String,
    pub lossy: bool,
}

/// A directory-local Level-0 failure. The generation continues after every
/// variant: this is standing row state, not a generation-wide terminal error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisDirectoryFailure {
    Missing,
    PermissionDenied,
    NotDirectory,
    TraversalRefused,
    EntriesUnavailable { count: u32, detail: String },
    Unavailable { detail: String },
}

/// Bounded, path-scrubbed diagnostic output from one ffprobe invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisDiagnosticTail {
    pub text: String,
    pub truncated: bool,
}

/// A file-local Basic Scan failure. One row failing never aborts its
/// generation; infrastructure failures that prevent the pool from making
/// progress remain generation-wide failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisScanFailure {
    Missing,
    Unavailable {
        detail: String,
    },
    TimedOut {
        diagnostic: AnalysisDiagnosticTail,
    },
    Rejected {
        diagnostic: AnalysisDiagnosticTail,
    },
    InvalidOutput {
        detail: String,
        diagnostic: AnalysisDiagnosticTail,
    },
    Supervision {
        detail: String,
        diagnostic: AnalysisDiagnosticTail,
    },
    ChangedAfterProbe,
    ChangedDuringSampling,
}

/// Standing Level-1 result for a discovered file. Imported analyses remain
/// durable provenance on the selected `FileRecord`; they are intentionally
/// not copied into this ephemeral row as reusable evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisFileScan {
    Discovered,
    Scanned {
        content_key: ContentKey,
        metadata: VideoMeta,
        refresh_failure: Option<Box<AnalysisScanFailure>>,
        status: Box<AnalysisRowStatus>,
    },
    SettledOutput {
        source_content_key: ContentKey,
        output_content_key: ContentKey,
        metadata: Option<VideoMeta>,
        refresh_failure: Option<Box<AnalysisScanFailure>>,
        status: Box<AnalysisRowStatus>,
    },
    Failed {
        failure: AnalysisScanFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisRowEntry {
    Folder {
        failure: Option<AnalysisDirectoryFailure>,
    },
    File {
        scan: AnalysisFileScan,
    },
}

/// Generation-scoped row facts. Mutually exclusive folder and file state is
/// represented by a tagged enum rather than nullable cross-variant fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisRow {
    pub id: AnalysisRowId,
    pub parent: Option<AnalysisRowId>,
    pub entry: AnalysisRowEntry,
    pub display_name: AnalysisDisplayText,
    pub display_path: AnalysisDisplayText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisActivity {
    Discovering,
    Discovered,
    BasicScanning,
    Ready,
    Cancelled,
    Failed { detail: String },
}

impl AnalysisActivity {
    /// Whether the generation still accepts row updates. A cancelled or
    /// failed generation is left exactly as it ended.
    #[must_use]
    pub const fn is_live(&self) -> bool {
        matches!(
            self,
            Self::Discovering | Self::Discovered | Self::BasicScanning | Self::Ready
        )
    }
}

/// One generation's standing public state. Native paths and execution
/// handles deliberately live in the engine's generation registry instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisGeneration {
    pub id: AnalysisGenerationId,
    pub roots: Vec<AnalysisDisplayText>,
    pub activity: AnalysisActivity,
    #[serde(serialize_with = "serialize_rows")]
    #[specta(type = Vec<AnalysisRow>)]
    pub rows: BTreeMap<AnalysisRowId, AnalysisRow>,
}

fn serialize_rows<S: Serializer>(
    rows: &BTreeMap<AnalysisRowId, AnalysisRow>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(rows.values())
}

/// Standing Analysis state. It is reducer-owned and replayed on subscribe,
/// but never enters the durable application snapshot or journal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisSnapshot {
    pub current: Option<AnalysisGeneration>,
}

/// Bounded live changes plus the complete replacement used at generation
/// start and reconnect. Consumers apply this through [`fold_analysis`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisDelta {
    Reset {
        snapshot: Box<AnalysisSnapshot>,
    },
    RowsUpserted {
        generation: AnalysisGenerationId,
        rows: Vec<AnalysisRow>,
    },
    ActivityChanged {
        generation: AnalysisGenerationId,
        activity: AnalysisActivity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisCommand {
    Begin {
        roots: Vec<AnalysisDisplayText>,
    },
    UpsertRows {
        generation: AnalysisGenerationId,
        rows: Vec<AnalysisRow>,
    },
    SetActivity {
        generation: AnalysisGenerationId,
        activity: AnalysisActivity,
    },
    BeginBasicScan {
        generation: AnalysisGenerationId,
    },
    InspectFile {
        generation: AnalysisGenerationId,
        row_id: AnalysisRowId,
        path_hash: PathHash,
        current: CurrentFileIdentity,
        timestamp_reliability: TimestampReliability,
        import_paths: Vec<ImportPath>,
    },
    ObserveFile {
        generation: AnalysisGenerationId,
        row_id: AnalysisRowId,
        observation: Box<MediaObservation>,
        import_paths: Vec<ImportPath>,
    },
    FailFile {
        generation: AnalysisGenerationId,
        row_id: AnalysisRowId,
        failure: AnalysisScanFailure,
    },
    FinishBasicScan {
        generation: AnalysisGenerationId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicScanDisposition {
    Complete,
    Observe,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnalysisMutationError {
    EmptyRoots,
    GenerationExhausted,
    InvalidActivityTransition,
    ResetIsNotNext,
    StaleGeneration,
    UnknownRow,
}

/// Allocate and install the next process-local generation. The discovery
/// command path calls this reducer primitive; callers never
/// supply their own generation id.
pub(crate) fn begin_analysis_generation(
    state: &AnalysisSnapshot,
    roots: Vec<AnalysisDisplayText>,
) -> Result<AnalysisDelta, AnalysisMutationError> {
    if roots.is_empty() {
        return Err(AnalysisMutationError::EmptyRoots);
    }
    let next = state.current.as_ref().map_or(Ok(1), |current| {
        current
            .id
            .0
            .checked_add(1)
            .ok_or(AnalysisMutationError::GenerationExhausted)
    })?;
    Ok(AnalysisDelta::Reset {
        snapshot: Box::new(AnalysisSnapshot {
            current: Some(AnalysisGeneration {
                id: AnalysisGenerationId(next),
                roots,
                activity: AnalysisActivity::Discovering,
                rows: BTreeMap::new(),
            }),
        }),
    })
}

/// Authoritative reducer-side mutation gate. Unlike the consumer fold,
/// replacement must advance the generation and live deltas must name the
/// current generation. The shell/frontend use [`fold_analysis`] because a
/// reconnect Reset is allowed to replace any local state.
pub(crate) fn apply_analysis_mutation(
    state: &mut AnalysisSnapshot,
    delta: &AnalysisDelta,
) -> Result<(), AnalysisMutationError> {
    validate_analysis_mutation(state, delta)?;
    fold_analysis(state, delta);
    Ok(())
}

/// The gate alone, so a handler can check a delta against standing state
/// without cloning the snapshot; `apply` folds the accepted delta later.
pub(crate) fn validate_analysis_mutation(
    state: &AnalysisSnapshot,
    delta: &AnalysisDelta,
) -> Result<(), AnalysisMutationError> {
    match delta {
        AnalysisDelta::Reset { snapshot } => {
            let Some(next) = snapshot.current.as_ref() else {
                return Err(AnalysisMutationError::ResetIsNotNext);
            };
            if next.roots.is_empty() {
                return Err(AnalysisMutationError::EmptyRoots);
            }
            let expected = state.current.as_ref().map_or(Ok(1), |current| {
                current
                    .id
                    .0
                    .checked_add(1)
                    .ok_or(AnalysisMutationError::GenerationExhausted)
            })?;
            if next.id != AnalysisGenerationId(expected) {
                return Err(AnalysisMutationError::ResetIsNotNext);
            }
        }
        AnalysisDelta::RowsUpserted { generation, rows } => {
            let Some(current) = state
                .current
                .as_ref()
                .filter(|current| current.id == *generation)
            else {
                return Err(AnalysisMutationError::StaleGeneration);
            };
            // New row ids are allocated only while discovery or a scan is
            // producing them; afterwards a live generation still refreshes
            // the rows it has.
            match current.activity {
                AnalysisActivity::Discovering | AnalysisActivity::BasicScanning => {}
                AnalysisActivity::Discovered | AnalysisActivity::Ready => {
                    if rows.iter().any(|row| !current.rows.contains_key(&row.id)) {
                        return Err(AnalysisMutationError::UnknownRow);
                    }
                }
                AnalysisActivity::Cancelled | AnalysisActivity::Failed { .. } => {
                    return Err(AnalysisMutationError::InvalidActivityTransition);
                }
            }
        }
        AnalysisDelta::ActivityChanged {
            generation,
            activity,
        } => {
            let Some(current) = state
                .current
                .as_ref()
                .filter(|current| current.id == *generation)
            else {
                return Err(AnalysisMutationError::StaleGeneration);
            };
            if !activity_transition_allowed(&current.activity, activity) {
                return Err(AnalysisMutationError::InvalidActivityTransition);
            }
        }
    }
    Ok(())
}

fn activity_transition_allowed(from: &AnalysisActivity, to: &AnalysisActivity) -> bool {
    matches!(
        (from, to),
        (AnalysisActivity::Discovering, AnalysisActivity::Discovered)
            | (AnalysisActivity::Discovering, AnalysisActivity::Cancelled)
            | (
                AnalysisActivity::Discovering,
                AnalysisActivity::Failed { .. }
            )
            | (
                AnalysisActivity::Discovered,
                AnalysisActivity::BasicScanning
            )
            | (AnalysisActivity::Discovered, AnalysisActivity::Cancelled)
            | (AnalysisActivity::BasicScanning, AnalysisActivity::Ready)
            | (AnalysisActivity::BasicScanning, AnalysisActivity::Cancelled)
            | (
                AnalysisActivity::BasicScanning,
                AnalysisActivity::Failed { .. }
            )
            | (AnalysisActivity::Ready, AnalysisActivity::BasicScanning)
            | (AnalysisActivity::Ready, AnalysisActivity::Cancelled)
    )
}

/// Structural Analysis fold shared by the reducer and shell mirror. A stale
/// live delta is ignored defensively; the reducer remains the authoritative
/// generation-validation boundary before such a delta is produced.
pub fn fold_analysis(state: &mut AnalysisSnapshot, delta: &AnalysisDelta) {
    match delta {
        AnalysisDelta::Reset { snapshot } => *state = (**snapshot).clone(),
        AnalysisDelta::RowsUpserted { generation, rows } => {
            let Some(current) = state
                .current
                .as_mut()
                .filter(|current| current.id == *generation)
            else {
                return;
            };
            for row in rows {
                current.rows.insert(row.id, row.clone());
            }
        }
        AnalysisDelta::ActivityChanged {
            generation,
            activity,
        } => {
            let Some(current) = state
                .current
                .as_mut()
                .filter(|current| current.id == *generation)
            else {
                return;
            };
            current.activity = activity.clone();
        }
    }
}

/// Engine-supplied judgment about whether an mtime is safe for the metadata
/// fast path. Core has no clock or filesystem knowledge and never infers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampReliability {
    Reliable,
    Unknown,
    CoarseOrRecent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentFileIdentity {
    Missing,
    Unavailable,
    Present(DestructiveIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessReason {
    NoBinding,
    FileIdentityChanged,
    SizeChanged,
    ModifiedTimeChanged,
    UnknownTimestamp,
    CoarseOrRecentTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshnessDecision {
    RecognizeSettledOutput,
    ReuseObservation,
    Reobserve(FreshnessReason),
    Missing,
    Unavailable,
}

/// Result of the three destructive-identity reads bracketing probe and
/// content sampling. A changed file never publishes a successful observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationStability {
    Stable,
    ChangedAfterProbe,
    ChangedDuringSampling,
}

#[must_use]
pub fn observation_stability(
    before_probe: &DestructiveIdentity,
    after_probe: &DestructiveIdentity,
    after_sampling: &DestructiveIdentity,
) -> ObservationStability {
    if before_probe != after_probe {
        ObservationStability::ChangedAfterProbe
    } else if after_probe != after_sampling {
        ObservationStability::ChangedDuringSampling
    } else {
        ObservationStability::Stable
    }
}

/// Decide the metadata fast path before ordinary path rebinding. Exact full
/// settled-output identity deliberately outranks a stale source binding.
/// Unknown/coarse/recent timestamps never become size-only cache hits.
#[must_use]
pub(crate) fn decide_freshness(
    current: &CurrentFileIdentity,
    cached: Option<&DestructiveIdentity>,
    settled_output: Option<&DestructiveIdentity>,
    timestamp_reliability: TimestampReliability,
) -> FreshnessDecision {
    let current = match current {
        CurrentFileIdentity::Missing => return FreshnessDecision::Missing,
        CurrentFileIdentity::Unavailable => return FreshnessDecision::Unavailable,
        CurrentFileIdentity::Present(identity) => identity,
    };
    let current_timestamp_reason = match timestamp_reliability {
        TimestampReliability::Reliable if current.modified_ns.is_some() => None,
        TimestampReliability::Reliable | TimestampReliability::Unknown => {
            Some(FreshnessReason::UnknownTimestamp)
        }
        TimestampReliability::CoarseOrRecent => Some(FreshnessReason::CoarseOrRecentTimestamp),
    };
    if let Some(reason) = current_timestamp_reason {
        return FreshnessDecision::Reobserve(reason);
    }
    // A completed replace-mode transaction carries a full destructive
    // identity captured after settlement. It wins before the stale source
    // binding, but only after timestamp reliability has admitted a metadata
    // shortcut (ADR-017).
    if settled_output == Some(current) {
        return FreshnessDecision::RecognizeSettledOutput;
    }
    let Some(cached) = cached else {
        return FreshnessDecision::Reobserve(FreshnessReason::NoBinding);
    };
    if current.file_id != cached.file_id {
        return FreshnessDecision::Reobserve(FreshnessReason::FileIdentityChanged);
    }
    if current.size != cached.size {
        return FreshnessDecision::Reobserve(FreshnessReason::SizeChanged);
    }
    if cached.modified_ns.is_none() {
        return FreshnessDecision::Reobserve(FreshnessReason::UnknownTimestamp);
    }
    if current.modified_ns != cached.modified_ns {
        return FreshnessDecision::Reobserve(FreshnessReason::ModifiedTimeChanged);
    }
    FreshnessDecision::ReuseObservation
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{FileSystemId, FileTimeNs};

    use super::*;

    fn identity(file: u64, size: u64, modified_ns: Option<u64>) -> DestructiveIdentity {
        DestructiveIdentity {
            file_id: FileSystemId::Unix {
                device: 1,
                inode: file,
            },
            size,
            modified_ns: modified_ns.map(FileTimeNs),
        }
    }

    fn row(id: u64, text: &str) -> AnalysisRow {
        let display = AnalysisDisplayText {
            text: text.to_owned(),
            lossy: false,
        };
        AnalysisRow {
            id: AnalysisRowId(id),
            parent: None,
            entry: AnalysisRowEntry::File {
                scan: AnalysisFileScan::Discovered,
            },
            display_name: display.clone(),
            display_path: display,
        }
    }

    fn snapshot(generation: u64) -> AnalysisSnapshot {
        AnalysisSnapshot {
            current: Some(AnalysisGeneration {
                id: AnalysisGenerationId(generation),
                roots: vec![AnalysisDisplayText {
                    text: "root".to_owned(),
                    lossy: false,
                }],
                activity: AnalysisActivity::Discovering,
                rows: BTreeMap::new(),
            }),
        }
    }

    #[test]
    fn reset_replaces_the_complete_standing_generation() {
        let mut state = snapshot(1);
        let replacement = snapshot(2);
        fold_analysis(
            &mut state,
            &AnalysisDelta::Reset {
                snapshot: Box::new(replacement.clone()),
            },
        );
        assert_eq!(state, replacement);
    }

    #[test]
    #[expect(clippy::expect_used, reason = "test assertion")]
    fn reducer_allocates_generations_and_rejects_stale_mutations() {
        let mut state = AnalysisSnapshot::default();
        assert_eq!(
            begin_analysis_generation(&state, Vec::new()),
            Err(AnalysisMutationError::EmptyRoots)
        );
        let first = begin_analysis_generation(
            &state,
            vec![AnalysisDisplayText {
                text: "first".to_owned(),
                lossy: false,
            }],
        )
        .expect("first generation");
        assert!(matches!(
            first,
            AnalysisDelta::Reset { ref snapshot }
                if snapshot.current.as_ref().map(|generation| generation.id)
                    == Some(AnalysisGenerationId(1))
        ));
        apply_analysis_mutation(&mut state, &first).expect("apply first generation");
        let second = begin_analysis_generation(
            &state,
            vec![AnalysisDisplayText {
                text: "second".to_owned(),
                lossy: false,
            }],
        )
        .expect("second generation");
        apply_analysis_mutation(&mut state, &second).expect("apply second generation");
        assert_eq!(
            state.current.as_ref().map(|generation| generation.id),
            Some(AnalysisGenerationId(2))
        );

        let stale = AnalysisDelta::RowsUpserted {
            generation: AnalysisGenerationId(1),
            rows: vec![row(1, "stale.mkv")],
        };
        assert_eq!(
            apply_analysis_mutation(&mut state, &stale),
            Err(AnalysisMutationError::StaleGeneration)
        );
        assert!(
            state
                .current
                .as_ref()
                .expect("current generation")
                .rows
                .is_empty()
        );

        assert_eq!(
            apply_analysis_mutation(
                &mut state,
                &AnalysisDelta::Reset {
                    snapshot: Box::new(snapshot(2)),
                },
            ),
            Err(AnalysisMutationError::ResetIsNotNext)
        );
        assert_eq!(
            apply_analysis_mutation(
                &mut state,
                &AnalysisDelta::Reset {
                    snapshot: Box::default(),
                },
            ),
            Err(AnalysisMutationError::ResetIsNotNext)
        );
        let mut empty_roots = snapshot(3);
        empty_roots
            .current
            .as_mut()
            .expect("generation")
            .roots
            .clear();
        assert_eq!(
            apply_analysis_mutation(
                &mut state,
                &AnalysisDelta::Reset {
                    snapshot: Box::new(empty_roots),
                },
            ),
            Err(AnalysisMutationError::EmptyRoots)
        );
        assert_eq!(
            begin_analysis_generation(
                &snapshot(u64::MAX),
                vec![AnalysisDisplayText {
                    text: "exhausted".to_owned(),
                    lossy: false,
                }],
            ),
            Err(AnalysisMutationError::GenerationExhausted)
        );
    }

    #[test]
    #[expect(clippy::expect_used, reason = "test assertion")]
    fn live_deltas_apply_only_to_the_current_generation() {
        let mut state = snapshot(2);
        fold_analysis(
            &mut state,
            &AnalysisDelta::RowsUpserted {
                generation: AnalysisGenerationId(1),
                rows: vec![row(1, "stale.mkv")],
            },
        );
        fold_analysis(
            &mut state,
            &AnalysisDelta::RowsUpserted {
                generation: AnalysisGenerationId(2),
                rows: vec![row(2, "current.mkv")],
            },
        );
        fold_analysis(
            &mut state,
            &AnalysisDelta::ActivityChanged {
                generation: AnalysisGenerationId(1),
                activity: AnalysisActivity::Failed {
                    detail: "stale".to_owned(),
                },
            },
        );

        let current = state.current.expect("current generation");
        assert_eq!(current.activity, AnalysisActivity::Discovering);
        assert_eq!(
            current.rows,
            BTreeMap::from([(AnalysisRowId(2), row(2, "current.mkv"))])
        );
    }

    #[test]
    #[expect(clippy::expect_used, reason = "test assertion")]
    fn activity_transitions_enforce_the_generation_lifecycle() {
        let allowed = [
            (AnalysisActivity::Discovering, AnalysisActivity::Discovered),
            (AnalysisActivity::Discovering, AnalysisActivity::Cancelled),
            (
                AnalysisActivity::Discovering,
                AnalysisActivity::Failed {
                    detail: "discovery failed".to_owned(),
                },
            ),
            (
                AnalysisActivity::Discovered,
                AnalysisActivity::BasicScanning,
            ),
            (AnalysisActivity::Discovered, AnalysisActivity::Cancelled),
            (AnalysisActivity::BasicScanning, AnalysisActivity::Ready),
            (AnalysisActivity::BasicScanning, AnalysisActivity::Cancelled),
            (
                AnalysisActivity::BasicScanning,
                AnalysisActivity::Failed {
                    detail: "scan failed".to_owned(),
                },
            ),
            (AnalysisActivity::Ready, AnalysisActivity::BasicScanning),
            (AnalysisActivity::Ready, AnalysisActivity::Cancelled),
        ];
        for (from, to) in allowed {
            let mut state = snapshot(1);
            state.current.as_mut().expect("generation").activity = from;
            assert_eq!(
                apply_analysis_mutation(
                    &mut state,
                    &AnalysisDelta::ActivityChanged {
                        generation: AnalysisGenerationId(1),
                        activity: to,
                    },
                ),
                Ok(())
            );
        }

        for from in [
            AnalysisActivity::Cancelled,
            AnalysisActivity::Failed {
                detail: "terminal".to_owned(),
            },
        ] {
            let mut state = snapshot(1);
            state.current.as_mut().expect("generation").activity = from;
            assert_eq!(
                apply_analysis_mutation(
                    &mut state,
                    &AnalysisDelta::ActivityChanged {
                        generation: AnalysisGenerationId(1),
                        activity: AnalysisActivity::Discovering,
                    },
                ),
                Err(AnalysisMutationError::InvalidActivityTransition)
            );
        }
    }

    #[test]
    #[expect(clippy::expect_used, reason = "test assertion")]
    fn rows_are_allocated_while_producing_and_refreshed_while_live() {
        let mut state = snapshot(1);
        fold_analysis(
            &mut state,
            &AnalysisDelta::RowsUpserted {
                generation: AnalysisGenerationId(1),
                rows: vec![row(1, "known.mkv")],
            },
        );
        let known = AnalysisDelta::RowsUpserted {
            generation: AnalysisGenerationId(1),
            rows: vec![row(1, "known-refreshed.mkv")],
        };
        let unknown = AnalysisDelta::RowsUpserted {
            generation: AnalysisGenerationId(1),
            rows: vec![row(2, "too-late.mkv")],
        };
        for activity in [AnalysisActivity::Discovered, AnalysisActivity::Ready] {
            state.current.as_mut().expect("generation").activity = activity;
            assert_eq!(apply_analysis_mutation(&mut state, &known), Ok(()));
            assert_eq!(
                apply_analysis_mutation(&mut state, &unknown),
                Err(AnalysisMutationError::UnknownRow)
            );
        }
        for activity in [
            AnalysisActivity::Cancelled,
            AnalysisActivity::Failed {
                detail: "terminal".to_owned(),
            },
        ] {
            state.current.as_mut().expect("generation").activity = activity;
            assert_eq!(
                apply_analysis_mutation(&mut state, &known),
                Err(AnalysisMutationError::InvalidActivityTransition)
            );
        }
    }

    #[test]
    fn observation_window_reports_the_stage_that_changed() {
        let before = identity(10, 100, Some(1_000));
        let after_probe = identity(11, 100, Some(1_000));
        let after_sampling = identity(10, 101, Some(1_000));
        assert_eq!(
            observation_stability(&before, &before, &before),
            ObservationStability::Stable
        );
        assert_eq!(
            observation_stability(&before, &after_probe, &after_probe),
            ObservationStability::ChangedAfterProbe
        );
        assert_eq!(
            observation_stability(&before, &before, &after_sampling),
            ObservationStability::ChangedDuringSampling
        );
    }

    #[test]
    fn settled_output_identity_precedes_the_cached_source_binding() {
        let source = identity(10, 100, Some(1_000));
        let output = identity(20, 60, Some(2_000));
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(output.clone()),
                Some(&source),
                Some(&output),
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::RecognizeSettledOutput
        );
    }

    #[test]
    fn settled_identity_with_unknown_mtime_requires_reobservation() {
        let source = identity(10, 100, None);
        let output = identity(20, 60, None);
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(output.clone()),
                Some(&source),
                Some(&output),
                TimestampReliability::Unknown,
            ),
            FreshnessDecision::Reobserve(FreshnessReason::UnknownTimestamp)
        );
    }

    #[test]
    fn same_stamp_with_a_different_file_id_requires_observation() {
        let cached = identity(10, 100, Some(1_000));
        let replacement = identity(20, 100, Some(1_000));
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(replacement),
                Some(&cached),
                None,
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::Reobserve(FreshnessReason::FileIdentityChanged)
        );
    }

    #[test]
    fn size_and_modified_time_changes_have_distinct_reasons() {
        let cached = identity(10, 100, Some(1_000));
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(identity(10, 101, Some(1_000))),
                Some(&cached),
                None,
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::Reobserve(FreshnessReason::SizeChanged)
        );
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(identity(10, 100, Some(2_000))),
                Some(&cached),
                None,
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::Reobserve(FreshnessReason::ModifiedTimeChanged)
        );
    }

    #[test]
    fn size_and_mtime_alone_never_recognize_a_settled_output() {
        let settled = identity(10, 100, Some(1_000));
        let replacement = identity(20, 100, Some(1_000));
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(replacement.clone()),
                Some(&replacement),
                Some(&settled),
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::ReuseObservation
        );
    }

    #[test]
    fn unknown_and_coarse_timestamps_never_reuse_by_size() {
        let unknown = identity(10, 100, None);
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(unknown.clone()),
                Some(&unknown),
                None,
                TimestampReliability::Unknown,
            ),
            FreshnessDecision::Reobserve(FreshnessReason::UnknownTimestamp)
        );
        let known = identity(10, 100, Some(1_000));
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(known.clone()),
                Some(&known),
                None,
                TimestampReliability::CoarseOrRecent,
            ),
            FreshnessDecision::Reobserve(FreshnessReason::CoarseOrRecentTimestamp)
        );
    }

    #[test]
    fn exact_reliable_destructive_identity_reuses_the_observation() {
        let known = identity(10, 100, Some(1_000));
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(known.clone()),
                Some(&known),
                None,
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::ReuseObservation
        );
    }

    #[test]
    fn unrelated_settled_output_with_unknown_mtime_does_not_poison_cache_reuse() {
        let known = identity(10, 100, Some(1_000));
        let unrelated = identity(20, 60, None);
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(known.clone()),
                Some(&known),
                Some(&unrelated),
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::ReuseObservation
        );
    }

    #[test]
    fn missing_unavailable_and_uncached_files_have_distinct_outcomes() {
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Missing,
                None,
                None,
                TimestampReliability::Unknown,
            ),
            FreshnessDecision::Missing
        );
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Unavailable,
                None,
                None,
                TimestampReliability::Unknown,
            ),
            FreshnessDecision::Unavailable
        );
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(identity(10, 100, Some(1_000))),
                None,
                None,
                TimestampReliability::Reliable,
            ),
            FreshnessDecision::Reobserve(FreshnessReason::NoBinding)
        );
    }

    #[test]
    fn moved_and_duplicated_paths_reobserve_before_probable_content_join() {
        let current = CurrentFileIdentity::Present(identity(10, 100, Some(1_000)));
        for scenario in ["moved", "duplicated"] {
            assert_eq!(
                decide_freshness(&current, None, None, TimestampReliability::Reliable),
                FreshnessDecision::Reobserve(FreshnessReason::NoBinding),
                "{scenario}"
            );
        }
    }
}
