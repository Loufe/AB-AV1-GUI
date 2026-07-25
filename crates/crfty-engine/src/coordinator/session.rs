//! The session loop and per-job dispatch: claiming work, routing it to the
//! encode or remux runner, and publishing phase and terminal transitions.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use crfty_core::{
    ClaimId, ClaimedJob, Command, DurationMs, ItemOutcome, JobAction, JobPhase, JobProgress,
    PhaseSpan, Reply, RunId, Telemetry, WorkerCommand,
};

use crate::{
    ab_av1::AbAv1Runtime,
    driver::CommandSender,
    media::{DecodeResolver, MediaInspector},
    vendor::discovery::{CurrentTools, MediaTools},
};

use super::job::{run_encode, run_remux, search_with_fallback};
use super::output_flow::{begin_output, resolve_output_destination};
use super::supervision::ActiveCancellation;
use super::{EngineConfig, require_accepted};
use crate::clock::now_millis;

#[derive(Clone, Copy)]
pub(super) struct JobServices<'a> {
    pub(super) commands: &'a CommandSender,
    pub(super) runtime: &'a AbAv1Runtime,
    pub(super) tools: &'a MediaTools,
    pub(super) cancellation: &'a ActiveCancellation,
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
    tools_slot: &Mutex<Option<CurrentTools>>,
    cancellation: &ActiveCancellation,
    ids: &AtomicU64,
) -> Result<(), String> {
    // Snapshot the slot once: every claim in this session executes with the
    // same binaries and revisions, and the reducer refuses vendor installs
    // while a session runs, so the snapshot cannot go stale mid-session.
    let current = {
        let slot = match tools_slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.clone()
    };
    let Some(current) = current else {
        // Unreachable past the reducer's session-start gate; finish the
        // session gracefully rather than reporting a worker crash.
        return require_accepted(
            "finish tool-less worker session",
            commands.submit(Command::Worker(WorkerCommand::Finished)),
        );
    };
    let tools = &current.media;
    let inspector = MediaInspector::new(tools.ffprobe.clone());
    let mut decoder_resolver = DecodeResolver::new(tools.ffmpeg.clone());
    // Claim-time revision immutability: the composed revisions freeze into
    // each JobSpec at PrepareReserved and survive any later tool swap.
    let mut base_execution = config.execution.clone();
    base_execution.profile.ab_av1_revision = current.revisions.ab_av1.clone();
    base_execution.profile.ffmpeg_revision = current.revisions.ffmpeg.clone();
    base_execution.profile.encoder_revision = current.revisions.encoder.clone();
    loop {
        let claim_id = ClaimId(ids.fetch_add(1, Ordering::Relaxed));
        let run_id = RunId(ids.fetch_add(1, Ordering::Relaxed));
        let reservation = commands.submit(Command::Worker(WorkerCommand::ReserveNext {
            claim_id,
            run_id,
        }));
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
        let observation = match inspector.observe(&reserved.input) {
            Ok(observation) => Some(Box::new(observation)),
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
        let mut execution = base_execution.clone();
        execution.profile.decode_mode =
            observation
                .as_ref()
                .map_or(crfty_core::DecodeMode::Software, |observed| {
                    decoder_resolver.resolve(execution.decode_preference, &observed.metadata.codec)
                });
        // The observed file's normalized spellings, matched against the
        // parked import inbox by the reducer during preparation.
        let import_paths = crate::history_import::import_path_candidates(&reserved.input);
        let prepared = commands.submit(Command::Worker(WorkerCommand::PrepareReserved {
            item_id: reserved.item_id,
            claim_id,
            run_id,
            observation,
            import_paths,
            execution,
        }));
        let job = match prepared {
            Ok(Reply::Claimed(Some(job))) => job,
            Ok(Reply::Rejected { reason } | Reply::DurabilityUnknown { reason }) => {
                let _reply = commands.submit(Command::Worker(WorkerCommand::AbandonReservation {
                    item_id: reserved.item_id,
                    claim_id,
                    run_id,
                    at: now_millis(),
                }));
                return Err(reason);
            }
            Err(error) => {
                let _reply = commands.submit(Command::Worker(WorkerCommand::AbandonReservation {
                    item_id: reserved.item_id,
                    claim_id,
                    run_id,
                    at: now_millis(),
                }));
                return Err(format!("worker preparation failed: {error}"));
            }
            Ok(
                Reply::Accepted
                | Reply::AnalysisStarted { .. }
                | Reply::BasicScan(_)
                | Reply::Claimed(None)
                | Reply::Reserved(_)
                | Reply::Imported { .. },
            ) => {
                return Err("preparation command returned an invalid reply".to_owned());
            }
        };
        require_accepted(
            "mark worker item started",
            commands.submit(Command::Worker(WorkerCommand::Started {
                item_id: job.spec.item_id,
                claim_id,
                run_id,
                at: now_millis(),
            })),
        )?;
        process_job(
            commands,
            runtime,
            tools,
            cancellation,
            &job,
            input_duration_ms,
        )?;
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
    cancellation: &ActiveCancellation,
    job: &ClaimedJob,
    input_duration_ms: Option<u64>,
) -> Result<(), String> {
    let mut tracker = PhaseTracker::new();
    let services = JobServices {
        commands,
        runtime,
        tools,
        cancellation,
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
            let Some(output) = begin_output(commands, tools, job, destination, &mut tracker)?
            else {
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
        let searched =
            match search_with_fallback(commands, runtime, tools, cancellation, job, &mut tracker) {
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
    let Some(output) = begin_output(commands, tools, job, destination, &mut tracker)? else {
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
