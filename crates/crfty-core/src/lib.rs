#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)
)]

//! Pure domain logic for CRFty.
//!
//! This crate cannot depend on processes, filesystems, clocks, async runtimes,
//! or user-interface frameworks.

mod journal;
mod output;
mod reducer;
mod state;

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
