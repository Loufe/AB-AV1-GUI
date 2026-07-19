#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    env, fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use crfty_engine::remux::{RemuxRequest, RemuxTerminal, start};

const EXPECTED_MAX_STDERR_TAIL_BYTES: usize = 16 * 1024;

#[test]
fn native_remux_process_contract() {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture"));
    let directory = env::temp_dir().join(format!(
        "crfty-remux-contract-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    fs::create_dir_all(&directory).expect("create remux contract directory");
    let ffmpeg = copy_tool(&fixture, &directory, "ffmpeg");
    let input = directory.join("input.mp4");
    fs::write(&input, vec![1_u8; 8192]).expect("create remux input");

    let output = directory.join("output.part");
    let report = start(request(&ffmpeg, &input, &output))
        .expect("start remux")
        .wait()
        .expect("wait for remux");
    assert!(matches!(report.terminal, RemuxTerminal::Completed(_)));
    assert_eq!(
        report
            .final_telemetry
            .map(|telemetry| telemetry.position_ms),
        Some(2_000)
    );
    assert!(output.exists());

    let incompatible = directory.join("noisy-incompatible.mp4");
    fs::write(&incompatible, vec![2_u8; 8192]).expect("create incompatible input");
    let failed = start(request(
        &ffmpeg,
        &incompatible,
        &directory.join("failed.part"),
    ))
    .expect("start failed remux")
    .wait()
    .expect("wait for failed remux");
    match failed.terminal {
        RemuxTerminal::Failed(failure) => {
            assert!(failure.stderr_tail.contains("incompatible stream"));
            assert!(failure.stderr_tail.len() <= EXPECTED_MAX_STDERR_TAIL_BYTES);
        }
        terminal => panic!("expected remux failure, got {terminal:?}"),
    }

    let cancelled = directory.join("cancel-descendant.part");
    let heartbeat = cancelled.with_extension("heartbeat");
    let job = start(request(&ffmpeg, &input, &cancelled)).expect("start cancellable remux");
    wait_for_file(&heartbeat);
    job.cancellation_handle().cancel();
    let cancelled_report = job.wait().expect("wait for cancelled remux");
    assert_eq!(cancelled_report.terminal, RemuxTerminal::Cancelled);
    let heartbeat_value = fs::read(&heartbeat).expect("read heartbeat");
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::read(&heartbeat).expect("read stopped heartbeat"),
        heartbeat_value
    );

    fs::remove_dir_all(directory).expect("remove remux contract directory");
}

fn request(ffmpeg: &Path, input: &Path, output: &Path) -> RemuxRequest {
    RemuxRequest {
        ffmpeg: ffmpeg.to_owned(),
        input: input.to_owned(),
        output: output.to_owned(),
    }
}

fn copy_tool(fixture: &Path, directory: &Path, name: &str) -> PathBuf {
    let destination = match fixture.extension() {
        Some(extension) => directory.join(name).with_extension(extension),
        None => directory.join(name),
    };
    fs::copy(fixture, &destination).expect("copy fake FFmpeg");
    destination
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

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos()
}
