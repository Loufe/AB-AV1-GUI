//! Row status: what a claim on the file behind an Analysis row would do and
//! reuse. The reducer projects it from the same execution composition and
//! job policy the claim runs (`compose_execution`, `select_job_action`), so a
//! row can never report a result a claim would not reproduce. Statuses are
//! recomputed inside `apply` whenever the facts they derive from change;
//! nothing here is persisted.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    AnalysisDelta, AnalysisFileScan, AnalysisIntent, AnalysisResult, AnalysisRow, AnalysisRowEntry,
    AppState, Applied, ContentKey, Crf, DurableDelta, DurationMs, EphemeralDelta,
    ExecutionUnavailable, FileRecord, JobAction, Operation, OverwriteDecision, ParkedStatus, RunId,
    SkipReason, UnixMillis, Verdict, VerdictKind, VideoMeta, VmafScore, VmafTarget,
    compose_execution, fold_analysis, select_job_action, validate_analysis_mutation,
};

/// Rows refreshed by one command are published in batches of this size, the
/// same bound discovery uses, so a whole-generation refresh never produces
/// one unbounded delta.
pub const ANALYSIS_REFRESH_BATCH_ROWS: usize = 128;

/// What the current facts, tools, and settings let a claim do with the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisRowStatus {
    pub applicable: ApplicableLevel,
    pub historical: HistoricalLevel,
    pub analyze: RowEligibility,
    pub convert: RowEligibility,
}

/// The highest tier a claim would reuse right now. A row that carries no
/// status is merely discovered; every observed row stands at least at
/// `Scanned`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum ApplicableLevel {
    Scanned { reuse: ReuseStanding },
    Analyzed { prediction: SearchPrediction },
    Converted { summary: ConversionSummary },
}

/// Why a scanned row has no reusable analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub enum ReuseStanding {
    /// No native search has ever been recorded for this content.
    Unanalyzed,
    /// Searches exist, but none was recorded under the profile and targets a
    /// claim would use now.
    Inapplicable,
    /// The claim profile cannot be composed until the located tools are
    /// verified, so no stored search can be matched against it.
    ToolchainUnverified { reason: ExecutionUnavailable },
}

/// The highest tier the content is known to have reached, from native
/// searches, verdicts, and imported provenance alike. Never below
/// `applicable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, specta::Type)]
pub enum HistoricalLevel {
    Scanned,
    Analyzed,
    Converted,
}

/// ab-av1's prediction from the search a claim would reuse. Distinct from a
/// measured conversion summary: nothing here has happened yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct SearchPrediction {
    pub crf: Crf,
    pub score: VmafScore,
    #[specta(type = crate::JsNumber)]
    pub predicted_size: u64,
    pub predicted_percent_basis_points: u32,
    #[specta(type = crate::JsNumber)]
    pub predicted_duration_ms: u64,
    pub requested_target: VmafTarget,
    pub successful_target: VmafTarget,
}

impl SearchPrediction {
    fn from_result(result: &AnalysisResult) -> Self {
        Self {
            crf: result.measurement.crf,
            score: result.measurement.score,
            predicted_size: result.measurement.predicted_size,
            predicted_percent_basis_points: result.measurement.predicted_percent_basis_points,
            predicted_duration_ms: result.measurement.predicted_duration_ms,
            requested_target: result.requested_target,
            successful_target: result.successful_target,
        }
    }
}

/// The measured summary of the conversion that decided the content. Every
/// measurement is nullable because an imported or crash-recovered verdict
/// cannot honestly supply it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum ConversionSummary {
    Encoded {
        #[specta(type = Option<crate::JsNumber>)]
        input_size: Option<u64>,
        #[specta(type = Option<crate::JsNumber>)]
        output_size: Option<u64>,
        encoding_time: Option<DurationMs>,
        crf: Option<Crf>,
        vmaf: Option<VmafScore>,
        target: Option<VmafTarget>,
        decided_at: UnixMillis,
        source_run: Option<RunId>,
    },
    Remuxed {
        #[specta(type = Option<crate::JsNumber>)]
        input_size: Option<u64>,
        #[specta(type = Option<crate::JsNumber>)]
        output_size: Option<u64>,
        decided_at: UnixMillis,
        source_run: Option<RunId>,
    },
}

