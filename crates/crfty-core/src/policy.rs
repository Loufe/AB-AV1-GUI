use serde::{Deserialize, Serialize};

use crate::{
    AnalysisResult, ExecutionSettings, FileRecord, MediaContainer, Operation, VideoCodec, VideoMeta,
};

pub const MIN_VIDEO_PIXELS: u64 = 921_600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    LowResolution { pixels: u64, minimum: u64 },
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
