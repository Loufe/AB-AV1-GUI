//! Isolated adapter for CRFty's pinned ab-av1 revision.

mod operation;
mod runtime;
mod types;

/// The ab-av1 revision compiled into this binary, recorded as analysis
/// provenance. A unit test pins it to the workspace lockfile so the string
/// cannot drift from the dependency it describes.
pub const AB_AV1_REVISION: &str = "8bde51723f6f95945792f58a94c08f59171047d7";

#[cfg(feature = "contract-test-fixture")]
pub use runtime::FaultInjection;
pub use runtime::{AbAv1Runtime, CancellationHandle, JobHandle};
pub use types::{
    CancelMode, EncodeOutcome, EncodeRequest, EncodeTelemetry, JobFailure, JobFailureKind,
    JobReport, JobTerminal, RuntimeStartError, SearchOutcome, SearchRequest, SearchTelemetry,
    SearchWork, ShutdownError, StartJobError, StreamSizes, Telemetry, WaitError,
};

#[cfg(test)]
mod tests {
    use super::AB_AV1_REVISION;

    #[test]
    fn pinned_revision_matches_the_workspace_lockfile() {
        let lockfile = include_str!("../../../../Cargo.lock");
        assert!(
            lockfile.contains(&format!("?rev={AB_AV1_REVISION}#{AB_AV1_REVISION}")),
            "AB_AV1_REVISION does not match the locked ab-av1 dependency"
        );
    }
}
