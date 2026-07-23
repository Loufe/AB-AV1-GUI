#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

//! Pure domain logic for CRFty.
//!
//! This crate cannot depend on processes, filesystems, clocks, async runtimes,
//! or user-interface frameworks.

mod analysis;
mod constants;
mod estimation;
mod failure;
mod history;
mod job;
mod journal;
mod media;
mod output;
mod policy;
mod projection;
mod reducer;
mod settings;
mod state;
mod time;

/// Export-only override target for `#[specta(type = ...)]` on integers wider
/// than 32 bits. Tauri's JSON transport delivers every integer as a JavaScript
/// `number`, so generated bindings must say `number` — never `bigint`. Values
/// are exact below 2^53; the only fields that can exceed that bound are
/// filesystem-identity internals the frontend treats as opaque and never sends
/// back. Do not use this alias as a runtime type.
pub(crate) type JsNumber = u32;

pub use analysis::{
    AnalysisActivity, AnalysisCommand, AnalysisDelta, AnalysisDiagnosticTail,
    AnalysisDirectoryFailure, AnalysisDisplayText, AnalysisFileScan, AnalysisGeneration,
    AnalysisGenerationId, AnalysisLevel, AnalysisLevelAssessment, AnalysisRow, AnalysisRowEntry,
    AnalysisRowId, AnalysisRowRef, AnalysisScanFailure, AnalysisSnapshot, BasicScanDisposition,
    CurrentFileIdentity, FreshnessReason, ObservationStability, TimestampReliability,
    assess_analysis_levels, fold_analysis, observation_stability,
};
pub(crate) use analysis::{
    AnalysisMutationError, FreshnessDecision, apply_analysis_mutation, begin_analysis_generation,
    decide_freshness,
};
pub use estimation::{
    EstimateBasis, EstimateConfidence, EstimationModel, HistoricalTier, Quartiles,
    ResolutionBucket, TimeEstimate, exclusive_quartiles,
};
pub use failure::{DIAGNOSTIC_TAIL_MAX_BYTES, DiagnosticTail, FailureFacts, FailureKind};
pub use job::{
    AnalysisAttempt, AnalysisIntent, AnalysisProfile, AnalysisResult, ClaimedJob, Crf, DecodeMode,
    DecodePreference, ExecutionSettings, HardwareDecoder, JobAction, JobPhase, JobSpec, Operation,
    OutputTarget, OverwriteDecision, ReservedJob, SearchMeasurement, ToolRevisions, VmafScore,
    VmafTarget,
};
pub use journal::{
    COMPACTION_IDLE_MIN_JOURNAL_BYTES, CorruptionReport, CorruptionSignature, JournalEnvelope,
    JournalReplay, compaction_due, compaction_quiescent, corruption_signature, encode_record,
    encode_snapshot, replay,
};
pub(crate) use media::FileStamp;
pub use media::{
    AudioCodec, AudioStreamMeta, FileRecord, ImportPath, ImportedHistoryRecord, ImportedProvenance,
    MediaContainer, MediaObservation, ParkedStatus, PathBinding, PathHash, Verdict, VerdictKind,
    VideoCodec, VideoMeta,
};
pub use output::{
    ArtifactIdentity, ConflictKind, ContentKey, DestructiveIdentity, DestructiveObservation,
    FileSystemFacts, FileSystemId, OutputDelta, OutputRecoveryAction, OutputState,
    OutputTransaction, Replacement, recover_output,
};
pub(crate) use policy::{
    ParkedResolution, evaluate_enqueue, resolve_parked, select_analysis, select_job_action,
};
pub use policy::{SkipReason, permitted_profiles, verdict_applies};
pub use projection::{HistoryRow, history_rows};
pub(crate) use projection::{StatisticsPayload, collect_stat_facts, statistics};
pub use reducer::{
    Applied, Command, Effect, EphemeralDelta, HistoryCommand, ProjectionCommand, QueueAddRequest,
    QueueCommand, QueueItemEdit, Reply, SessionCommand, SettingsCommand, SystemCommand,
    VendorCommand, WorkerCommand, apply,
};
pub use settings::{DefaultOutputMode, Settings, VideoExtension};
pub use state::{
    AppSnapshot, AppState, ClaimId, CompletionEvidence, ConfigDelta, ConversionRun, DurableDelta,
    DurableState, ItemOutcome, JobProgress, JournalSequence, MediaTool, PhaseSpan, QueueItem,
    QueueItemId, QueueItemState, RunId, SessionAggregates, SessionState, StreamByteSizes,
    Telemetry, ToolAvailability, ToolSource, ToolsState, VendorActivity, fold, fold_config,
};
pub use time::{DurationMs, FileTimeNs, UnixMillis};

#[cfg(test)]
mod tests;
pub use constants::{
    CRF_FIXED_SCALE, DEFAULT_VMAF_TARGET, MAX_ENCODING_PRESET, MAX_PERCENT_BASIS_POINTS,
    MAX_VMAF_SCORE, MIN_VMAF_FALLBACK_TARGET, PERCENT_BASIS_POINTS_SCALE, VMAF_FALLBACK_STEP,
    VMAF_SCORE_FIXED_SCALE,
};
pub(crate) use constants::{
    DEFAULT_ENCODING_PRESET, DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS, DEFAULT_SAMPLE_DURATION_MS,
    IMPORT_MTIME_TOLERANCE_NS,
};