impl ConversionSummary {
    fn from_verdict(verdict: &Verdict) -> Option<Self> {
        match &verdict.kind {
            VerdictKind::Converted {
                input_size,
                output_size,
                encoding_time,
                crf,
                vmaf,
                target,
                ..
            } => Some(Self::Encoded {
                input_size: *input_size,
                output_size: *output_size,
                encoding_time: *encoding_time,
                crf: *crf,
                vmaf: *vmaf,
                target: *target,
                decided_at: verdict.decided_at,
                source_run: verdict.source_run,
            }),
            VerdictKind::Remuxed {
                input_size,
                output_size,
                ..
            } => Some(Self::Remuxed {
                input_size: *input_size,
                output_size: *output_size,
                decided_at: verdict.decided_at,
                source_run: verdict.source_run,
            }),
            VerdictKind::NotWorthwhile { .. } => None,
        }
    }
}

/// What a claim for one operation would do with the file, before any
/// filesystem check the engine performs at claim time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum RowEligibility {
    Eligible,
    Remux,
    Skip { reason: SkipReason },
}

fn eligibility(action: JobAction) -> RowEligibility {
    match action {
        JobAction::Skip { reason } => RowEligibility::Skip { reason },
        JobAction::Remux => RowEligibility::Remux,
        JobAction::Analyze { .. } | JobAction::Encode { .. } => RowEligibility::Eligible,
    }
}

/// The observed facts a row's status derives from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ScanFacts<'a> {
    Scanned {
        content_key: &'a ContentKey,
        metadata: &'a VideoMeta,
    },
    /// The file is the settled output of a conversion of `source_content_key`.
    /// The level reports that conversion; eligibility is the output file's
    /// own.
    SettledOutput {
        source_content_key: &'a ContentKey,
        output_content_key: &'a ContentKey,
        metadata: Option<&'a VideoMeta>,
    },
}

impl AnalysisFileScan {
    pub(crate) fn facts(&self) -> Option<ScanFacts<'_>> {
        match self {
            Self::Scanned {
                content_key,
                metadata,
                ..
            } => Some(ScanFacts::Scanned {
                content_key,
                metadata,
            }),
            Self::SettledOutput {
                source_content_key,
                output_content_key,
                metadata,
                ..
            } => Some(ScanFacts::SettledOutput {
                source_content_key,
                output_content_key,
                metadata: metadata.as_ref(),
            }),
            Self::Discovered | Self::Failed { .. } => None,
        }
    }

    pub(crate) fn status(&self) -> Option<&AnalysisRowStatus> {
        match self {
            Self::Scanned { status, .. } | Self::SettledOutput { status, .. } => Some(status),
            Self::Discovered | Self::Failed { .. } => None,
        }
    }

    fn with_status(&self, status: AnalysisRowStatus) -> Self {
        let mut scan = self.clone();
        match &mut scan {
            Self::Scanned { status: slot, .. } | Self::SettledOutput { status: slot, .. } => {
                **slot = status;
            }
            Self::Discovered | Self::Failed { .. } => {}
        }
        scan
    }
}

/// Project one row's status from the current state. Eligibility reads only
/// the fallback floor from the execution, which the configured base already
/// carries, so unverified tools never hide a skip; reuse needs the composed
/// profile and reports `ToolchainUnverified` until it exists.
#[must_use]
pub(crate) fn project_row_status(state: &AppState, facts: ScanFacts<'_>) -> AnalysisRowStatus {
    let (content_record, metadata, file_record) = match facts {
        ScanFacts::Scanned {
            content_key,
            metadata,
        } => {
            let record = state.durable.records.get(content_key);
            (record, Some(metadata), record)
        }
        ScanFacts::SettledOutput {
            source_content_key,
            output_content_key,
            metadata,
        } => (
            state.durable.records.get(source_content_key),
            metadata,
            state.durable.records.get(output_content_key),
        ),
    };
    let composed = compose_execution(
        &state.execution,
        &state.tools,
        &state.settings,
        OverwriteDecision::FollowSettings,
        metadata.map(|metadata| &metadata.codec),
    );
    let execution = composed.as_ref().unwrap_or(&state.execution);
    let analyze = select_job_action(
        metadata,
        file_record,
        Operation::Analyze,
        AnalysisIntent::ReuseIfFresh,
        execution,
    );
    let convert = select_job_action(
        metadata,
        file_record,
        Operation::Convert,
        AnalysisIntent::ReuseIfFresh,
        execution,
    );
    let summary = content_record
        .and_then(|record| record.verdict.as_ref())
        .and_then(ConversionSummary::from_verdict);
    let applicable = match (summary, &composed, &analyze) {
        (Some(summary), _, _) => ApplicableLevel::Converted { summary },
        (None, Err(reason), _) => ApplicableLevel::Scanned {
            reuse: ReuseStanding::ToolchainUnverified { reason: *reason },
        },
        (
            None,
            Ok(_),
            JobAction::Analyze {
                selected_analysis: Some(result),
            },
        ) => ApplicableLevel::Analyzed {
            prediction: SearchPrediction::from_result(result),
        },
        (None, Ok(_), _) => ApplicableLevel::Scanned {
            reuse: if content_record.is_some_and(|record| !record.analyses.is_empty()) {
                ReuseStanding::Inapplicable
            } else {
                ReuseStanding::Unanalyzed
            },
        },
    };
    AnalysisRowStatus {
        applicable,
        historical: historical_level(content_record),
        analyze: eligibility(analyze),
        convert: eligibility(convert),
    }
}

