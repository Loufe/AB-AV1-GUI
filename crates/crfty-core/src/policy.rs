use serde::{Deserialize, Serialize};

use crate::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, DEFAULT_VMAF_TARGET, DecodeMode,
    DestructiveIdentity, ExecutionSettings, FileRecord, FileStamp, IMPORT_MTIME_TOLERANCE_NS,
    JobAction, MIN_VMAF_FALLBACK_TARGET, MediaContainer, MediaObservation, Operation, ParkedRecord,
    ParkedStatus, Verdict, VerdictKind, VideoCodec, VideoMeta,
};

pub const MIN_VIDEO_PIXELS: u64 = 921_600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum SkipReason {
    LowResolution {
        #[specta(type = crate::JsNumber)]
        pixels: u64,
        #[specta(type = crate::JsNumber)]
        minimum: u64,
    },
    AlreadyAv1Matroska,
    OutputExists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    Process,
    Remux,
    Skip(SkipReason),
}

#[must_use]
pub fn evaluate_eligibility(metadata: &VideoMeta, operation: Operation) -> Eligibility {
    let pixels = metadata.post_rotation_pixels();
    if pixels < MIN_VIDEO_PIXELS {
        return Eligibility::Skip(SkipReason::LowResolution {
            pixels,
            minimum: MIN_VIDEO_PIXELS,
        });
    }
    if metadata.codec == VideoCodec::Av1 {
        if metadata.container == MediaContainer::Matroska {
            return Eligibility::Skip(SkipReason::AlreadyAv1Matroska);
        }
        if operation == Operation::Convert {
            return Eligibility::Remux;
        }
    }
    Eligibility::Process
}

#[must_use]
pub fn select_analysis(
    record: &FileRecord,
    execution: &ExecutionSettings,
) -> Option<AnalysisResult> {
    let analyses = record.analyses.get(&execution.profile)?;
    analyses
        .range(execution.requested_target..)
        .find_map(|(_, result)| {
            result
                .validate_reusable_for(execution)
                .is_ok()
                .then(|| result.clone())
        })
        .or_else(|| {
            analyses.values().rev().find_map(|result| {
                let same_fallback_request = result.requested_target == execution.requested_target
                    && result.fallback_floor == execution.fallback_floor
                    && result.fallback_step == execution.fallback_step;
                (same_fallback_request && result.validate_reusable_for(execution).is_ok())
                    .then(|| result.clone())
            })
        })
}

/// Whether a decided verdict still describes the file that would be enqueued,
/// without ffprobe.
///
/// After a replace-mode conversion settles, `state.paths` still holds the OLD
/// input binding (only `MediaObserved` writes path bindings, and core cannot
/// hash paths), so recognition of the output-at-the-input-path CANNOT come
/// from the binding. It comes from the settled output identity instead:
/// `settled_output` is the final identity from
/// `outputs[verdict.source_run].state` (Committed or Retired), and
/// `current_stamp` is a cheap stat of the file now at the candidate path.
///
/// - `Converted`/`Remuxed` apply while the file on disk IS the produced
///   output: stamp equality (size and modification time, both or neither
///   known) against the settled identity. A missing file, a changed file, or
///   a pruned/unsettled transaction means the verdict no longer answers for
///   the path.
/// - `NotWorthwhile` is a judgment about input content. Callers resolve the
///   record by `ContentKey`, so content match is already established; the
///   verdict applies regardless of path state. Whether its targets satisfy a
///   NEW request is `select_analysis`/target policy, not freshness.
///
/// An adopted verdict (`source_run: None`) has no transaction to resolve, so
/// callers pass `settled_output: None`: its Converted form never applies
/// (the imported output was never content-hashed, so the file on disk cannot
/// be proven to be it), while NotWorthwhile applies by content identity as
/// usual.
#[must_use]
pub fn verdict_applies(
    verdict: &Verdict,
    settled_output: Option<&DestructiveIdentity>,
    current_stamp: Option<&FileStamp>,
) -> bool {
    match &verdict.kind {
        VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. } => {
            let (Some(identity), Some(stamp)) = (settled_output, current_stamp) else {
                return false;
            };
            identity.size == stamp.size && identity.modified_ns == stamp.modified_ns
        }
        VerdictKind::NotWorthwhile { .. } => true,
    }
}

/// What becomes of a parked import record once its file is actually
/// observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParkedResolution {
    /// The record describes this content. `verdict: None` for records that
    /// decided nothing (scanned/analyzed) — the file still gains import
    /// provenance and the parked entry retires.
    Adopt { verdict: Option<Verdict> },
    /// The record no longer describes the file at its path.
    Retire,
}

