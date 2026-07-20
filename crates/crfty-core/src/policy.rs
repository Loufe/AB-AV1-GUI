use serde::{Deserialize, Serialize};

use crate::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, DecodeMode, DestructiveIdentity,
    ExecutionSettings, FileRecord, FileStamp, JobAction, MediaContainer, Operation, Verdict,
    VerdictKind, VideoCodec, VideoMeta,
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
