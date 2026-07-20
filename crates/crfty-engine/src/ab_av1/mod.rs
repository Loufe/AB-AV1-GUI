//! Isolated adapter for CRFty's pinned ab-av1 revision.

mod operation;
mod runtime;
mod types;

#[cfg(feature = "contract-test-fixture")]
pub use runtime::FaultInjection;
pub use runtime::{AbAv1Runtime, CancellationHandle, JobHandle};
pub use types::{
    CancelMode, EncodeOutcome, EncodeRequest, EncodeTelemetry, JobFailure, JobFailureKind,
    JobReport, JobTerminal, RuntimeStartError, SearchOutcome, SearchRequest, SearchTelemetry,
    SearchWork, ShutdownError, StartJobError, StreamSizes, Telemetry, WaitError,
};