/// Decide a parked import record against a fresh observation of the file at
/// the path the record was keyed under:
///
/// - Stamp match (equal size, mtime within [`IMPORT_MTIME_TOLERANCE_NS`];
///   a record missing either stamp half never matches): the record describes
///   the observed content — decisive statuses adopt their verdict, scanned/
///   analyzed adopt provenance only.
/// - Stamp mismatch, status Converted, observed codec AV1: the replace-mode
///   case — the file now at the path IS the conversion's output, so the
///   Converted verdict adopts onto the observed (output) content.
/// - Anything else: the content changed; the imported claim retires.
///
/// Adopted verdicts carry `source_run: None` and whatever summary fields the
/// imported record supplied. Missing NotWorthwhile targets fall back to
/// [`DEFAULT_VMAF_TARGET`] / [`MIN_VMAF_FALLBACK_TARGET`].
#[must_use]
pub fn resolve_parked(parked: &ParkedRecord, observation: &MediaObservation) -> ParkedResolution {
    if parked_stamp_matches(parked, &observation.binding.stamp) {
        let verdict = match parked.status {
            ParkedStatus::Converted => Some(converted_verdict(parked)),
            ParkedStatus::NotWorthwhile => Some(not_worthwhile_verdict(parked)),
            ParkedStatus::Scanned | ParkedStatus::Analyzed => None,
        };
        return ParkedResolution::Adopt { verdict };
    }
    if parked.status == ParkedStatus::Converted && observation.metadata.codec == VideoCodec::Av1 {
        return ParkedResolution::Adopt {
            verdict: Some(converted_verdict(parked)),
        };
    }
    ParkedResolution::Retire
}

fn parked_stamp_matches(parked: &ParkedRecord, current: &FileStamp) -> bool {
    let (Some(size), Some(modified)) = (parked.size, parked.modified_ns) else {
        return false;
    };
    if size != current.size {
        return false;
    }
    current
        .modified_ns
        .is_some_and(|now| now.0.abs_diff(modified.0) <= IMPORT_MTIME_TOLERANCE_NS)
}

fn converted_verdict(parked: &ParkedRecord) -> Verdict {
    Verdict {
        kind: VerdictKind::Converted {
            output_content_key: None,
            input_size: parked.size,
            output_size: parked.output_size,
            encoding_time: parked.encoding_time,
            crf: parked.crf,
            vmaf: parked.vmaf,
            target: parked.target,
        },
        source_run: None,
        decided_at: parked.decided_at,
    }
}

fn not_worthwhile_verdict(parked: &ParkedRecord) -> Verdict {
    Verdict {
        kind: VerdictKind::NotWorthwhile {
            requested: parked.requested_target.unwrap_or(DEFAULT_VMAF_TARGET),
            floor: parked.floor_target.unwrap_or(MIN_VMAF_FALLBACK_TARGET),
        },
        source_run: None,
        decided_at: parked.decided_at,
    }
}

/// The analysis profiles a run may durably record: the prepared profile
/// itself, plus its software-decode variant when the prepared profile decodes
/// in hardware. This is the gate behind the hardware→software retry ladder —
/// a search that fails under hardware decode retries once with software and
/// records under the profile it actually ran with, while the requested
/// `JobSpec` is never rewritten. A software-prepared run has no wider ladder.
#[must_use]
pub fn permitted_profiles(execution: &ExecutionSettings) -> Vec<AnalysisProfile> {
    let mut profiles = vec![execution.profile.clone()];
    if matches!(execution.profile.decode_mode, DecodeMode::Hardware(_)) {
        let mut software = execution.profile.clone();
        software.decode_mode = DecodeMode::Software;
        profiles.push(software);
    }
    profiles
}

#[must_use]
pub fn select_job_action(
    metadata: Option<&VideoMeta>,
    record: Option<&FileRecord>,
    operation: Operation,
    intent: AnalysisIntent,
    execution: &ExecutionSettings,
) -> JobAction {
    if let Some(metadata) = metadata {
        match evaluate_eligibility(metadata, operation) {
            Eligibility::Remux => return JobAction::Remux,
            Eligibility::Skip(reason) => return JobAction::Skip { reason },
            Eligibility::Process => {}
        }
    }

    let selected_analysis = match intent {
        AnalysisIntent::ReuseIfFresh => record
            .and_then(|known| select_analysis(known, execution))
            .map(Box::new),
        AnalysisIntent::Refresh => None,
    };
    match operation {
        Operation::Analyze => JobAction::Analyze { selected_analysis },
        Operation::Convert => JobAction::Encode { selected_analysis },
    }
}
