//! Official ab-av1 CLI search and directly supervised FFmpeg encoding.

pub(crate) mod operation;
mod protocol;
mod runtime;
mod tool_path;
mod types;

#[cfg(feature = "contract-test-fixture")]
pub use runtime::FaultInjection;
pub use runtime::{AbAv1Runtime, CancellationHandle, JobHandle};
pub use types::{
    CancelMode, EncodeRequest, EncodeTelemetry, JobFailure, JobFailureKind, JobReport, JobTerminal,
    ProcessFailureKind, RuntimeStartError, SearchOutcome, SearchRequest, SearchTelemetry,
    ShutdownError, StartJobError, Telemetry, WaitError,
};
