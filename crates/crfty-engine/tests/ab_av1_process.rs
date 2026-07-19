#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn encode_cancel_cleanup_and_second_job_contract() {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture"));
    let directory = env::temp_dir().join(format!(
        "crfty-ab-av1-contract-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let tools = directory.join("tools");
    fs::create_dir_all(&tools).expect("create contract directories");
    let input = directory.join("input.mkv");
    fs::write(&input, vec![1_u8; 8192]).expect("create input fixture");

    copy_tool(&fixture, &tools, "ffmpeg");
    copy_tool(&fixture, &tools, "ffprobe");

    let mut path = OsString::from(&tools);
    if let Some(existing) = env::var_os("PATH") {
        path.push(path_separator());
        path.push(existing);
    }
    let status = Command::new(&fixture)
        .arg("run")
        .arg(&input)
        .arg(&directory)
        .env("PATH", path)
        .status()
        .expect("run process contract fixture");

    assert!(status.success(), "contract fixture failed: {status}");
    assert!(directory.join("first.mkv").exists());
    assert!(directory.join("second.mkv").exists());
    assert!(!directory.join("cancel.mkv").exists());
    fs::remove_dir_all(&directory).expect("remove contract directory");
}

fn copy_tool(fixture: &Path, directory: &Path, name: &str) {
    let extension = fixture.extension();
    let destination = match extension {
        Some(extension) => directory.join(name).with_extension(extension),
        None => directory.join(name),
    };
    fs::copy(fixture, destination).expect("copy fake media tool");
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos()
}

#[cfg(windows)]
fn path_separator() -> &'static str {
    ";"
}

#[cfg(not(windows))]
fn path_separator() -> &'static str {
    ":"
}
