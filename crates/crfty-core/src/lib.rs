#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

//! Pure domain logic for CRFty.
//!
//! This crate cannot depend on processes, filesystems, clocks, async runtimes,
//! or user-interface frameworks.

mod constants;
mod failure;
mod job;
mod journal;
mod media;
mod output;
mod policy;
mod reducer;
mod settings;
mod state;

/// Export-only override target for `#[specta(type = ...)]` on integers wider
/// than 32 bits. Tauri's JSON transport delivers every integer as a JavaScript
/// `number`, so generated bindings must say `number` — never `bigint`. Values
/// are exact below 2^53; the only fields that can exceed that bound are
/// filesystem-identity internals the frontend treats as opaque and never sends
/// back. Do not use this alias as a runtime type.
pub type JsNumber = u32;

pub use failure::{DIAGNOSTIC_TAIL_MAX_BYTES, DiagnosticTail, FailureFacts, FailureKind};
pub use job::{
    AnalysisAttempt, AnalysisProfile, AnalysisResult, ClaimedJob, Crf, DecodeMode,
    DecodePreference, ExecutionSettings, HardwareDecoder, JobAction, JobPhase, JobSpec, Operation,
    OutputTarget, ReservedJob, SearchMeasurement, ToolRevisions, VmafScore, VmafTarget,
};
pub use journal::{
    JOURNAL_SCHEMA_VERSION, JournalCorruption, JournalEnvelope, JournalReplay, encode_record,
    replay,
};
pub use media::{
    FileRecord, FileStamp, MediaContainer, MediaObservation, PathBinding, PathHash, VideoCodec,
    VideoMeta,
};
pub use output::{
    ArtifactIdentity, ConflictKind, ContentKey, DestructiveIdentity, DestructiveObservation,
    FileSystemFacts, FileSystemId, OutputDelta, OutputRecoveryAction, OutputState,
    OutputTransaction, RecoveryConflict, Replacement, recover_output,
};
pub use policy::{
    Eligibility, MIN_VIDEO_PIXELS, SkipReason, evaluate_eligibility, select_analysis,
    select_job_action,
};
pub use reducer::{
    Applied, Command, Effect, EphemeralDelta, QueueCommand, Reply, SessionCommand, SettingsCommand,
    SystemCommand, WorkerCommand, apply,
};
pub use settings::{
    DEFAULT_OUTPUT_SUFFIX, DefaultOutputMode, OutputSettings, PrivacySettings, Settings,
    VideoExtension,
};
pub use state::{
    AppSnapshot, AppState, ClaimId, ConfigDelta, DurableDelta, DurableState, ItemOutcome,
    JobProgress, JournalSequence, MediaTool, QueueItem, QueueItemId, QueueItemState, RunId,
    SessionState, Telemetry, ToolAvailability, fold, fold_config,
};

#[cfg(test)]
mod tests;
pub use constants::{
    CRF_FIXED_SCALE, DEFAULT_ENCODING_PRESET, DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS,
    DEFAULT_SAMPLE_DURATION_MS, DEFAULT_VMAF_TARGET, MAX_ENCODING_PRESET, MAX_PERCENT_BASIS_POINTS,
    MAX_VMAF_SCORE, MIN_VMAF_FALLBACK_TARGET, PERCENT_BASIS_POINTS_SCALE, VMAF_FALLBACK_STEP,
    VMAF_SCORE_FIXED_SCALE,
};
