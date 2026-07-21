use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, Serializer};

use crate::{DestructiveIdentity, ExecutionSettings, FileRecord, ParkedStatus, VerdictKind};

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

/// Highest useful Analysis tier. This value is always derived from current
/// facts and execution settings; it is never persisted as a second source of
/// truth.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub enum AnalysisLevel {
    Discovered,
    Scanned,
    Analyzed,
    Converted,
}

/// Separates what can be reused for the current file/settings from the best
/// thing known to have happened historically. An imported Analyzed summary,
/// for example, raises `historical` but cannot raise `applicable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AnalysisLevelAssessment {
    pub applicable: AnalysisLevel,
    pub historical: Option<AnalysisLevel>,
}

/// Derive current applicability and historical achievement for one row.
/// `record` must be the record selected by the row's freshly observed
/// `ContentKey`; passing it is the content-identity gate. `parked_status`
/// supplies display-only history before that path has been adopted.
#[must_use]
pub fn assess_analysis_levels(
    record: Option<&FileRecord>,
    parked_status: Option<ParkedStatus>,
    execution: &ExecutionSettings,
) -> AnalysisLevelAssessment {
    let applicable = record.map_or(AnalysisLevel::Discovered, |record| {
        match record.verdict.as_ref().map(|verdict| &verdict.kind) {
            Some(VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. }) => {
                AnalysisLevel::Converted
            }
            _ if crate::select_analysis(record, execution).is_some() => AnalysisLevel::Analyzed,
            _ => AnalysisLevel::Scanned,
        }
    });

    let mut historical = parked_status.map(level_for_imported_status);
    if let Some(record) = record {
        promote_level(&mut historical, AnalysisLevel::Scanned);
        if !record.analyses.is_empty() {
            promote_level(&mut historical, AnalysisLevel::Analyzed);
        }
        if let Some(imported) = &record.imported {
            promote_level(
                &mut historical,
                level_for_imported_status(imported.record.status),
            );
        }
        if let Some(verdict) = &record.verdict {
            let level = match &verdict.kind {
                VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. } => {
                    AnalysisLevel::Converted
                }
                VerdictKind::NotWorthwhile { .. } => AnalysisLevel::Analyzed,
            };
            promote_level(&mut historical, level);
        }
    }

    AnalysisLevelAssessment {
        applicable,
        historical,
    }
}

fn level_for_imported_status(status: ParkedStatus) -> AnalysisLevel {
    match status {
        ParkedStatus::Scanned => AnalysisLevel::Scanned,
        ParkedStatus::Analyzed | ParkedStatus::NotWorthwhile => AnalysisLevel::Analyzed,
        ParkedStatus::Converted => AnalysisLevel::Converted,
    }
}

