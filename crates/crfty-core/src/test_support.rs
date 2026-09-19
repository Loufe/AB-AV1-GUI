//! Builders for durable-state fixtures shared by the projection and
//! observation tests, so both derive from the same run shapes.

use std::path::PathBuf;

use crate::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, AudioCodec, AudioStreamMeta, ClaimId,
    CompletionEvidence, ContentKey, ConversionRun, Crf, DecodeMode, DestructiveIdentity,
    DurationMs, ExecutionSettings, FileSystemId, ItemOutcome, JobAction, JobPhase, JobSpec,
    MediaContainer, Operation, OutputTarget, PhaseSpan, QueueItemId, RunId, SearchMeasurement,
    UnixMillis, VideoCodec, VideoMeta, VmafScore, VmafTarget,
};

pub(crate) fn key(name: &str) -> ContentKey {
    ContentKey(name.to_owned())
}

pub(crate) fn meta(codec: VideoCodec, size_bytes: u64) -> VideoMeta {
    VideoMeta {
        codec,
        container: MediaContainer::Matroska,
        width: 1920,
        height: 1080,
        rotation_degrees: 0,
        duration_ms: 600_000,
        size_bytes,
        audio: vec![AudioStreamMeta {
            codec: AudioCodec::Aac,
            channels: 2,
        }],
        subtitle_count: 0,
    }
}

pub(crate) fn spec(run: u64, content_key: &ContentKey, operation: Operation) -> JobSpec {
    JobSpec {
        item_id: QueueItemId(run),
        claim_id: ClaimId(run),
        run_id: RunId(run),
        input: PathBuf::from(format!("input-{run}.mkv")),
        content_key: Some(content_key.clone()),
        operation,
        intent: AnalysisIntent::ReuseIfFresh,
        output_target: OutputTarget::Suffix {
            suffix: "-av1".to_owned(),
        },
        execution: ExecutionSettings::production(AnalysisProfile::production(), false),
        action: JobAction::Encode {
            selected_analysis: None,
        },
    }
}

pub(crate) fn analysis(crf_milli: u32, score_centi: u16) -> AnalysisResult {
    AnalysisResult {
        requested_target: VmafTarget(95),
        successful_target: VmafTarget(95),
        fallback_floor: VmafTarget(90),
        fallback_step: 1,
        failed_attempts: Vec::new(),
        measurement: SearchMeasurement {
            crf: Crf(crf_milli),
            score: VmafScore(score_centi),
            predicted_size: 1_000,
            predicted_percent_basis_points: 5_000,
            predicted_duration_ms: 60_000,
            from_cache: false,
        },
        profile: AnalysisProfile::production(),
    }
}

/// A converted-shaped run with an analysis and both phase spans; callers
/// override the outcome and, where the outcome demands it, the analysis.
pub(crate) fn finished_run(
    run: u64,
    content_key: &ContentKey,
    outcome: ItemOutcome,
    finished_at: UnixMillis,
) -> ConversionRun {
    ConversionRun {
        spec: spec(run, content_key, Operation::Convert),
        analysis: Some(analysis(24_000, 9_512)),
        output_content_key: None,
        outcome: Some(outcome),
        started_at: Some(UnixMillis(finished_at.0.saturating_sub(1_000))),
        finished_at: Some(finished_at),
        phase_spans: vec![
            PhaseSpan {
                phase: JobPhase::Analyzing,
                duration: DurationMs(60_000),
            },
            PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(240_000),
            },
        ],
    }
}

pub(crate) fn live(input_size: u64, output_size: u64) -> CompletionEvidence {
    CompletionEvidence::LiveEncode {
        input_size,
        output_size,
        encode_decode: DecodeMode::Software,
    }
}

pub(crate) fn identity(size: u64) -> DestructiveIdentity {
    DestructiveIdentity {
        file_id: FileSystemId::Unix {
            device: 1,
            inode: size,
        },
        size,
        modified_ns: None,
    }
}
