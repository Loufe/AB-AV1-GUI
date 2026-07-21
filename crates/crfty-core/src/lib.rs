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
pub type JsNumber = u32;

pub use analysis::{
    AnalysisActivity, AnalysisCommand, AnalysisDelta, AnalysisDisplayText, AnalysisEntryKind,
    AnalysisGeneration, AnalysisGenerationId, AnalysisLevel, AnalysisLevelAssessment,
    AnalysisMutationError, AnalysisRow, AnalysisRowId, AnalysisRowRef, AnalysisSnapshot,
    CurrentFileIdentity, FreshnessDecision, FreshnessReason, ObservationStability,
    TimestampReliability, apply_analysis_mutation, assess_analysis_levels,
    begin_analysis_generation, decide_freshness, fold_analysis, observation_stability,
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
    COMPACTION_HARD_LIMIT_BYTES, COMPACTION_IDLE_MIN_JOURNAL_BYTES, COMPACTION_IDLE_MIN_RATIO,
    CorruptionReport, CorruptionSignature, JOURNAL_SCHEMA_VERSION, JournalCorruption,
    JournalEnvelope, JournalReplay, JournalSnapshot, compaction_due, compaction_quiescent,
    corruption_signature, encode_record, encode_snapshot, replay,
};
pub use media::{
    AudioCodec, AudioStreamMeta, FileRecord, FileStamp, ImportPath, ImportedHistoryRecord,
    ImportedProvenance, MediaContainer, MediaObservation, ParkedStatus, PathBinding, PathHash,
    Verdict, VerdictKind, VideoCodec, VideoMeta,
};
pub use output::{
    ArtifactIdentity, ConflictKind, ContentKey, DestructiveIdentity, DestructiveObservation,
    FileSystemFacts, FileSystemId, OutputDelta, OutputRecoveryAction, OutputState,
    OutputTransaction, RecoveryConflict, Replacement, recover_output,
};
pub use policy::{
    Eligibility, MIN_VIDEO_PIXELS, ParkedResolution, SkipReason, evaluate_eligibility,
    evaluate_enqueue, permitted_profiles, resolve_parked, select_analysis, select_job_action,
    settled_output_identity, verdict_applies,
};
pub use projection::{
    CodecCount, CumulativeSavingsPoint, HistoryRow, HistoryRowKey, HistoryStatus, RunTotals,
    StatFact, StatFactKind, StatisticsPayload, ValueSpread, collect_stat_facts, history_rows,
    local_epoch_day, statistics,
};
pub use reducer::{
    Applied, Command, Effect, EphemeralDelta, HistoryCommand, ProjectionCommand, QueueAddRequest,
    QueueCommand, QueueItemEdit, Reply, SessionCommand, SettingsCommand, SystemCommand,
    VendorCommand, WorkerCommand, apply,
};
pub use settings::{
    DEFAULT_OUTPUT_SUFFIX, DefaultOutputMode, OutputSettings, PrivacySettings, Settings,
    VideoExtension,
};
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
    CRF_FIXED_SCALE, DEFAULT_ENCODING_PRESET, DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS,
    DEFAULT_SAMPLE_DURATION_MS, DEFAULT_VMAF_TARGET, IMPORT_MTIME_TOLERANCE_NS,
    MAX_ENCODING_PRESET, MAX_PERCENT_BASIS_POINTS, MAX_VMAF_SCORE, MIN_VMAF_FALLBACK_TARGET,
    PERCENT_BASIS_POINTS_SCALE, VMAF_FALLBACK_STEP, VMAF_SCORE_FIXED_SCALE,
};