fn historical_level(record: Option<&FileRecord>) -> HistoricalLevel {
    let mut level = HistoricalLevel::Scanned;
    let Some(record) = record else {
        return level;
    };
    if !record.analyses.is_empty() {
        level = level.max(HistoricalLevel::Analyzed);
    }
    if let Some(imported) = &record.imported {
        level = level.max(match imported.record.status {
            ParkedStatus::Scanned => HistoricalLevel::Scanned,
            ParkedStatus::Analyzed | ParkedStatus::NotWorthwhile => HistoricalLevel::Analyzed,
            ParkedStatus::Converted => HistoricalLevel::Converted,
        });
    }
    if let Some(verdict) = &record.verdict {
        level = level.max(match verdict.kind {
            VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. } => {
                HistoricalLevel::Converted
            }
            VerdictKind::NotWorthwhile { .. } => HistoricalLevel::Analyzed,
        });
    }
    level
}

/// Which rows one applied command may have changed the status of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefreshScope {
    Nothing,
    Content(BTreeSet<ContentKey>),
    Everything,
}

impl RefreshScope {
    fn covers(&self, facts: &ScanFacts<'_>) -> bool {
        match self {
            Self::Nothing => false,
            Self::Everything => true,
            Self::Content(keys) => match facts {
                ScanFacts::Scanned { content_key, .. } => keys.contains(*content_key),
                ScanFacts::SettledOutput {
                    source_content_key,
                    output_content_key,
                    ..
                } => keys.contains(*source_content_key) || keys.contains(*output_content_key),
            },
        }
    }
}

/// Decide the refresh scope after a command's deltas have folded. Tool and
/// execution changes touch every row; durable content facts touch the rows
/// observing that content. `everything` carries the command-level triggers
/// `apply` can only see before folding (a base execution or hardware decode
/// change).
#[must_use]
pub(crate) fn refresh_scope(state: &AppState, applied: &Applied, everything: bool) -> RefreshScope {
    if everything
        || applied
            .ephemeral
            .iter()
            .any(|delta| matches!(delta, EphemeralDelta::ToolsChanged(_)))
    {
        return RefreshScope::Everything;
    }
    let mut keys = BTreeSet::new();
    for delta in &applied.durable {
        match delta {
            DurableDelta::MediaObserved { observation } => {
                keys.insert(observation.binding.content_key.clone());
            }
            DurableDelta::AnalysisRecorded { run_id, .. }
            | DurableDelta::ItemFinished { run_id, .. } => {
                if let Some(content_key) = state
                    .durable
                    .conversion_runs
                    .get(run_id)
                    .and_then(|run| run.spec.content_key.clone())
                {
                    keys.insert(content_key);
                }
            }
            DurableDelta::ParkedAdopted { content_key, .. } => {
                keys.insert(content_key.clone());
            }
            _ => {}
        }
    }
    if keys.is_empty() {
        RefreshScope::Nothing
    } else {
        RefreshScope::Content(keys)
    }
}

