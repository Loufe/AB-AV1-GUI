use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, Serializer};

use crate::DestructiveIdentity;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisEntryKind {
    Folder,
    File,
}

/// Level-0 row facts only. Media facts, applicability, predictions, and
/// failures extend the Analysis read model in their owning child issues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisRow {
    pub id: AnalysisRowId,
    pub parent: Option<AnalysisRowId>,
    pub kind: AnalysisEntryKind,
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

/// One generation's standing public state. Native paths and execution
/// handles deliberately live in the engine's generation registry instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisGeneration {
    pub id: AnalysisGenerationId,
    pub root: AnalysisDisplayText,
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
pub enum FreshnessDecision {
    RecognizeSettledOutput,
    ReuseObservation,
    Reobserve(FreshnessReason),
    Missing,
    Unavailable,
}

/// Decide the metadata fast path before ordinary path rebinding. Exact full
/// settled-output identity deliberately outranks a stale source binding.
/// Unknown/coarse/recent timestamps never become size-only cache hits.
#[must_use]
pub fn decide_freshness(
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
    if let Some(reason) = current_timestamp_reason {
        return FreshnessDecision::Reobserve(reason);
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
            kind: AnalysisEntryKind::File,
            display_name: display.clone(),
            display_path: display,
        }
    }

    fn snapshot(generation: u64) -> AnalysisSnapshot {
        AnalysisSnapshot {
            current: Some(AnalysisGeneration {
                id: AnalysisGenerationId(generation),
                root: AnalysisDisplayText {
                    text: "root".to_owned(),
                    lossy: false,
                },
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
    fn full_settled_identity_is_recognized_even_when_mtime_is_unknown() {
        let source = identity(10, 100, None);
        let output = identity(20, 60, None);
        assert_eq!(
            decide_freshness(
                &CurrentFileIdentity::Present(output.clone()),
                Some(&source),
                Some(&output),
                TimestampReliability::Unknown,
            ),
            FreshnessDecision::RecognizeSettledOutput
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
}
