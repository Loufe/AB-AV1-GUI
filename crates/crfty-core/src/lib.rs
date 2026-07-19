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
mod job;
mod journal;
mod output;
mod reducer;
mod state;

pub use job::{
    AnalysisAttempt, AnalysisProfile, AnalysisResult, ClaimedJob, Crf, ExecutionSettings, JobPhase,
    JobSpec, Operation, OutputTarget, SearchMeasurement, ToolRevisions, VmafScore, VmafTarget,
};
pub use journal::{
    JOURNAL_SCHEMA_VERSION, JournalCorruption, JournalEnvelope, JournalReplay, encode_record,
    replay,
};
pub use output::{
    ArtifactIdentity, ArtifactObservation, ContentKey, FileSystemFacts, OutputDelta,
    OutputRecoveryAction, OutputState, OutputTransaction, RecoveryConflict, Replacement,
    recover_output,
};
pub use reducer::{
    Applied, Command, Effect, EphemeralDelta, QueueCommand, Reply, SessionCommand, SystemCommand,
    WorkerCommand, apply,
};
pub use state::{
    AppState, ClaimId, DurableDelta, DurableState, ItemOutcome, JournalSequence, QueueItem,
    QueueItemId, QueueItemState, RunId, SessionState, Telemetry, fold,
};

#[cfg(test)]
mod tests;
pub use constants::{
    CRF_FIXED_SCALE, DEFAULT_ENCODING_PRESET, DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS,
    DEFAULT_SAMPLE_DURATION_MS, DEFAULT_VMAF_TARGET, MAX_VMAF_SCORE, MIN_VMAF_FALLBACK_TARGET,
    PERCENT_BASIS_POINTS_SCALE, VMAF_FALLBACK_STEP, VMAF_SCORE_FIXED_SCALE,
};
