#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crfty_engine::process_supervisor::{
    ProcessCancellation, ProcessFailureStage, ProcessLimits, ProcessTerminal, run,
};

const LARGE_STREAM_BYTES: usize = 2 * 1024 * 1024;
const STDOUT_LIMIT: usize = 257;
const STDERR_LIMIT: usize = 263;

#[test]
fn large_streams_are_drained_concurrently_and_retained_deterministically() {
    let fixture = fixture();
    let report = run(
        Command::new(fixture)
            .arg("emit")
            .arg(LARGE_STREAM_BYTES.to_string())
            .arg(LARGE_STREAM_BYTES.to_string())
            .arg("0"),
        &ProcessCancellation::new(),
        limits(Some(Duration::from_secs(10))),
    );

    assert!(
        matches!(report.terminal, ProcessTerminal::Success(_)),
        "unexpected terminal: {:?}",
        report.terminal
    );
    assert_eq!(report.stdout.total_bytes(), LARGE_STREAM_BYTES);
    assert_eq!(report.stderr_tail.total_bytes(), LARGE_STREAM_BYTES);
    assert_eq!(
        report.stdout.as_bytes(),
        expected_pattern(0, STDOUT_LIMIT, b'a')
    );
    assert_eq!(
        report.stderr_tail.as_bytes(),
        expected_pattern(LARGE_STREAM_BYTES - STDERR_LIMIT, STDERR_LIMIT, b'A')
    );
    assert!(report.stdout.was_truncated());
    assert!(report.stderr_tail.was_truncated());
}

#[test]
fn reports_tool_failure_and_preserves_its_diagnostic_tail() {
    let fixture = fixture();
    let report = run(
        Command::new(fixture)
            .arg("emit")
            .arg("8")
            .arg("1024")
            .arg("7"),
        &ProcessCancellation::new(),
        limits(Some(Duration::from_secs(5))),
    );

    assert!(
        matches!(report.terminal, ProcessTerminal::ToolFailed(_)),
        "unexpected terminal: {:?}",
        report.terminal
    );
    assert_eq!(report.stdout.as_bytes(), expected_pattern(0, 8, b'a'));
    assert_eq!(report.stderr_tail.as_bytes().len(), STDERR_LIMIT);
}

#[test]
fn reports_spawn_failure_without_panicking() {
    let missing = env::temp_dir().join(format!(
        "crfty-missing-process-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let report = run(
        &mut Command::new(missing),
        &ProcessCancellation::new(),
        limits(Some(Duration::from_secs(1))),
    );

    assert!(matches!(
        report.terminal,
        ProcessTerminal::SpawnFailed(ref failure)
            if failure.stage == ProcessFailureStage::Spawn
    ));
}

#[test]
fn timeout_terminates_the_process_group() {
    let fixture = fixture();
    let report = run(
        Command::new(fixture).arg("sleep"),
        &ProcessCancellation::new(),
        limits(Some(Duration::from_millis(100))),
    );

    assert_eq!(report.terminal, ProcessTerminal::TimedOut);
}

#[test]
fn natural_leader_exit_cleans_a_descendant_that_kept_its_pipes() {
    let fixture = fixture();
    let directory = test_directory("orphan-kept-pipes");
    fs::create_dir_all(&directory).expect("create process supervisor test directory");
    let heartbeat = directory.join("heartbeat");
    let report = run(
        Command::new(fixture)
            .arg("orphan-heartbeat")
            .arg(&heartbeat),
        &ProcessCancellation::new(),
        limits(Some(Duration::from_secs(5))),
    );

    assert!(
        matches!(report.terminal, ProcessTerminal::Success(_)),
        "unexpected terminal: {:?}",
        report.terminal
    );
    let stopped_value = fs::read(&heartbeat).expect("read heartbeat after leader exit");
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::read(&heartbeat).expect("read stopped inherited-pipe heartbeat"),
        stopped_value
    );
    fs::remove_dir_all(directory).expect("remove process supervisor test directory");
}

#[test]
fn natural_leader_exit_cleans_a_descendant_that_closed_its_pipes() {
    let fixture = fixture();
    let directory = test_directory("orphan-closed-pipes");
    fs::create_dir_all(&directory).expect("create process supervisor test directory");
    let heartbeat = directory.join("heartbeat");
    let report = run(
        Command::new(fixture)
            .arg("orphan-closed-pipes")
            .arg(&heartbeat),
        &ProcessCancellation::new(),
        limits(Some(Duration::from_secs(5))),
    );

    assert!(
        matches!(report.terminal, ProcessTerminal::Success(_)),
        "unexpected terminal: {:?}",
        report.terminal
    );
    let stopped_value = fs::read(&heartbeat).expect("read heartbeat after leader exit");
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::read(&heartbeat).expect("read stopped detached heartbeat"),
        stopped_value
    );
    fs::remove_dir_all(directory).expect("remove process supervisor test directory");
}

#[test]
fn cancellation_terminates_the_native_process_tree_and_joins_readers() {
    let fixture = fixture();
    let directory = test_directory("cancel-tree");
    fs::create_dir_all(&directory).expect("create process supervisor test directory");
    let heartbeat = directory.join("heartbeat");
    let cancellation = ProcessCancellation::new();
    let worker_cancellation = cancellation.clone();
    let worker_fixture = fixture.clone();
    let worker_heartbeat = heartbeat.clone();
    let worker = thread::spawn(move || {
        run(
            Command::new(worker_fixture)
                .arg("spawn-heartbeat")
                .arg(worker_heartbeat),
            &worker_cancellation,
            limits(Some(Duration::from_secs(10))),
        )
    });

    wait_for_file(&heartbeat);
    cancellation.cancel();
    let report = worker.join().expect("join process supervisor caller");
    assert_eq!(report.terminal, ProcessTerminal::Cancelled);
    let stopped_value = fs::read(&heartbeat).expect("read heartbeat after cancellation");
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::read(&heartbeat).expect("read stopped heartbeat"),
        stopped_value
    );
    fs::remove_dir_all(directory).expect("remove process supervisor test directory");
}

#[test]
fn cancellation_before_spawn_has_no_side_effect() {
    let fixture = fixture();
    let directory = test_directory("pre-cancel");
    fs::create_dir_all(&directory).expect("create process supervisor test directory");
    let heartbeat = directory.join("heartbeat");
    let cancellation = ProcessCancellation::new();
    cancellation.cancel();
    let report = run(
        Command::new(fixture).arg("spawn-heartbeat").arg(&heartbeat),
        &cancellation,
        limits(Some(Duration::from_secs(1))),
    );

    assert_eq!(report.terminal, ProcessTerminal::Cancelled);
    assert!(!heartbeat.exists());
    fs::remove_dir_all(directory).expect("remove process supervisor test directory");
}

fn limits(timeout: Option<Duration>) -> ProcessLimits {
    ProcessLimits::new(timeout, STDOUT_LIMIT, STDERR_LIMIT)
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_crfty-process-fixture"))
}

fn expected_pattern(offset: usize, count: usize, base: u8) -> Vec<u8> {
    (offset..offset + count)
        .map(|position| base + u8::try_from(position % 26).unwrap_or_default())
        .collect()
}

fn wait_for_file(path: &Path) {
    for _attempt in 0..500 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("fixture did not create {}", path.display());
}

fn test_directory(label: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "crfty-process-{label}-{}-{}",
        std::process::id(),
        unique_suffix()
    ))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos()
}
