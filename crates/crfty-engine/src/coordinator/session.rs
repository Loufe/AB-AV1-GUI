//! The session loop and per-job dispatch: claiming work, routing it to the
//! encode or remux runner, and publishing phase and terminal transitions.

use std::{sync::Mutex, time::Instant};

use crfty_core::{
    ClaimedJob, Command, DurationMs, ItemOutcome, JobAction, JobPhase, JobProgress, LocatedTools,
    PhaseSpan, Reply, RunId, SystemCommand, Telemetry, ToolVerification, WorkerCommand,
};

use crate::{
    ab_av1::AbAv1Runtime,
    driver::CommandSender,
    media::{MediaError, MediaInspector},
    process_supervisor::ProcessCancellation,
    tools::{
        MediaTools,
        probe::{ProbeOutcome, probe_capabilities},
    },
};

use super::job::{run_encode, run_remux, search_with_fallback};
use super::output_flow::{begin_output, resolve_output_destination};
use super::supervision::{ActiveCancellation, RunScope};
use super::{EngineConfig, ToolsConfig, require_accepted};
use crate::clock::now_millis;

#[derive(Clone, Copy)]
pub(super) struct JobServices<'a> {
    pub(super) commands: &'a CommandSender,
    pub(super) runtime: &'a AbAv1Runtime,
    pub(super) tools: &'a MediaTools,
    /// The run's cancellation scope: probes borrow its signal and adapter
    /// jobs register with it, so Force Stop reaches both.
    pub(super) run: &'a RunScope<'a>,
    /// Input media duration from the claim-time preflight probe; the total
    /// the encode/remux output position runs toward, so the ETA's remaining
    /// work is known. `None` (probe failed or reported zero) means no ETA.
    pub(super) input_duration_ms: Option<u64>,
}

/// Per-job telemetry sequencing plus monotonic phase-span accumulation. The
/// spans ride the lossless terminal command; telemetry stays the lossy path.
pub(super) struct PhaseTracker {
    sequence: u64,
    current: Option<(JobPhase, Instant)>,
    spans: Vec<PhaseSpan>,
}

impl PhaseTracker {
    fn new() -> Self {
        Self {
            sequence: 0,
            current: None,
            spans: Vec::new(),
        }
    }

    pub(super) fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    /// Starts measuring `phase`, closing the previous span. Re-entering the
    /// running phase is a no-op so repeated search attempts accumulate into
    /// one span instead of fragmenting.
    pub(super) fn enter(&mut self, phase: JobPhase) {
        if self
            .current
            .as_ref()
            .is_some_and(|(active, _)| *active == phase)
        {
            return;
        }
        self.close_current();
        self.current = Some((phase, Instant::now()));
    }

