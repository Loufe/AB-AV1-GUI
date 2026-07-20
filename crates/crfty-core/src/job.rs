use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    ClaimId, DEFAULT_ENCODING_PRESET, DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS,
    DEFAULT_SAMPLE_DURATION_MS, DEFAULT_VMAF_TARGET, MAX_ENCODING_PRESET, MAX_VMAF_SCORE,
    MIN_VMAF_FALLBACK_TARGET, QueueItemId, RunId, VMAF_FALLBACK_STEP, VMAF_SCORE_FIXED_SCALE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum Operation {
    Analyze,
    Convert,
}

/// Whether a queued item may reuse a durable analysis that already satisfies
/// its execution settings. `Refresh` forces a fresh search even when a cached
/// result qualifies; the new result still lands in the record's analysis
/// index like any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum AnalysisIntent {
    ReuseIfFresh,
    Refresh,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub enum DecodePreference {
    SoftwareOnly,
    HardwarePreferred,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub enum HardwareDecoder {
    H264Cuvid,
    H264Qsv,
    HevcCuvid,
    HevcQsv,
    Vp9Cuvid,
    Vp9Qsv,
    Av1Cuvid,
    Av1Qsv,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub enum DecodeMode {
    Software,
    Hardware(HardwareDecoder),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub struct VmafTarget(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct VmafScore(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Crf(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type)]
pub struct AnalysisProfile {
    pub preset: u8,
    pub max_encoded_percent_basis_points: u32,
    #[specta(type = Option<crate::JsNumber>)]
    pub samples: Option<u64>,
    #[specta(type = crate::JsNumber)]
    pub sample_duration_ms: u64,
    pub thorough: bool,
    /// The decode mode the search actually ran with — part of the analysis
    /// identity (ADR-007, per #33 §7): hardware and software decoders can
    /// produce different decoded frames, so their VMAF measurements are not
    /// interchangeable. The pin is decoder-granular: a Cuvid-recorded
    /// analysis is not returned for a Qsv execution and re-searches instead.
    /// Requested-vs-actual provenance lives on the run (`decode_preference`,
    /// this field, and the terminal evidence's `encode_decode`).
    pub decode_mode: DecodeMode,
    pub ab_av1_revision: String,
    pub ffmpeg_revision: String,
    pub encoder_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ToolRevisions {
    pub ab_av1: String,
    pub ffmpeg: String,
    pub encoder: String,
}

impl AnalysisProfile {
    /// Production base profile. Tool revisions start empty: the engine
    /// composes the discovered revisions in when it prepares a claim, so a
    /// base profile satisfies [`Self::validate_base`] but not
    /// [`Self::validate`].
    #[must_use]
    pub fn production() -> Self {
        Self {
            preset: DEFAULT_ENCODING_PRESET,
            max_encoded_percent_basis_points: DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS,
            samples: None,
            sample_duration_ms: DEFAULT_SAMPLE_DURATION_MS,
            thorough: false,
            decode_mode: DecodeMode::Software,
            ab_av1_revision: String::new(),
            ffmpeg_revision: String::new(),
            encoder_revision: String::new(),
        }
    }

    /// Structural checks that a base profile must satisfy before tool
    /// revisions are composed in at claim time.
    pub fn validate_base(&self) -> Result<(), &'static str> {
        if self.preset > MAX_ENCODING_PRESET {
            return Err("encoding preset is outside the supported range");
        }
        if self.max_encoded_percent_basis_points == 0 {
            return Err("maximum encoded percent must be positive");
        }
        if self.samples == Some(0) {
            return Err("sample count must be positive when specified");
        }
        if self.sample_duration_ms == 0 {
            return Err("sample duration must be positive");
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_base()?;
        if self.ab_av1_revision.is_empty()
            || self.ffmpeg_revision.is_empty()
            || self.encoder_revision.is_empty()
        {
            return Err("tool revisions must not be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ExecutionSettings {
    pub requested_target: VmafTarget,
    pub fallback_floor: VmafTarget,
    pub fallback_step: u8,
    pub overwrite_existing: bool,
    pub decode_preference: DecodePreference,
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
            decode_preference: DecodePreference::HardwarePreferred,
            profile,
        }
    }

    /// Validates everything except the profile's tool revisions, which the
    /// engine composes in at claim time.
    pub fn validate_base(&self) -> Result<(), &'static str> {
        if u16::from(self.requested_target.0) > MAX_VMAF_SCORE
            || u16::from(self.fallback_floor.0) > MAX_VMAF_SCORE
        {
            return Err("VMAF targets must be in 0..=100");
        }
        if self.fallback_floor > self.requested_target {
            return Err("VMAF fallback floor exceeds the requested target");
        }
        if self.fallback_step == 0 {
            return Err("VMAF fallback step must be positive");
        }
        self.profile.validate_base()
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        self.validate_base()?;
        self.profile.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SearchMeasurement {
    pub crf: Crf,
    pub score: VmafScore,
    #[specta(type = crate::JsNumber)]
    pub predicted_size: u64,
    pub predicted_percent_basis_points: u32,
    #[specta(type = crate::JsNumber)]
    pub predicted_duration_ms: u64,
    pub from_cache: bool,
}

impl SearchMeasurement {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.score.0 > MAX_VMAF_SCORE.saturating_mul(VMAF_SCORE_FIXED_SCALE) {
            return Err("VMAF score is outside the supported range");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AnalysisAttempt {
    pub target: VmafTarget,
    pub last_measurement: Option<SearchMeasurement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AnalysisResult {
    pub requested_target: VmafTarget,
    pub successful_target: VmafTarget,
    pub fallback_floor: VmafTarget,
    pub fallback_step: u8,
    pub failed_attempts: Vec<AnalysisAttempt>,
    pub measurement: SearchMeasurement,
    pub profile: AnalysisProfile,
}

impl AnalysisResult {
    pub fn validate_for(&self, execution: &ExecutionSettings) -> Result<(), &'static str> {
        if self.requested_target != execution.requested_target
            || self.fallback_floor != execution.fallback_floor
            || self.fallback_step != execution.fallback_step
            || !crate::permitted_profiles(execution).contains(&self.profile)
        {
            return Err("analysis provenance does not match the claimed job");
        }
        if self.successful_target > self.requested_target
            || self.successful_target < self.fallback_floor
        {
            return Err("successful VMAF target is outside the requested fallback range");
        }
        if self
            .failed_attempts
            .iter()
            .any(|attempt| attempt.target <= self.successful_target)
        {
            return Err("failed analysis attempts are inconsistent with the successful target");
        }
        self.measurement.validate()
    }

    pub fn validate_reusable_for(&self, execution: &ExecutionSettings) -> Result<(), &'static str> {
        if self.profile != execution.profile {
            return Err("analysis profile does not match the claimed job");
        }
        let satisfies_requested_target = self.successful_target >= execution.requested_target;
        let repeats_same_fallback_request = self.requested_target == execution.requested_target
            && self.fallback_floor == execution.fallback_floor
            && self.fallback_step == execution.fallback_step;
        if !satisfies_requested_target && !repeats_same_fallback_request {
            return Err("analysis target and fallback provenance do not satisfy the claimed job");
        }
        if self.successful_target > self.requested_target
            || self.successful_target < self.fallback_floor
            || self.fallback_step == 0
            || self
                .failed_attempts
                .iter()
                .any(|attempt| attempt.target <= self.successful_target)
        {
            return Err("reused analysis has invalid fallback provenance");
        }
        self.measurement.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum JobAction {
    Analyze {
        selected_analysis: Option<Box<AnalysisResult>>,
    },
    Encode {
        selected_analysis: Option<Box<AnalysisResult>>,
    },
    Remux,
    Skip {
        reason: crate::SkipReason,
    },
}

impl JobAction {
    #[must_use]
    pub fn selected_analysis(&self) -> Option<&AnalysisResult> {
        match self {
            Self::Analyze { selected_analysis } | Self::Encode { selected_analysis } => {
                selected_analysis.as_deref()
            }
            Self::Remux | Self::Skip { .. } => None,
        }
    }

    #[must_use]
    pub const fn produces_output(&self) -> bool {
        matches!(self, Self::Encode { .. } | Self::Remux)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct JobSpec {
    pub item_id: QueueItemId,
    pub claim_id: ClaimId,
    pub run_id: RunId,
    pub input: PathBuf,
    pub content_key: Option<crate::ContentKey>,
    pub operation: Operation,
    pub intent: AnalysisIntent,
    pub output_target: OutputTarget,
    pub execution: ExecutionSettings,
    pub action: JobAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ReservedJob {
    pub item_id: QueueItemId,
    pub claim_id: ClaimId,
    pub run_id: RunId,
    pub input: PathBuf,
    pub operation: Operation,
    pub intent: AnalysisIntent,
    pub output_target: OutputTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ClaimedJob {
    pub spec: JobSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum JobPhase {
    Preparing,
    Analyzing,
    Encoding,
    Verifying,
    Finalizing,
}
