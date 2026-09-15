//! Capability probe over located tools (ADR-023). Every step is a real
//! invocation judged by exit status, never by parsing a banner or a feature
//! listing, and each is bounded by the process supervisor so a wedged binary
//! can neither stall a session start nor outlive a shutdown. The ffprobe
//! version comes from its JSON document and stands in for both the FFmpeg
//! and encoder revisions: any FFmpeg change conservatively invalidates
//! cached analyses.

use std::{process::Command, time::Duration};

use crfty_core::{ProbeFailure, ToolCapability, ToolRevisions};
use serde::Deserialize;

use super::MediaTools;
use crate::{
    ab_av1::AB_AV1_REVISION,
    process_supervisor::{
        self, BoundedOutput, ProcessCancellation, ProcessLimits, ProcessTerminal,
    },
};

/// Generous for a one-frame synthetic encode; anything slower is a binary
/// that would never finish a real conversion either.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// The version document is a few hundred bytes; anything past this is not
/// ffprobe answering the question.
const STDOUT_HEAD_BYTES: usize = 64 * 1024;
const STDERR_TAIL_BYTES: usize = 4 * 1024;
/// One synthetic frame: enough for the encoder and the filter to initialise
/// and prove they exist, without reading any operator media.
const SYNTHETIC_SOURCE: &str = "testsrc2=size=128x128:rate=1:duration=1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    Verified(ToolRevisions),
    Failed(ProbeFailure),
    Cancelled,
}

enum StepResult {
    Passed(Vec<u8>),
    Failed(ProbeFailure),
    Cancelled,
}

pub(crate) fn probe_capabilities(
    tools: &MediaTools,
    cancellation: &ProcessCancellation,
) -> ProbeOutcome {
    let version = match run_step(
        version_command(tools),
        ToolCapability::FfprobeVersion,
        cancellation,
    ) {
        StepResult::Passed(document) => match parse_version(&document) {
            Ok(version) => version,
            Err(detail) => {
                return ProbeOutcome::Failed(ProbeFailure::InvalidVersionDocument { detail });
            }
        },
        StepResult::Failed(failure) => return ProbeOutcome::Failed(failure),
        StepResult::Cancelled => return ProbeOutcome::Cancelled,
    };
    for (capability, command) in [
        (ToolCapability::Svtav1Encoder, svtav1_command(tools)),
        (ToolCapability::VmafFilter, vmaf_command(tools)),
    ] {
        match run_step(command, capability, cancellation) {
            StepResult::Passed(_) => {}
            StepResult::Failed(failure) => return ProbeOutcome::Failed(failure),
            StepResult::Cancelled => return ProbeOutcome::Cancelled,
        }
    }
    ProbeOutcome::Verified(ToolRevisions {
        ab_av1: AB_AV1_REVISION.to_owned(),
        ffmpeg: version.clone(),
        encoder: version,
    })
}

fn version_command(tools: &MediaTools) -> Command {
    let mut command = Command::new(&tools.ffprobe);
    command.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_program_version",
    ]);
    command
}

fn svtav1_command(tools: &MediaTools) -> Command {
    let mut command = ffmpeg_command(tools);
    command.args([
        "-f",
        "lavfi",
        "-i",
        SYNTHETIC_SOURCE,
        "-c:v",
        "libsvtav1",
        "-preset",
        "8",
        "-f",
        "null",
        "-",
    ]);
    command
}

fn vmaf_command(tools: &MediaTools) -> Command {
    let mut command = ffmpeg_command(tools);
    command.args([
        "-f",
        "lavfi",
        "-i",
        SYNTHETIC_SOURCE,
        "-f",
        "lavfi",
        "-i",
        SYNTHETIC_SOURCE,
        "-filter_complex",
        "[0:v][1:v]libvmaf",
        "-f",
        "null",
        "-",
    ]);
    command
}

fn ffmpeg_command(tools: &MediaTools) -> Command {
    let mut command = Command::new(&tools.ffmpeg);
    command.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    command
}

fn run_step(
    mut command: Command,
    capability: ToolCapability,
    cancellation: &ProcessCancellation,
) -> StepResult {
    let report = process_supervisor::run(
        &mut command,
        cancellation,
        ProcessLimits::new(Some(STEP_TIMEOUT), STDOUT_HEAD_BYTES, STDERR_TAIL_BYTES),
    );
    match report.terminal {
        ProcessTerminal::Success(_) => StepResult::Passed(report.stdout.as_bytes().to_vec()),
        ProcessTerminal::ToolFailed(status) => StepResult::Failed(ProbeFailure::Unsupported {
            capability,
            diagnostic: diagnostic(&report.stderr_tail, &status.to_string()),
        }),
        ProcessTerminal::Cancelled => StepResult::Cancelled,
        ProcessTerminal::TimedOut => StepResult::Failed(ProbeFailure::TimedOut { capability }),
        ProcessTerminal::SpawnFailed(failure)
        | ProcessTerminal::SupervisionFailed(failure)
        | ProcessTerminal::CleanupFailed(failure) => {
            StepResult::Failed(ProbeFailure::CouldNotRun {
                capability,
                detail: failure.message,
            })
        }
    }
}

/// The stderr tail is human-oriented and only ever shown, never matched.
fn diagnostic(stderr_tail: &BoundedOutput, status: &str) -> String {
    let tail = String::from_utf8_lossy(stderr_tail.as_bytes());
    let tail = tail.trim();
    if tail.is_empty() {
        status.to_owned()
    } else {
        tail.to_owned()
    }
}

#[derive(Debug, Deserialize)]
struct VersionDocument {
    program_version: ProgramVersion,
}

#[derive(Debug, Deserialize)]
struct ProgramVersion {
    version: String,
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
