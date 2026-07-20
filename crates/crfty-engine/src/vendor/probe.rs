//! Machine-readable ffprobe version probe. System and explicit tools have no
//! trusted metadata, so their revision provenance is established by running
//! `ffprobe -print_format json -show_program_version` and parsing the JSON
//! document — never the human `-version` banner.

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::process::ContainedChild;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// The version document is a few hundred bytes; anything past this is not
/// ffprobe answering the question.
const PROBE_MAX_OUTPUT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
struct VersionDocument {
    program_version: ProgramVersion,
}

#[derive(Debug, Deserialize)]
struct ProgramVersion {
    version: String,
}

/// Runs the given ffprobe binary and returns its self-reported version
/// string. Any failure — spawn, timeout, non-zero exit, unparseable output —
/// is a typed error the caller treats as fail-closed missing tools.
pub(super) fn ffprobe_version(ffprobe: &Path) -> Result<String, String> {
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_program_version",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = ContainedChild::spawn(&mut command)
        .map_err(|error| format!("failed to run the ffprobe version probe: {error}"))?;
    let stdout = child
        .take_stdout()
        .ok_or_else(|| "the ffprobe version probe has no stdout pipe".to_owned())?;
    let reader = thread::Builder::new()
        .name("crfty-ffprobe-version".to_owned())
        .spawn(move || {
            let mut buffer = Vec::new();
            let result = stdout.take(PROBE_MAX_OUTPUT_BYTES).read_to_end(&mut buffer);
            result.map(|_bytes| buffer)
        })
        .map_err(|error| format!("failed to start the probe output reader: {error}"))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    if let Err(error) = child.terminate_and_wait() {
                        eprintln!("failed to terminate a timed-out ffprobe probe: {error}");
                    }
                    let _output = reader.join();
                    return Err(format!(
                        "the ffprobe version probe timed out after {} seconds",
                        PROBE_TIMEOUT.as_secs()
                    ));
                }
                thread::sleep(PROBE_POLL_INTERVAL);
            }
            Err(error) => {
                if let Err(terminate) = child.terminate_and_wait() {
                    eprintln!("failed to terminate a failed ffprobe probe: {terminate}");
                }
                let _output = reader.join();
                return Err(format!(
                    "failed to wait for the ffprobe version probe: {error}"
                ));
            }
        }
    };
    let output = reader
        .join()
        .map_err(|_panic| "the probe output reader panicked".to_owned())?
        .map_err(|error| format!("failed to read the ffprobe version probe output: {error}"))?;
    if !status.success() {
        return Err(format!("the ffprobe version probe exited with {status}"));
    }
    parse_version(&output)
}

fn parse_version(output: &[u8]) -> Result<String, String> {
    let document: VersionDocument = serde_json::from_slice(output)
        .map_err(|error| format!("the ffprobe version document is not valid JSON: {error}"))?;
    if document.program_version.version.is_empty() {
        return Err("ffprobe reported an empty version".to_owned());
    }
    Ok(document.program_version.version)
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn parses_the_program_version_document() {
        let document = br#"{"program_version": {"version": "8.1.2", "copyright": "c", "compiler_ident": "gcc", "configuration": ""}}"#;
        assert_eq!(parse_version(document), Ok("8.1.2".to_owned()));
    }

    #[test]
    fn rejects_banners_and_empty_versions() {
        assert!(parse_version(b"ffprobe version 8.1.2 Copyright (c)").is_err());
        assert!(parse_version(br#"{"program_version": {"version": ""}}"#).is_err());
        assert!(parse_version(br#"{"streams": []}"#).is_err());
    }
}
