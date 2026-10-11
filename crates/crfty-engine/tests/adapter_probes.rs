#![forbid(unsafe_code)]
#![cfg(feature = "contract-test-fixture")]

use crfty_engine::process_supervisor::{ProcessCancellation, ProcessLimits, ProcessTerminal, run};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[test]
fn initial_search_probe_is_cancellable() {
    contract("cancel-search-entry");
}
#[test]
fn initial_encode_probe_is_cancellable() {
    contract("cancel-encode-entry");
}
#[test]
fn encoded_sample_probe_is_cancellable() {
    contract("cancel-sample-probe");
}
#[test]
fn cancellation_settles_probe_descendants_after_leader_exit() {
    contract("cancel-orphan-probe");
}
#[test]
fn completed_probe_settles_descendants_that_closed_pipes() {
    contract("closed-orphan-probe");
}
#[test]
fn initial_probe_times_out_with_typed_bounded_diagnostics() {
    contract("timeout-entry");
}
#[test]
fn encoded_sample_probe_times_out_with_typed_bounded_diagnostics() {
    contract("timeout-sample");
}
#[test]
fn rejected_probe_retains_a_bounded_diagnostic_tail() {
    contract("reject");
}
#[test]
fn invalid_probe_json_is_a_typed_failure() {
    contract("invalid");
}
#[test]
fn oversized_probe_json_is_a_typed_failure() {
    contract("oversized");
}

#[cfg(unix)]
#[test]
fn closed_pipes_with_a_live_probe_leader_are_cancellable() {
    contract("cancel-closed-probe");
}
#[cfg(unix)]
#[test]
fn closed_pipes_with_a_live_probe_leader_time_out() {
    contract("timeout-closed-probe");
}
#[cfg(unix)]
#[test]
fn fifo_input_does_not_block_cancellation_or_shutdown() {
    contract("cancel-fifo");
}
#[cfg(unix)]
#[test]
fn fifo_input_does_not_bypass_the_probe_deadline() {
    contract("timeout-fifo");
}
#[test]
fn rejected_sample_probe_scrubs_the_sample_path() {
    contract("reject-sample");
}

#[test]
fn entry_probe_path_is_scrubbed_before_tail_truncation() {
    contract("reject-boundary");
}
#[test]
fn sample_probe_path_is_scrubbed_before_tail_truncation() {
    contract("reject-sample-boundary");
}

#[expect(clippy::expect_used, reason = "fixture setup and assertions")]
fn contract(mode: &str) {
    let directory = tempfile::tempdir().expect("create adapter probe fixture directory");
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture"));
    let tools = directory.path().join("tools");
    fs::create_dir_all(&tools).expect("create tool directory");
    let empty_path = directory.path().join("empty-path");
    fs::create_dir_all(&empty_path).expect("create empty PATH directory");
    #[cfg(unix)]
    if mode.ends_with("-fifo") {
        assert!(
            Command::new("mkfifo")
                .arg(directory.path().join("adapter-entry-hang.mkv"))
                .status()
                .expect("create blocked-read FIFO")
                .success()
        );
    }
    let ffmpeg = copy_tool(&fixture, &tools, "ffmpeg");
    let ffprobe = copy_tool(&fixture, &tools, "ffprobe");
    let report = run(
        Command::new(&fixture)
            .arg("adapter-probes")
            .arg(mode)
            .arg(directory.path())
            .arg(ffmpeg)
            .arg(ffprobe)
            .current_dir(directory.path())
            .env(
                "CRFTY_ADAPTER_PROBE_MARKER",
                directory.path().join("probe.heartbeat"),
            )
            .env("PATH", &empty_path)
            .env("XDG_CACHE_HOME", directory.path().join("cache"))
            .env("LOCALAPPDATA", directory.path().join("cache")),
        &ProcessCancellation::new(),
        ProcessLimits::new(Some(Duration::from_secs(50)), 4096, 8192),
    );
    assert!(
        matches!(report.terminal, ProcessTerminal::Success(_)),
        "{mode} failed: {:?}; stderr: {}",
        report.terminal,
        String::from_utf8_lossy(report.stderr_tail.as_bytes())
    );
    assert!(directory.path().join("after-probe.mkv").exists());
}

#[expect(clippy::expect_used, reason = "fixture setup")]
fn copy_tool(fixture: &Path, directory: &Path, name: &str) -> PathBuf {
    let destination = match fixture.extension() {
        Some(extension) => directory.join(name).with_extension(extension),
        None => directory.join(name),
    };
    fs::copy(fixture, &destination).expect("copy fake media tool");
    destination
}