/// Re-project every live row the scope covers and publish the ones whose
/// status changed. A row this command already published is patched in place,
/// so one `Applied` never carries two versions of a row; the rest go out in
/// bounded batches after it. Terminal generations are left as they ended.
pub(crate) fn refresh_analysis_rows(
    state: &mut AppState,
    applied: &mut Applied,
    scope: &RefreshScope,
) {
    if *scope == RefreshScope::Nothing {
        return;
    }
    let (generation, mut updates) = {
        let Some(current) = state.analysis.current.as_ref() else {
            return;
        };
        if !current.activity.is_live() {
            return;
        }
        let updates: Vec<AnalysisRow> = current
            .rows
            .values()
            .filter_map(|row| {
                let AnalysisRowEntry::File { scan } = &row.entry else {
                    return None;
                };
                let facts = scan.facts()?;
                if !scope.covers(&facts) {
                    return None;
                }
                let status = project_row_status(state, facts);
                (scan.status() != Some(&status)).then(|| AnalysisRow {
                    entry: AnalysisRowEntry::File {
                        scan: scan.with_status(status),
                    },
                    ..row.clone()
                })
            })
            .collect();
        (current.id, updates)
    };
    if updates.is_empty() {
        return;
    }
    for delta in &mut applied.ephemeral {
        let EphemeralDelta::Analysis(delta) = delta else {
            continue;
        };
        let AnalysisDelta::RowsUpserted {
            generation: published,
            rows,
        } = delta
        else {
            continue;
        };
        if *published != generation {
            continue;
        }
        let mut patched = false;
        for row in rows.iter_mut() {
            if let Some(index) = updates.iter().position(|update| update.id == row.id) {
                *row = updates.remove(index);
                patched = true;
            }
        }
        if patched {
            fold_analysis(&mut state.analysis, delta);
        }
    }
    let mut pending = updates.into_iter();
    loop {
        let rows: Vec<AnalysisRow> = pending.by_ref().take(ANALYSIS_REFRESH_BATCH_ROWS).collect();
        if rows.is_empty() {
            break;
        }
        let delta = AnalysisDelta::RowsUpserted { generation, rows };
        debug_assert!(validate_analysis_mutation(&state.analysis, &delta).is_ok());
        fold_analysis(&mut state.analysis, &delta);
        applied.ephemeral.push(EphemeralDelta::Analysis(delta));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::*;
    use crate::{
        AnalysisProfile, AudioCodec, AudioStreamMeta, DecodeMode, DurationMs, ExecutionSettings,
        HardwareDecoder, ImportPath, ImportedHistoryRecord, ImportedProvenance, LocatedTool,
        LocatedTools, MediaContainer, Settings, ToolAvailability, ToolRevisions, ToolSource,
        ToolVerification, VideoCodec,
    };

    fn located(verification: ToolVerification) -> ToolAvailability {
        ToolAvailability::Located {
            tools: LocatedTools {
                ffmpeg: LocatedTool {
                    source: ToolSource::SearchPath,
                    path: PathBuf::from("/usr/bin/ffmpeg"),
                },
                ffprobe: LocatedTool {
                    source: ToolSource::SearchPath,
                    path: PathBuf::from("/usr/bin/ffprobe"),
                },
            },
            verification,
        }
    }

    fn verified(decoders: &[HardwareDecoder]) -> ToolAvailability {
        located(ToolVerification::Verified {
            revisions: ToolRevisions {
                ab_av1: "ab-fixture".to_owned(),
                ffmpeg: "ffmpeg-fixture".to_owned(),
                encoder: "encoder-fixture".to_owned(),
            },
            hardware_decoders: decoders.iter().copied().collect(),
        })
    }

    fn state_with(tools: ToolAvailability) -> AppState {
        AppState {
            tools,
            settings: Settings {
                hardware_decode: false,
                ..Settings::default()
            },
            ..AppState::default()
        }
    }

    fn meta(codec: VideoCodec, container: MediaContainer, width: u32, height: u32) -> VideoMeta {
        VideoMeta {
            codec,
            container,
            width,
            height,
            rotation_degrees: 0,
            duration_ms: 60_000,
            size_bytes: 1_000,
            audio: vec![AudioStreamMeta {
                codec: AudioCodec::Aac,
                channels: 2,
            }],
            subtitle_count: 0,
        }
    }

    fn hd() -> VideoMeta {
        meta(VideoCodec::H264, MediaContainer::Matroska, 1_920, 1_080)
    }

    fn key(name: &str) -> ContentKey {
        ContentKey(name.to_owned())
    }

    fn insert_record(state: &mut AppState, name: &str, metadata: VideoMeta) -> ContentKey {
        let content_key = key(name);
        state
            .durable
            .records
            .insert(content_key.clone(), FileRecord::new(metadata));
        content_key
    }

    fn composed(state: &AppState, codec: &VideoCodec) -> ExecutionSettings {
        match compose_execution(
            &state.execution,
            &state.tools,
            &state.settings,
            OverwriteDecision::FollowSettings,
            Some(codec),
        ) {
            Ok(execution) => execution,
            Err(reason) => panic!("fixture tools must compose: {reason:?}"),
        }
    }

    fn result_for(execution: &ExecutionSettings) -> AnalysisResult {
        AnalysisResult {
            requested_target: execution.requested_target,
            successful_target: execution.requested_target,
            fallback_floor: execution.fallback_floor,
            fallback_step: execution.fallback_step,
            failed_attempts: Vec::new(),
            measurement: crate::SearchMeasurement {
                crf: Crf(30_000),
                score: VmafScore(9_550),
                predicted_size: 500,
                predicted_percent_basis_points: 5_000,
                predicted_duration_ms: 30_000,
                from_cache: false,
            },
            profile: execution.profile.clone(),
        }
    }

    fn record_result(state: &mut AppState, content_key: &ContentKey, result: AnalysisResult) {
        if let Some(record) = state.durable.records.get_mut(content_key) {
            record.record_analysis(result);
        }
    }

    fn set_verdict(state: &mut AppState, content_key: &ContentKey, kind: VerdictKind) {
        if let Some(record) = state.durable.records.get_mut(content_key) {
            record.verdict = Some(Verdict {
                kind,
                source_run: Some(RunId(7)),
                decided_at: UnixMillis(2),
            });
        }
    }

    fn converted_kind() -> VerdictKind {
        VerdictKind::Converted {
            output_content_key: Some(key("output")),
            input_size: Some(1_000),
            output_size: Some(400),
            encoding_time: Some(DurationMs(90_000)),
            crf: Some(Crf(30_000)),
            vmaf: Some(VmafScore(9_550)),
            target: Some(VmafTarget(95)),
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

    fn scanned_status(
        state: &AppState,
        content_key: &ContentKey,
        metadata: &VideoMeta,
    ) -> AnalysisRowStatus {
        project_row_status(
            state,
            ScanFacts::Scanned {
                content_key,
                metadata,
            },
        )
    }

    #[test]
    fn unverified_tools_report_eligibility_but_no_reuse() {
        let cases = [
            (
                ToolAvailability::default(),
                ExecutionUnavailable::ToolsMissing,
            ),
            (
                located(ToolVerification::Pending),
                ExecutionUnavailable::ToolsPending,
            ),
        ];
        for (tools, reason) in cases {
            let mut state = state_with(tools);
            let content_key = insert_record(&mut state, "movie", hd());
            let execution = ExecutionSettings::default();
            record_result(&mut state, &content_key, result_for(&execution));
            assert_eq!(
                scanned_status(&state, &content_key, &hd()),
                AnalysisRowStatus {
                    applicable: ApplicableLevel::Scanned {
                        reuse: ReuseStanding::ToolchainUnverified { reason },
                    },
                    historical: HistoricalLevel::Analyzed,
                    analyze: RowEligibility::Eligible,
                    convert: RowEligibility::Eligible,
                }
            );
        }
    }

    #[test]
    fn a_reusable_native_search_is_applicable_with_its_prediction() {
        let mut state = state_with(verified(&[]));
        let content_key = insert_record(&mut state, "movie", hd());
        let unanalyzed = scanned_status(&state, &content_key, &hd());
        assert_eq!(
            unanalyzed.applicable,
            ApplicableLevel::Scanned {
                reuse: ReuseStanding::Unanalyzed
            }
        );
        assert_eq!(unanalyzed.historical, HistoricalLevel::Scanned);

        let execution = composed(&state, &VideoCodec::H264);
        record_result(&mut state, &content_key, result_for(&execution));
        let status = scanned_status(&state, &content_key, &hd());
        assert_eq!(
            status.applicable,
            ApplicableLevel::Analyzed {
                prediction: SearchPrediction {
                    crf: Crf(30_000),
                    score: VmafScore(9_550),
                    predicted_size: 500,
                    predicted_percent_basis_points: 5_000,
                    predicted_duration_ms: 30_000,
                    requested_target: execution.requested_target,
                    successful_target: execution.requested_target,
                }
            }
        );
        assert_eq!(status.historical, HistoricalLevel::Analyzed);
        assert_eq!(status.analyze, RowEligibility::Eligible);
        assert_eq!(status.convert, RowEligibility::Eligible);
    }

    #[test]
    fn every_profile_field_participates_in_reuse() {
        let state = state_with(verified(&[]));
        let execution = composed(&state, &VideoCodec::H264);
        let mut recorded = Vec::new();
        let mut changed = execution.profile.clone();
        changed.preset = changed.preset.saturating_sub(1);
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.max_encoded_percent_basis_points += 1;
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.samples = Some(4);
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.sample_duration_ms += 1;
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.thorough = !changed.thorough;
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.decode_mode = DecodeMode::Hardware(HardwareDecoder::H264Cuvid);
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.ab_av1_revision.push_str("-new");
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.ffmpeg_revision.push_str("-new");
        recorded.push(changed);
        let mut changed = execution.profile.clone();
        changed.encoder_revision.push_str("-new");
        recorded.push(changed);

        for profile in recorded {
            let mut state = state_with(verified(&[]));
            let content_key = insert_record(&mut state, "movie", hd());
            let mut result = result_for(&execution);
            result.profile = profile;
            record_result(&mut state, &content_key, result);
            let status = scanned_status(&state, &content_key, &hd());
            assert_eq!(
                status.applicable,
                ApplicableLevel::Scanned {
                    reuse: ReuseStanding::Inapplicable
                }
            );
            assert_eq!(status.historical, HistoricalLevel::Analyzed);
        }
    }

    #[test]
    fn target_and_fallback_provenance_control_reuse() {
        let mut state = state_with(verified(&[]));
        let content_key = insert_record(&mut state, "movie", hd());
        let execution = composed(&state, &VideoCodec::H264);
        record_result(&mut state, &content_key, result_for(&execution));

        state.execution.requested_target = VmafTarget(execution.requested_target.0 - 1);
        assert!(matches!(
            scanned_status(&state, &content_key, &hd()).applicable,
            ApplicableLevel::Analyzed { .. }
        ));

        state.execution.requested_target = VmafTarget(execution.requested_target.0 + 1);
        assert_eq!(
            scanned_status(&state, &content_key, &hd()).applicable,
            ApplicableLevel::Scanned {
                reuse: ReuseStanding::Inapplicable
            }
        );

        let mut fallback_state = state_with(verified(&[]));
        let fallback_key = insert_record(&mut fallback_state, "fallback", hd());
        let mut fallback = result_for(&execution);
        fallback.successful_target = VmafTarget(execution.requested_target.0 - 1);
        record_result(&mut fallback_state, &fallback_key, fallback);
        assert!(matches!(
            scanned_status(&fallback_state, &fallback_key, &hd()).applicable,
            ApplicableLevel::Analyzed { .. }
        ));
        fallback_state.execution.fallback_step += 1;
        assert_eq!(
            scanned_status(&fallback_state, &fallback_key, &hd()).applicable,
            ApplicableLevel::Scanned {
                reuse: ReuseStanding::Inapplicable
            }
        );
    }

    #[test]
    fn hardware_decode_reuses_only_the_probed_decoder_profile() {
        let mut state = state_with(verified(&[HardwareDecoder::H264Cuvid]));
        state.settings.hardware_decode = true;
        let content_key = insert_record(&mut state, "movie", hd());
        let execution = composed(&state, &VideoCodec::H264);
        assert_eq!(
            execution.profile.decode_mode,
            DecodeMode::Hardware(HardwareDecoder::H264Cuvid)
        );
        record_result(&mut state, &content_key, result_for(&execution));
        assert!(matches!(
            scanned_status(&state, &content_key, &hd()).applicable,
            ApplicableLevel::Analyzed { .. }
        ));

        state.settings.hardware_decode = false;
        assert_eq!(
            scanned_status(&state, &content_key, &hd()).applicable,
            ApplicableLevel::Scanned {
                reuse: ReuseStanding::Inapplicable
            }
        );
    }

    #[test]
    fn decisive_verdicts_are_converted_and_skip_conversion() {
        let mut state = state_with(verified(&[]));
        let content_key = insert_record(&mut state, "movie", hd());
        set_verdict(&mut state, &content_key, converted_kind());
        let status = scanned_status(&state, &content_key, &hd());
        assert_eq!(
            status.applicable,
            ApplicableLevel::Converted {
                summary: ConversionSummary::Encoded {
                    input_size: Some(1_000),
                    output_size: Some(400),
                    encoding_time: Some(DurationMs(90_000)),
                    crf: Some(Crf(30_000)),
                    vmaf: Some(VmafScore(9_550)),
                    target: Some(VmafTarget(95)),
                    decided_at: UnixMillis(2),
                    source_run: Some(RunId(7)),
                }
            }
        );
        assert_eq!(status.historical, HistoricalLevel::Converted);
        assert_eq!(status.analyze, RowEligibility::Eligible);
        assert_eq!(
            status.convert,
            RowEligibility::Skip {
                reason: SkipReason::ProbableDuplicate {
                    source_run: Some(RunId(7)),
                }
            }
        );

        set_verdict(
            &mut state,
            &content_key,
            VerdictKind::Remuxed {
                output_content_key: key("remux-output"),
                input_size: Some(1_000),
                output_size: Some(900),
            },
        );
        let status = scanned_status(&state, &content_key, &hd());
        assert_eq!(
            status.applicable,
            ApplicableLevel::Converted {
                summary: ConversionSummary::Remuxed {
                    input_size: Some(1_000),
                    output_size: Some(900),
                    decided_at: UnixMillis(2),
                    source_run: Some(RunId(7)),
                }
            }
        );

        // Verdicts stand regardless of tool verification.
        state.tools = located(ToolVerification::Pending);
        assert!(matches!(
            scanned_status(&state, &content_key, &hd()).applicable,
            ApplicableLevel::Converted { .. }
        ));
    }

    #[test]
    fn not_worthwhile_is_history_and_a_floor_policy() {
        let mut state = state_with(verified(&[]));
        let content_key = insert_record(&mut state, "movie", hd());
        set_verdict(
            &mut state,
            &content_key,
            VerdictKind::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
            },
        );
        let status = scanned_status(&state, &content_key, &hd());
        assert_eq!(
            status.applicable,
            ApplicableLevel::Scanned {
                reuse: ReuseStanding::Unanalyzed
            }
        );
        assert_eq!(status.historical, HistoricalLevel::Analyzed);
        assert_eq!(status.analyze, RowEligibility::Eligible);
        assert_eq!(
            status.convert,
            RowEligibility::Skip {
                reason: SkipReason::NotWorthwhile {
                    source_run: Some(RunId(7)),
                }
            }
        );

        // A lower floor is untried ground, decided from the base execution
        // even before the tools are verified.
        state.execution.fallback_floor = VmafTarget(85);
        state.tools = located(ToolVerification::Pending);
        let status = scanned_status(&state, &content_key, &hd());
        assert_eq!(status.convert, RowEligibility::Eligible);
    }

    #[test]
    fn media_eligibility_is_reported_per_operation() {
        let mut state = state_with(verified(&[]));
        let low = meta(VideoCodec::H264, MediaContainer::Matroska, 640, 480);
        let low_key = insert_record(&mut state, "low", low.clone());
        let status = scanned_status(&state, &low_key, &low);
        let expected = RowEligibility::Skip {
            reason: SkipReason::LowResolution {
                pixels: 307_200,
                minimum: crate::policy::MIN_VIDEO_PIXELS,
            },
        };
        assert_eq!(status.analyze, expected);
        assert_eq!(status.convert, expected);

        let av1_mkv = meta(VideoCodec::Av1, MediaContainer::Matroska, 1_920, 1_080);
        let av1_mkv_key = insert_record(&mut state, "av1-mkv", av1_mkv.clone());
        let status = scanned_status(&state, &av1_mkv_key, &av1_mkv);
        let expected = RowEligibility::Skip {
            reason: SkipReason::AlreadyAv1Matroska,
        };
        assert_eq!(status.analyze, expected);
        assert_eq!(status.convert, expected);

        let av1_mp4 = meta(
            VideoCodec::Av1,
            MediaContainer::Other("mov,mp4,m4a".to_owned()),
            1_920,
            1_080,
        );
        let av1_mp4_key = insert_record(&mut state, "av1-mp4", av1_mp4.clone());
        let status = scanned_status(&state, &av1_mp4_key, &av1_mp4);
        assert_eq!(status.analyze, RowEligibility::Eligible);
        assert_eq!(status.convert, RowEligibility::Remux);
    }

    #[test]
    fn imported_provenance_raises_only_historical() {
        let cases = [
            (ParkedStatus::Scanned, HistoricalLevel::Scanned),
            (ParkedStatus::Analyzed, HistoricalLevel::Analyzed),
            (ParkedStatus::NotWorthwhile, HistoricalLevel::Analyzed),
            (ParkedStatus::Converted, HistoricalLevel::Converted),
        ];
        for (status, historical) in cases {
            let mut state = state_with(verified(&[]));
            let content_key = insert_record(&mut state, "movie", hd());
            if let Some(record) = state.durable.records.get_mut(&content_key) {
                record.imported = Some(imported(status));
            }
            let projected = scanned_status(&state, &content_key, &hd());
            assert_eq!(
                projected.applicable,
                ApplicableLevel::Scanned {
                    reuse: ReuseStanding::Unanalyzed
                }
            );
            assert_eq!(projected.historical, historical);
            assert_eq!(projected.convert, RowEligibility::Eligible);
        }
    }

    #[test]
    fn a_settled_output_reports_the_source_conversion_and_its_own_eligibility() {
        let mut state = state_with(verified(&[]));
        let source = insert_record(&mut state, "source", hd());
        set_verdict(&mut state, &source, converted_kind());
        let output_meta = meta(VideoCodec::Av1, MediaContainer::Matroska, 1_920, 1_080);
        let output = insert_record(&mut state, "output", output_meta.clone());
        let status = project_row_status(
            &state,
            ScanFacts::SettledOutput {
                source_content_key: &source,
                output_content_key: &output,
                metadata: Some(&output_meta),
            },
        );
        assert!(matches!(
            status.applicable,
            ApplicableLevel::Converted {
                summary: ConversionSummary::Encoded { .. }
            }
        ));
        assert_eq!(status.historical, HistoricalLevel::Converted);
        let expected = RowEligibility::Skip {
            reason: SkipReason::AlreadyAv1Matroska,
        };
        assert_eq!(status.analyze, expected);
        assert_eq!(status.convert, expected);

        // Without output metadata the claim would fail open, so the row does.
        let unknown = project_row_status(
            &state,
            ScanFacts::SettledOutput {
                source_content_key: &source,
                output_content_key: &output,
                metadata: None,
            },
        );
        assert_eq!(unknown.analyze, RowEligibility::Eligible);
    }

    #[test]
    fn applicable_never_exceeds_historical_and_imports_never_raise_it() {
        fn rank(level: &ApplicableLevel) -> HistoricalLevel {
            match level {
                ApplicableLevel::Scanned { .. } => HistoricalLevel::Scanned,
                ApplicableLevel::Analyzed { .. } => HistoricalLevel::Analyzed,
                ApplicableLevel::Converted { .. } => HistoricalLevel::Converted,
            }
        }
        let verdicts = [
            None,
            Some(converted_kind()),
            Some(VerdictKind::Remuxed {
                output_content_key: key("remux"),
                input_size: None,
                output_size: None,
            }),
            Some(VerdictKind::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
            }),
        ];
        let imports = [
            None,
            Some(ParkedStatus::Scanned),
            Some(ParkedStatus::Analyzed),
            Some(ParkedStatus::NotWorthwhile),
            Some(ParkedStatus::Converted),
        ];
        let toolchains = [verified(&[]), located(ToolVerification::Pending)];
        // 0: no search, 1: a reusable search, 2: a search under another profile.
        for search in 0..3_u8 {
            for verdict in &verdicts {
                for import in imports {
                    for tools in &toolchains {
                        let mut state = state_with(tools.clone());
                        let content_key = insert_record(&mut state, "movie", hd());
                        let mut result = result_for(&ExecutionSettings::default());
                        let reference = state_with(verified(&[]));
                        let execution = composed(&reference, &VideoCodec::H264);
                        match search {
                            1 => result.profile = execution.profile.clone(),
                            2 => result.profile.preset = 1,
                            _ => {}
                        }
                        if search > 0 {
                            record_result(&mut state, &content_key, result);
                        }
                        if let Some(kind) = verdict {
                            set_verdict(&mut state, &content_key, kind.clone());
                        }
                        let without_import = scanned_status(&state, &content_key, &hd());
                        if let (Some(status), Some(record)) =
                            (import, state.durable.records.get_mut(&content_key))
                        {
                            record.imported = Some(imported(status));
                        }
                        let status = scanned_status(&state, &content_key, &hd());
                        assert!(rank(&status.applicable) <= status.historical, "{status:?}");
                        assert_eq!(status.applicable, without_import.applicable);
                        assert_eq!(status.analyze, without_import.analyze);
                        assert_eq!(status.convert, without_import.convert);
                    }
                }
            }
        }
    }

    #[test]
    fn refresh_scope_names_the_content_each_durable_fact_touches() {
        let state = AppState::default();
        let mut applied = Applied {
            durable: Vec::new(),
            config: Vec::new(),
            ephemeral: Vec::new(),
            effects: Vec::new(),
            reply: crate::Reply::Accepted,
        };
        assert_eq!(
            refresh_scope(&state, &applied, false),
            RefreshScope::Nothing
        );
        assert_eq!(
            refresh_scope(&state, &applied, true),
            RefreshScope::Everything
        );
        applied
            .ephemeral
            .push(EphemeralDelta::ToolsChanged(ToolAvailability::default()));
        assert_eq!(
            refresh_scope(&state, &applied, false),
            RefreshScope::Everything
        );
        applied.ephemeral.clear();
        applied.durable.push(DurableDelta::ParkedAdopted {
            import_path: ImportPath("/videos/movie.mkv".to_owned()),
            content_key: key("adopted"),
            imported: imported(ParkedStatus::Analyzed).record,
            verdict: None,
        });
        assert_eq!(
            refresh_scope(&state, &applied, false),
            RefreshScope::Content(BTreeSet::from([key("adopted")]))
        );
        let scope = refresh_scope(&state, &applied, false);
        assert!(scope.covers(&ScanFacts::SettledOutput {
            source_content_key: &key("adopted"),
            output_content_key: &key("other"),
            metadata: None,
        }));
        assert!(!scope.covers(&ScanFacts::Scanned {
            content_key: &key("other"),
            metadata: &hd(),
        }));
    }

    #[test]
    fn a_default_profile_without_revisions_never_matches_a_verified_claim() {
        let profile = AnalysisProfile::production();
        assert!(profile.ab_av1_revision.is_empty());
        let mut state = state_with(verified(&[]));
        let content_key = insert_record(&mut state, "movie", hd());
        record_result(
            &mut state,
            &content_key,
            result_for(&ExecutionSettings::default()),
        );
        assert_eq!(
            scanned_status(&state, &content_key, &hd()).applicable,
            ApplicableLevel::Scanned {
                reuse: ReuseStanding::Inapplicable
            }
        );
    }
}
