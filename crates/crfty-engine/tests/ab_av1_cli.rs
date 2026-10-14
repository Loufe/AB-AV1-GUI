#![forbid(unsafe_code)]
#![cfg(feature = "contract-test-fixture")]
use crfty_engine::process_supervisor::{ProcessCancellation, ProcessLimits, ProcessTerminal, run};
use std::{path::PathBuf, process::Command, time::Duration};

#[test]
fn official_cli_boundary_rejects_invalid_protocol_and_settles_descendants()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture"));
    for mode in [
        "invalid",
        "oversized",
        "missing",
        "disagree",
        "reject",
        "noisy",
        "hang",
        "orphan",
        "route",
    ] {
        let directory = tempfile::tempdir_in(fixture.parent().ok_or("fixture parent")?)?;
        let report = run(
            Command::new(&fixture)
                .arg("cli-case")
                .arg(mode)
                .arg(directory.path())
                .env("CRFTY_CLI_CASE", mode)
                .env(
                    "CRFTY_ROUTING_MARKER",
                    directory.path().join("routing.marker"),
                ),
            &ProcessCancellation::new(),
            ProcessLimits::new(Some(Duration::from_secs(10)), 4096, 4096),
        );
        assert!(
            matches!(report.terminal, ProcessTerminal::Success(_)),
            "{mode}: {:?}: {}",
            report.terminal,
            String::from_utf8_lossy(report.stderr_tail.as_bytes())
        );
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn windows_adjacent_decoy_cannot_override_the_configured_tool()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture"));
    let directory = tempfile::tempdir_in(fixture.parent().ok_or("fixture parent")?)?;
    let report = run(
        Command::new(env!("CARGO_BIN_EXE_crfty-contract-fixture"))
            .arg("cli-case")
            .arg("decoy")
            .arg(directory.path()),
        &ProcessCancellation::new(),
        ProcessLimits::new(Some(Duration::from_secs(10)), 4096, 4096),
    );
    assert!(
        matches!(report.terminal, ProcessTerminal::Success(_)),
        "{:?}: {}",
        report.terminal,
        String::from_utf8_lossy(report.stderr_tail.as_bytes())
    );
    Ok(())
}