    fn close_current(&mut self) {
        if let Some((phase, entered)) = self.current.take() {
            let elapsed = u64::try_from(entered.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.spans.push(PhaseSpan {
                phase,
                duration: DurationMs(elapsed),
            });
        }
    }

    fn finish(&mut self) -> Vec<PhaseSpan> {
        self.close_current();
        std::mem::take(&mut self.spans)
    }
}

pub(super) fn run_session(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    config: &EngineConfig,
    tools_slot: &Mutex<Option<LocatedTools>>,
    cancellation: &ActiveCancellation,
) -> Result<(), String> {
    // Snapshot the slot once: every claim in this session executes with the
    // same binaries and revisions. A rediscovery mid-session only affects
    // the next session.
    let located = {
        let slot = match tools_slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.clone()
    };
    let Some(located) = located else {
        // Unreachable past the reducer's session-start gate; finish the
        // session gracefully rather than reporting a worker crash.
        return require_accepted(
            "finish tool-less worker session",
            commands.submit(Command::Worker(WorkerCommand::Finished)),
        );
    };
    let tools = MediaTools::from(&located);
    let tools = &tools;
    // The probe is the gate before the first claim (ADR-023): binaries that
    // cannot encode or score never reserve an item. The reducer composes the
    // reported revisions and decoders into every claim this session prepares.
    if let ToolsConfig::Discover(_) = &config.tools {
        let probe_cancellation = ProcessCancellation::new();
        let outcome = {
            let _registration = cancellation.register_probe(&probe_cancellation);
            probe_capabilities(tools, &probe_cancellation)
        };
        let verification = match outcome {
            ProbeOutcome::Verified {
                revisions,
                hardware_decoders,
            } => ToolVerification::Verified {
                revisions,
                hardware_decoders,
            },
            ProbeOutcome::Failed(failure) => ToolVerification::Failed(failure),
            ProbeOutcome::Cancelled => {
                return require_accepted(
                    "finish cancelled worker session",
                    commands.submit(Command::Worker(WorkerCommand::Finished)),
                );
            }
        };
        let verified = match &verification {
            ToolVerification::Verified { .. } => true,
            ToolVerification::Failed(failure) => {
                tracing::warn!("media tools failed verification: {}", failure.summary());
                false
            }
            ToolVerification::Pending => false,
        };
        require_accepted(
            "report tool verification",
            commands.submit(Command::System(SystemCommand::ToolsProbed {
                tools: located.clone(),
                verification,
            })),
        )?;
        if !verified {
            return require_accepted(
                "finish unverified worker session",
                commands.submit(Command::Worker(WorkerCommand::Finished)),
            );
        }
    }
    let inspector = MediaInspector::new(tools.ffprobe.clone());
    loop {
        let reservation = commands.submit(Command::Worker(WorkerCommand::ReserveNext));
        let reserved = match reservation {
            Ok(Reply::Reserved(Some(job))) => job,
            Ok(Reply::Reserved(None) | Reply::Rejected { .. }) => break,
            Ok(Reply::DurabilityUnknown { reason }) => return Err(reason),
            Err(error) => return Err(format!("worker reservation failed: {error}")),
            Ok(
                Reply::Accepted
                | Reply::AnalysisStarted { .. }
                | Reply::BasicScan(_)
                | Reply::Claimed(_)
                | Reply::Imported { .. },
            ) => {
                return Err("reservation command returned an invalid reply".to_owned());
            }
        };
        let claim_id = reserved.claim_id;
        let run_id = reserved.run_id;
        // Opened before the claim-time probe so a Force Stop that lands while
        // ffprobe is reading the input terminates it like any other run work.
        let run = cancellation.begin_run(run_id);
        let observation = match inspector.observe(&reserved.input, run.probes()) {
            Ok(observation) => Some(Box::new(observation)),
            Err(MediaError::Cancelled) => None,
            Err(error) => {
                tracing::warn!(
                    "media preflight failed; continuing without reusable facts: {error}"
                );
                None
            }
        };
        let input_duration_ms = observation
            .as_ref()
            .map(|observed| observed.metadata.duration_ms)
            .filter(|duration| *duration > 0);
        if run.probes().is_cancelled() {
            require_accepted(
                "abandon force-stopped reservation",
                commands.submit(Command::Worker(WorkerCommand::AbandonReservation {
                    item_id: reserved.item_id,
                    claim_id,
                    run_id,
                    at: now_millis(),
                    disposition: crfty_core::ReservationDisposition::Stopped,
                })),
            )?;
            continue;
        }
        // The observed file's normalized spellings, matched against the
        // parked import inbox by the reducer during preparation.
        let import_paths = crate::history_import::import_path_candidates(&reserved.input);
        let prepared = commands.submit(Command::Worker(WorkerCommand::PrepareReserved {
            item_id: reserved.item_id,
            claim_id,
            run_id,
            observation,
            import_paths,
        }));
        let job = prepared_job(prepared, |reason| {
            require_accepted(
                "record rejected preparation",
                commands.submit(Command::Worker(WorkerCommand::AbandonReservation {
                    item_id: reserved.item_id,
                    claim_id,
                    run_id,
                    at: now_millis(),
                    disposition: crfty_core::ReservationDisposition::Failed(
                        crfty_core::FailureFacts::new(crfty_core::FailureKind::Internal, reason),
                    ),
                })),
            )
        })?;
        require_accepted(
            "mark worker item started",
            commands.submit(Command::Worker(WorkerCommand::Started {
                item_id: job.spec.item_id,
                claim_id,
                run_id,
                at: now_millis(),
            })),
        )?;
        process_job(commands, runtime, tools, &run, &job, input_duration_ms)?;
    }
    require_accepted(
        "finish worker session",
        commands.submit(Command::Worker(WorkerCommand::Finished)),
    )
}

fn process_job(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    run: &RunScope<'_>,
    job: &ClaimedJob,
    input_duration_ms: Option<u64>,
) -> Result<(), String> {
    let mut tracker = PhaseTracker::new();
    let services = JobServices {
        commands,
        runtime,
        tools,
        run,
        input_duration_ms,
    };
    publish_phase(commands, job.spec.run_id, &mut tracker, JobPhase::Preparing);
    match &job.spec.action {
        JobAction::Skip { reason } => {
            terminal(
                commands,
                job,
                &mut tracker,
                ItemOutcome::Skipped {
                    reason: reason.clone(),
                },
                None,
            )?;
            return Ok(());
        }
        JobAction::Remux => {
            let Some(destination) = resolve_output_destination(commands, job, &mut tracker)? else {
                return Ok(());
            };
            let Some(output) = begin_output(services, job, destination, &mut tracker)? else {
                return Ok(());
            };
            return run_remux(services, job, output, &mut tracker);
        }
        JobAction::Analyze { .. } | JobAction::Encode { .. } => {}
    }

    let destination = if matches!(job.spec.action, JobAction::Encode { .. }) {
        let Some(destination) = resolve_output_destination(commands, job, &mut tracker)? else {
            return Ok(());
        };
        Some(destination)
    } else {
        None
    };
    let analysis = if let Some(selected) = job.spec.action.selected_analysis() {
        selected.clone()
    } else {
        let searched = match search_with_fallback(commands, runtime, tools, run, job, &mut tracker)
        {
            Ok(result) => result,
            Err(outcome) => {
                terminal(commands, job, &mut tracker, outcome, None)?;
                return Ok(());
            }
        };
        require_accepted(
            "record analysis",
            commands.submit(Command::Worker(WorkerCommand::RecordAnalysis {
                item_id: job.spec.item_id,
                claim_id: job.spec.claim_id,
                run_id: job.spec.run_id,
                result: Box::new(searched.clone()),
            })),
        )?;
        searched
    };
    if matches!(job.spec.action, JobAction::Analyze { .. }) {
        publish_phase(
            commands,
            job.spec.run_id,
            &mut tracker,
            JobPhase::Finalizing,
        );
        terminal(commands, job, &mut tracker, ItemOutcome::Analyzed, None)?;
        return Ok(());
    }

    let Some(destination) = destination else {
        return Err("encode job has no resolved output destination".to_owned());
    };
    let Some(output) = begin_output(services, job, destination, &mut tracker)? else {
        return Ok(());
    };
    run_encode(services, job, output, analysis, &mut tracker)
}

pub(super) fn terminal(
    commands: &CommandSender,
    job: &ClaimedJob,
    tracker: &mut PhaseTracker,
    outcome: ItemOutcome,
    final_telemetry: Option<Telemetry>,
) -> Result<(), String> {
    require_accepted(
        "record terminal outcome",
        commands.submit(Command::Worker(WorkerCommand::Terminal {
            item_id: job.spec.item_id,
            claim_id: job.spec.claim_id,
            run_id: job.spec.run_id,
            outcome,
            at: now_millis(),
            phase_spans: tracker.finish(),
            final_telemetry,
        })),
    )
}

/// Wraps a final adapter progress value for the lossless terminal command.
/// Rate is live display state, meaningless once the run is over, so the
/// terminal record never carries one.
pub(super) fn map_progress(
    run_id: RunId,
    tracker: &mut PhaseTracker,
    phase: JobPhase,
    progress: Option<JobProgress>,
) -> Option<Telemetry> {
    progress.map(|progress| Telemetry {
        run_id,
        sequence: tracker.next_sequence(),
        phase,
        progress,
        fps_centi: None,
        eta_ms: None,
    })
}

pub(super) fn publish_phase(
    commands: &CommandSender,
    run_id: RunId,
    tracker: &mut PhaseTracker,
    phase: JobPhase,
) {
    tracker.enter(phase);
    commands.publish_telemetry(Telemetry {
        run_id,
        sequence: tracker.next_sequence(),
        phase,
        progress: JobProgress::Phase,
        fps_centi: None,
        eta_ms: None,
    });
}

fn prepared_job(
    reply: Result<Reply, crate::driver::SubmitError>,
    record_rejection: impl FnOnce(&str) -> Result<(), String>,
) -> Result<Box<ClaimedJob>, String> {
    match reply {
        Ok(Reply::Claimed(Some(job))) => Ok(job),
        Ok(Reply::Rejected { reason }) => {
            record_rejection(&reason)
                .map_err(|error| format!("preparation rejected: {reason}; {error}"))?;
            Err(reason)
        }
        Ok(Reply::DurabilityUnknown { reason }) => Err(reason),
        Err(error) => Err(format!("worker preparation failed: {error}")),
        Ok(_) => Err("preparation command returned an invalid reply".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::prepared_job;
    use crate::driver::SubmitError;
    use crfty_core::Reply;

    #[test]
    fn only_a_definitive_preparation_rejection_authorizes_a_failure_record() {
        for reply in [
            Ok(Reply::DurabilityUnknown {
                reason: "uncertain write".to_owned(),
            }),
            Err(SubmitError::Disconnected),
            Err(SubmitError::ReplyDisconnected),
            Ok(Reply::Claimed(None)),
            Ok(Reply::Accepted),
        ] {
            let mut recorded = false;
            let result = prepared_job(reply, |_| {
                recorded = true;
                Ok(())
            });
            assert!(result.is_err());
            assert!(!recorded);
        }
        let mut recorded = None;
        let result = prepared_job(
            Ok(Reply::Rejected {
                reason: "invalid preparation".to_owned(),
            }),
            |reason| {
                recorded = Some(reason.to_owned());
                Ok(())
            },
        );
        assert_eq!(recorded.as_deref(), Some("invalid preparation"));
        assert_eq!(result, Err("invalid preparation".to_owned()));
    }

    #[test]
    fn failed_rejection_record_keeps_both_failure_contexts() {
        let result = prepared_job(
            Ok(Reply::Rejected {
                reason: "stale reservation".to_owned(),
            }),
            |_| Err("record rejected preparation: driver disconnected".to_owned()),
        );
        assert_eq!(result, Err("preparation rejected: stale reservation; record rejected preparation: driver disconnected".to_owned()));
    }
}
