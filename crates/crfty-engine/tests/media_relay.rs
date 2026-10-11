#![forbid(unsafe_code)]
#![cfg(feature = "contract-test-fixture")]
use crfty_engine::process_supervisor::{ProcessCancellation, ProcessLimits, ProcessTerminal, run};
use std::{fs, process::Command, time::Duration};

#[test]
fn relay_preserves_tool_output_and_exit_status() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let alias = directory.path().join(if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    });
    fs::copy(env!("CARGO_BIN_EXE_crfty-media-relay"), &alias)?;
    let report = run(
        Command::new(alias).arg("--relay-exit").env(
            "CRFTY_RELAY_FFMPEG",
            env!("CARGO_BIN_EXE_crfty-contract-fixture"),
        ),
        &ProcessCancellation::new(),
        ProcessLimits::new(Some(Duration::from_secs(10)), 1024, 1024),
    );
    assert!(
        matches!(report.terminal, ProcessTerminal::ToolFailed(status) if status.code() == Some(37)),
        "{:?}",
        report.terminal
    );
    assert_eq!(report.stdout.as_bytes(), b"relay stdout\n");
    assert_eq!(report.stderr_tail.as_bytes(), b"relay stderr\n");
    Ok(())
}