fn promote_level(current: &mut Option<AnalysisLevel>, candidate: AnalysisLevel) {
    if current.is_none_or(|level| candidate > level) {
        *current = Some(candidate);
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisCommand {
    Begin {
        root: AnalysisDisplayText,
    },
    UpsertRows {
        generation: AnalysisGenerationId,
        rows: Vec<AnalysisRow>,
    },
    SetActivity {
        generation: AnalysisGenerationId,
        activity: AnalysisActivity,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisMutationError {
    GenerationExhausted,
    InvalidActivityTransition,
    ResetIsNotNext,
    StaleGeneration,
}

/// Allocate and install the next process-local generation. The discovery
/// command path added by #55 calls this reducer primitive; callers never
/// supply their own generation id.
pub fn begin_analysis_generation(
    state: &AnalysisSnapshot,
    root: AnalysisDisplayText,
) -> Result<AnalysisDelta, AnalysisMutationError> {
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
                root,
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
pub fn apply_analysis_mutation(
    state: &mut AnalysisSnapshot,
    delta: &AnalysisDelta,
) -> Result<(), AnalysisMutationError> {
    match delta {
        AnalysisDelta::Reset { snapshot } => {
            let Some(next) = snapshot.current.as_ref() else {
                return Err(AnalysisMutationError::ResetIsNotNext);
            };
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
        AnalysisDelta::RowsUpserted { generation, .. } => {
            if state.current.as_ref().map(|current| current.id) != Some(*generation) {
                return Err(AnalysisMutationError::StaleGeneration);
            }
            if !matches!(
                state.current.as_ref().map(|current| &current.activity),
                Some(AnalysisActivity::Discovering | AnalysisActivity::BasicScanning)
            ) {
                return Err(AnalysisMutationError::InvalidActivityTransition);
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
    fold_analysis(state, delta);
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
pub enum FreshnessDecision {
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
    if let Some(reason) = current_timestamp_reason {
        return FreshnessDecision::Reobserve(reason);
    }
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

    use crate::{
        AnalysisProfile, AnalysisResult, ContentKey, Crf, DecodeMode, DurationMs,
        ExecutionSettings, FileSystemId, FileTimeNs, HardwareDecoder, ImportPath,
        ImportedHistoryRecord, ImportedProvenance, MediaContainer, ParkedStatus, RunId,
        SearchMeasurement, UnixMillis, Verdict, VerdictKind, VideoCodec, VideoMeta, VmafScore,
        VmafTarget,
    };

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

    fn execution() -> ExecutionSettings {
        let mut profile = AnalysisProfile::production();
        profile.ab_av1_revision = "ab-fixture".to_owned();
        profile.ffmpeg_revision = "ffmpeg-fixture".to_owned();
        profile.encoder_revision = "encoder-fixture".to_owned();
        ExecutionSettings::production(profile, false)
    }

    fn record() -> FileRecord {
        FileRecord::new(VideoMeta {
            codec: VideoCodec::H264,
            container: MediaContainer::Matroska,
            width: 1_920,
            height: 1_080,
            rotation_degrees: 0,
            duration_ms: 60_000,
            size_bytes: 1_000,
            audio: Vec::new(),
            subtitle_count: 0,
        })
    }

    fn reusable_result(execution: &ExecutionSettings) -> AnalysisResult {
        AnalysisResult {
            requested_target: execution.requested_target,
            successful_target: execution.requested_target,
            fallback_floor: execution.fallback_floor,
            fallback_step: execution.fallback_step,
            failed_attempts: Vec::new(),
            measurement: SearchMeasurement {
                crf: Crf(30),
                score: VmafScore(9_500),
                predicted_size: 500,
                predicted_percent_basis_points: 5_000,
                predicted_duration_ms: 30_000,
                from_cache: false,
            },
            profile: execution.profile.clone(),
        }
    }

    fn imported(status: ParkedStatus) -> ImportedProvenance {
        ImportedProvenance {
            import_path: ImportPath("/videos/movie.mkv".to_owned()),
            record: ImportedHistoryRecord {
                status,
                size: None,
                modified_ns: None,
                video_codec: None,
                width: None,
                height: None,
                duration_ms: None,
                output_size: None,
                encoding_time: None,
                crf: None,
                vmaf: None,
                target: None,
                requested_target: None,
                floor_target: None,
                decided_at: UnixMillis(1),
            },
        }
    }

    fn converted_verdict() -> Verdict {
        Verdict {
            kind: VerdictKind::Converted {
                output_content_key: None,
                input_size: None,
                output_size: None,
                encoding_time: Some(DurationMs(1)),
                crf: None,
                vmaf: None,
                target: None,
            },
            source_run: Some(RunId(1)),
            decided_at: UnixMillis(2),
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
    fn reducer_allocates_generations_and_rejects_stale_mutations() {
        let mut state = AnalysisSnapshot::default();
        let first = begin_analysis_generation(
            &state,
            AnalysisDisplayText {
                text: "first".to_owned(),
                lossy: false,
            },
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
            AnalysisDisplayText {
                text: "second".to_owned(),
                lossy: false,
            },
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
        assert_eq!(
            begin_analysis_generation(
                &snapshot(u64::MAX),
                AnalysisDisplayText {
                    text: "exhausted".to_owned(),
                    lossy: false,
                },
            ),
            Err(AnalysisMutationError::GenerationExhausted)
        );
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

        let mut discovered = snapshot(1);
        discovered.current.as_mut().expect("generation").activity = AnalysisActivity::Discovered;
        assert_eq!(
            apply_analysis_mutation(
                &mut discovered,
                &AnalysisDelta::RowsUpserted {
                    generation: AnalysisGenerationId(1),
                    rows: vec![row(1, "too-late.mkv")],
                },
            ),
            Err(AnalysisMutationError::InvalidActivityTransition)
        );
    }

    #[test]
    fn current_level_and_historical_achievement_are_independent() {
        let execution = execution();
        assert_eq!(
            assess_analysis_levels(None, None, &execution),
            AnalysisLevelAssessment {
                applicable: AnalysisLevel::Discovered,
                historical: None,
            }
        );
        assert_eq!(
            assess_analysis_levels(None, Some(ParkedStatus::Analyzed), &execution),
            AnalysisLevelAssessment {
                applicable: AnalysisLevel::Discovered,
                historical: Some(AnalysisLevel::Analyzed),
            }
        );

        let mut adopted = record();
        adopted.imported = Some(imported(ParkedStatus::Analyzed));
        assert_eq!(
            assess_analysis_levels(Some(&adopted), None, &execution),
            AnalysisLevelAssessment {
                applicable: AnalysisLevel::Scanned,
                historical: Some(AnalysisLevel::Analyzed),
            }
        );
    }

    #[test]
    fn exact_native_analysis_is_currently_applicable() {
        let execution = execution();
        let mut record = record();
        record.record_analysis(reusable_result(&execution));
        assert_eq!(
            assess_analysis_levels(Some(&record), None, &execution),
            AnalysisLevelAssessment {
                applicable: AnalysisLevel::Analyzed,
                historical: Some(AnalysisLevel::Analyzed),
            }
        );
    }

    #[test]
    fn every_measurement_profile_field_participates_in_current_reuse() {
        let execution = execution();
        let mut record = record();
        record.record_analysis(reusable_result(&execution));

        let mut incompatible = Vec::new();
        let mut changed = execution.clone();
        changed.profile.preset = changed.profile.preset.saturating_sub(1);
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.max_encoded_percent_basis_points += 1;
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.samples = Some(4);
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.sample_duration_ms += 1;
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.thorough = !changed.profile.thorough;
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.decode_mode = DecodeMode::Hardware(HardwareDecoder::H264Cuvid);
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.ab_av1_revision.push_str("-new");
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.ffmpeg_revision.push_str("-new");
        incompatible.push(changed);
        let mut changed = execution.clone();
        changed.profile.encoder_revision.push_str("-new");
        incompatible.push(changed);

        for changed in incompatible {
            let assessment = assess_analysis_levels(Some(&record), None, &changed);
            assert_eq!(assessment.applicable, AnalysisLevel::Scanned);
            assert_eq!(assessment.historical, Some(AnalysisLevel::Analyzed));
        }
        assert_eq!(
            assess_analysis_levels(Some(&record), None, &execution).applicable,
            AnalysisLevel::Analyzed
        );
    }

    #[test]
    fn target_and_fallback_provenance_control_current_reuse() {
        let execution = execution();
        let mut record = record();
        record.record_analysis(reusable_result(&execution));

        let mut lower_target = execution.clone();
        lower_target.requested_target = VmafTarget(execution.requested_target.0 - 1);
        assert_eq!(
            assess_analysis_levels(Some(&record), None, &lower_target).applicable,
            AnalysisLevel::Analyzed
        );

        let mut higher_target = execution.clone();
        higher_target.requested_target = VmafTarget(execution.requested_target.0 + 1);
        assert_eq!(
            assess_analysis_levels(Some(&record), None, &higher_target).applicable,
            AnalysisLevel::Scanned
        );

        let mut changed_ladder = execution.clone();
        changed_ladder.fallback_step += 1;
        let mut fallback_result = reusable_result(&execution);
        fallback_result.successful_target = VmafTarget(execution.requested_target.0 - 1);
        let mut fallback_record = self::record();
        fallback_record.record_analysis(fallback_result);
        assert_eq!(
            assess_analysis_levels(Some(&fallback_record), None, &changed_ladder).applicable,
            AnalysisLevel::Scanned
        );
    }

    #[test]
    fn decisive_conversion_is_current_but_not_worthwhile_is_history_only() {
        let execution = execution();
        let mut converted = record();
        converted.verdict = Some(converted_verdict());
        assert_eq!(
            assess_analysis_levels(Some(&converted), None, &execution),
            AnalysisLevelAssessment {
                applicable: AnalysisLevel::Converted,
                historical: Some(AnalysisLevel::Converted),
            }
        );

        let mut not_worthwhile = record();
        not_worthwhile.verdict = Some(Verdict {
            kind: VerdictKind::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
            },
            source_run: None,
            decided_at: UnixMillis(2),
        });
        assert_eq!(
            assess_analysis_levels(Some(&not_worthwhile), None, &execution),
            AnalysisLevelAssessment {
                applicable: AnalysisLevel::Scanned,
                historical: Some(AnalysisLevel::Analyzed),
            }
        );
    }

    #[test]
    fn every_imported_status_maps_only_to_historical_achievement() {
        let execution = execution();
        let cases = [
            (ParkedStatus::Scanned, AnalysisLevel::Scanned),
            (ParkedStatus::Analyzed, AnalysisLevel::Analyzed),
            (ParkedStatus::NotWorthwhile, AnalysisLevel::Analyzed),
            (ParkedStatus::Converted, AnalysisLevel::Converted),
        ];
        for (status, historical) in cases {
            let mut adopted = record();
            adopted.imported = Some(imported(status));
            assert_eq!(
                assess_analysis_levels(Some(&adopted), None, &execution),
                AnalysisLevelAssessment {
                    applicable: AnalysisLevel::Scanned,
                    historical: Some(historical),
                }
            );
            assert_eq!(
                assess_analysis_levels(None, Some(status), &execution),
                AnalysisLevelAssessment {
                    applicable: AnalysisLevel::Discovered,
                    historical: Some(historical),
                }
            );
        }
    }

    #[test]
    fn native_remux_is_a_converted_achievement() {
        let execution = execution();
        let mut remuxed = record();
        remuxed.verdict = Some(Verdict {
            kind: VerdictKind::Remuxed {
                output_content_key: ContentKey("remux-output".to_owned()),
                input_size: Some(1_000),
                output_size: Some(900),
            },
            source_run: Some(RunId(2)),
            decided_at: UnixMillis(2),
        });
        assert_eq!(
            assess_analysis_levels(Some(&remuxed), None, &execution),
            AnalysisLevelAssessment {
                applicable: AnalysisLevel::Converted,
                historical: Some(AnalysisLevel::Converted),
            }
        );
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
