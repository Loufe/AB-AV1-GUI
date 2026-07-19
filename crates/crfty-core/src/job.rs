use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    ClaimId, DEFAULT_ENCODING_PRESET, DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS,
    DEFAULT_SAMPLE_DURATION_MS, DEFAULT_VMAF_TARGET, MIN_VMAF_FALLBACK_TARGET, QueueItemId, RunId,
    VMAF_FALLBACK_STEP,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Analyze,
    Convert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputTarget {
    Replace,
    Suffix {
        suffix: String,
    },
    SeparateFolder {
        directory: PathBuf,
        source_root: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VmafTarget(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmafScore(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crf(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisProfile {
    pub preset: u8,
    pub max_encoded_percent_basis_points: u32,
    pub samples: Option<u64>,
    pub sample_duration_ms: u64,
    pub thorough: bool,
    pub hardware_decode: bool,
    pub ab_av1_revision: String,
    pub ffmpeg_revision: String,
    pub encoder_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRevisions {
    pub ab_av1: String,
    pub ffmpeg: String,
    pub encoder: String,
}

impl AnalysisProfile {
    #[must_use]
    pub fn production(revisions: ToolRevisions, hardware_decode: bool) -> Self {
        Self {
            preset: DEFAULT_ENCODING_PRESET,
            max_encoded_percent_basis_points: DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS,
            samples: None,
            sample_duration_ms: DEFAULT_SAMPLE_DURATION_MS,
            thorough: false,
            hardware_decode,
            ab_av1_revision: revisions.ab_av1,
            ffmpeg_revision: revisions.ffmpeg,
            encoder_revision: revisions.encoder,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSettings {
    pub requested_target: VmafTarget,
    pub fallback_floor: VmafTarget,
    pub fallback_step: u8,
    pub overwrite_existing: bool,
    pub profile: AnalysisProfile,
}

impl ExecutionSettings {
    #[must_use]
    pub fn production(profile: AnalysisProfile, overwrite_existing: bool) -> Self {
        Self {
            requested_target: DEFAULT_VMAF_TARGET,
            fallback_floor: MIN_VMAF_FALLBACK_TARGET,
            fallback_step: VMAF_FALLBACK_STEP,
            overwrite_existing,
            profile,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMeasurement {
    pub crf: Crf,
    pub score: VmafScore,
    pub predicted_size: u64,
    pub predicted_percent_basis_points: u32,
    pub predicted_duration_ms: u64,
    pub from_cache: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisAttempt {
    pub target: VmafTarget,
    pub last_measurement: Option<SearchMeasurement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub requested_target: VmafTarget,
    pub successful_target: VmafTarget,
    pub fallback_floor: VmafTarget,
    pub fallback_step: u8,
    pub failed_attempts: Vec<AnalysisAttempt>,
    pub measurement: SearchMeasurement,
    pub profile: AnalysisProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    pub item_id: QueueItemId,
    pub claim_id: ClaimId,
    pub run_id: RunId,
    pub input: PathBuf,
    pub operation: Operation,
    pub output_target: OutputTarget,
    pub execution: ExecutionSettings,
    pub selected_analysis: Option<AnalysisResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedJob {
    pub spec: JobSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPhase {
    Preparing,
    Analyzing,
    Encoding,
    Verifying,
    Finalizing,
}
