//! Adapter runs for one claimed job: the encode path with its search and
//! hardware-decode fallback, the remux path, and the report waits both use.

use std::time::{Duration, Instant};

use crfty_core::{
    AnalysisAttempt, AnalysisResult, ClaimedJob, DecodeMode, FailureFacts, FailureKind,
    ItemOutcome, JobPhase, OutputDelta, OutputTransaction, PERCENT_BASIS_POINTS_SCALE, RunId,
    Telemetry, VmafTarget,
};

use crate::{
    ab_av1::{
        AbAv1Runtime, EncodeOutcome, EncodeRequest, JobFailureKind, JobHandle, JobReport,
        JobTerminal, SearchRequest,
    },
    driver::CommandSender,
    failure::scrub_tail,
    rate::{RateSample, RateTracker},
    remux::{self, RemuxHandle, RemuxReport, RemuxRequest, RemuxTerminal},
    vendor::discovery::MediaTools,
};

use super::output_flow::{
    MediaOutputManager, PreparedOutput, abandon_output, finish_successful_output, fold_transaction,
    submit_output,
};
use super::session::{JobServices, PhaseTracker, map_progress};
use super::supervision::{ActiveCancellation, ActiveJobCancellation};
use super::telemetry::{
    NORMALIZED_PROGRESS_MAX, crf_to_f32, measurement, rate_sample, remux_progress,
    telemetry_progress,
};

const ADAPTER_REPORT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// What a successful adapter run measured before settlement. The terminal
/// outcome is built only after the output transaction settles, from these
/// facts plus the settled ledger state.
pub(super) enum SuccessfulJob {
    Encode {
        outcome: EncodeOutcome,
        decode_mode: DecodeMode,
    },
    Remux,
}

pub(super) fn run_encode(
    services: JobServices<'_>,
    job: &ClaimedJob,
    output: PreparedOutput,
    analysis: AnalysisResult,
    tracker: &mut PhaseTracker,
) -> Result<(), String> {
    let PreparedOutput {
        manager,
        mut transaction,
    } = output;
    let mut decode_mode = analysis.profile.decode_mode;
    loop {
        let request = EncodeRequest {
            input: job.spec.input.clone(),
            output: transaction.staging.clone(),
            crf: crf_to_f32(analysis.measurement.crf),
            preset: analysis.profile.preset,
            decode_mode,
        };
        let handle = match services
            .runtime
            .start_encode(services.tools.clone(), request)
        {
            Ok(handle) => handle,
            Err(error) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(FailureFacts::new(
                        FailureKind::EncodeStart,
                        error.to_string(),
                    )),
                    None,
                    tracker,
                );
            }
        };
        let report = wait_for_report(
            services.commands,
            job.spec.run_id,
            handle,
            services.cancellation,
            JobPhase::Encoding,
            tracker,
            services.input_duration_ms.map(|duration| duration as f64),
        );
        match report {
            Ok(JobReport {
                terminal: JobTerminal::Completed(outcome),
                final_telemetry,
            }) => {
                return finish_successful_output(
                    services.commands,
                    job,
                    manager,
                    transaction,
                    SuccessfulJob::Encode {
                        outcome,
                        decode_mode,
                    },
                    final_telemetry.as_ref().map(telemetry_progress),
                    tracker,
                );
            }
            Ok(JobReport {
                terminal: JobTerminal::Cancelled,
                final_telemetry,
            }) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Stopped,
                    map_progress(
                        job.spec.run_id,
                        tracker,
                        JobPhase::Encoding,
                        final_telemetry.as_ref().map(telemetry_progress),
                    ),
                    tracker,
                );
            }
            Ok(JobReport {
                terminal: JobTerminal::Failed(failure),
                final_telemetry,
            }) => {
                // Hardware→software retry: hook BEFORE any abandonment so the
                // still-unsettled transaction is reused. The failed attempt's
                // adapter cleanup deleted the staging file, so restaging moves
                // the journaled pin to a recreated one; once a transaction is
                // abandoned the ledger refuses to restage, which is what makes
                // retry-after-abandonment unrepresentable. The requested
                // JobSpec is never rewritten — the divergence is recorded in
                // the terminal evidence's `encode_decode`.
                if matches!(decode_mode, DecodeMode::Hardware(_)) {
                    match restage_for_retry(&manager, services.commands, &mut transaction) {
                        Ok(()) => {
                            tracing::warn!(
                                "hardware-decode encode failed ({}); retrying once with software decode",
                                failure.message
                            );
                            decode_mode = DecodeMode::Software;
                            continue;
                        }
                        Err(restage_error) => {
                            tracing::warn!(
                                "staging could not be recreated for the software retry: {restage_error}"
                            );
                        }
                    }
                }
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(FailureFacts::new(FailureKind::EncodeRun, failure.message)),
                    map_progress(
                        job.spec.run_id,
                        tracker,
                        JobPhase::Encoding,
                        final_telemetry.as_ref().map(telemetry_progress),
                    ),
                    tracker,
                );
            }
            Ok(JobReport {
                terminal: JobTerminal::Panicked { cleanup_failure },
                final_telemetry,
            }) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(panicked_facts(
                        "encode adapter panicked",
                        cleanup_failure,
                        job,
                        &transaction,
                    )),
                    map_progress(
                        job.spec.run_id,
                        tracker,
                        JobPhase::Encoding,
                        final_telemetry.as_ref().map(telemetry_progress),
                    ),
                    tracker,
                );
            }
            Err(message) => {
                return abandon_output(
                    &manager,
                    services.commands,
                    job,
                    &transaction,
                    ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, message)),
                    None,
                    tracker,
                );
            }
        }
    }
}

/// Moves the journaled staging pin to a freshly recreated empty staging file
/// so a software-decode retry can reuse the still-unsettled transaction: the
/// recreated identity is journaled as a repeated `StagingCreated`.
fn restage_for_retry(
    manager: &MediaOutputManager,
    commands: &CommandSender,
    transaction: &mut OutputTransaction,
) -> Result<(), String> {
    let initial = manager
        .restage(transaction)
        .map_err(|error| error.to_string())?;
    let created = OutputDelta::StagingCreated {
        run_id: transaction.run_id,
        initial,
    };
    submit_output(commands, created.clone())?;
    fold_transaction(transaction, created);
    Ok(())
}

/// Facts for an adapter panic: the cleanup error, if any, may embed run paths,
/// so it travels as a scrubbed diagnostic rather than message prose.
fn panicked_facts(
    message: &str,
    cleanup_failure: Option<String>,
    job: &ClaimedJob,
    transaction: &OutputTransaction,
) -> FailureFacts {
    let facts = FailureFacts::new(
        FailureKind::AdapterPanicked {
            cleanup_failed: cleanup_failure.is_some(),
        },
        message,
    );
    match cleanup_failure {
        Some(cleanup) => facts.with_diagnostic(scrub_tail(
            &cleanup,
            &[
                (job.spec.input.as_path(), "<input>"),
                (transaction.staging.as_path(), "<staging>"),
                (transaction.final_path.as_path(), "<output>"),
            ],
        )),
        None => facts,
    }
}

pub(super) fn run_remux(
    services: JobServices<'_>,
    job: &ClaimedJob,
    output: PreparedOutput,
    tracker: &mut PhaseTracker,
) -> Result<(), String> {
    let PreparedOutput {
        manager,
        transaction,
    } = output;
    let handle = match remux::start(RemuxRequest {
        ffmpeg: services.tools.ffmpeg.clone(),
        input: job.spec.input.clone(),
        output: transaction.staging.clone(),
    }) {
        Ok(handle) => handle,
        Err(error) => {
            abandon_output(
                &manager,
                services.commands,
                job,
                &transaction,
                ItemOutcome::Failed(FailureFacts::new(
                    FailureKind::RemuxStart,
                    error.to_string(),
                )),
                None,
                tracker,
            )?;
            return Ok(());
        }
    };
    let report = wait_for_remux_report(
        services.commands,
        job.spec.run_id,
        handle,
        services.cancellation,
        tracker,
        services.input_duration_ms.map(|duration| duration as f64),
    );
    match report {
        Ok(RemuxReport {
            terminal: RemuxTerminal::Completed(_),
            final_telemetry,
        }) => finish_successful_output(
            services.commands,
            job,
            manager,
            transaction,
            SuccessfulJob::Remux,
            final_telemetry.map(remux_progress),
            tracker,
        )?,
        Ok(RemuxReport {
            terminal: RemuxTerminal::Cancelled,
            final_telemetry,
        }) => abandon_output(
            &manager,
            services.commands,
            job,
            &transaction,
            ItemOutcome::Stopped,
            map_progress(
                job.spec.run_id,
                tracker,
                JobPhase::Remuxing,
                final_telemetry.map(remux_progress),
            ),
            tracker,
        )?,
        Ok(RemuxReport {
            terminal: RemuxTerminal::Failed(failure),
            final_telemetry,
        }) => {
            let facts = FailureFacts::new(FailureKind::RemuxRun, failure.message).with_diagnostic(
                scrub_tail(
                    failure.stderr_tail.trim(),
                    &[
                        (job.spec.input.as_path(), "<input>"),
                        (transaction.staging.as_path(), "<staging>"),
                        (transaction.final_path.as_path(), "<output>"),
                    ],
                ),
            );
            abandon_output(
                &manager,
                services.commands,
                job,
                &transaction,
                ItemOutcome::Failed(facts),
                map_progress(
                    job.spec.run_id,
                    tracker,
                    JobPhase::Remuxing,
                    final_telemetry.map(remux_progress),
                ),
                tracker,
            )?;
        }
        Err(message) => abandon_output(
            &manager,
            services.commands,
            job,
            &transaction,
            ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, message)),
            None,
            tracker,
        )?,
    }
    Ok(())
}

pub(super) fn search_with_fallback(
    commands: &CommandSender,
    runtime: &AbAv1Runtime,
    tools: &MediaTools,
    cancellation: &ActiveCancellation,
    job: &ClaimedJob,
    tracker: &mut PhaseTracker,
) -> Result<AnalysisResult, ItemOutcome> {
    let execution = &job.spec.execution;
    let mut profile = execution.profile.clone();
    let mut target = execution.requested_target.0;
    let mut failed_attempts = Vec::new();
    loop {
        let request = SearchRequest {
            input: job.spec.input.clone(),
            target_vmaf: f32::from(target),
            max_encoded_percent: profile.max_encoded_percent_basis_points as f32
                / PERCENT_BASIS_POINTS_SCALE as f32,
            preset: profile.preset,
            samples: profile.samples,
            sample_duration: Duration::from_millis(profile.sample_duration_ms),
            thorough: profile.thorough,
            decode_mode: profile.decode_mode,
        };
        let handle = runtime
            .start_search(tools.clone(), request)
            .map_err(|error| {
                ItemOutcome::Failed(FailureFacts::new(
                    FailureKind::SearchStart,
                    error.to_string(),
                ))
            })?;
        let report = wait_for_report(
            commands,
            job.spec.run_id,
            handle,
            cancellation,
            JobPhase::Analyzing,
            tracker,
            Some(f64::from(NORMALIZED_PROGRESS_MAX)),
        )
        .map_err(|message| {
            ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, message))
        })?;
        match report.terminal {
            JobTerminal::Completed(outcome) => {
                let measured = measurement(outcome).map_err(|message| {
                    ItemOutcome::Failed(FailureFacts::new(FailureKind::SearchRun, message))
                })?;
                return Ok(AnalysisResult {
                    requested_target: execution.requested_target,
                    successful_target: VmafTarget(target),
                    fallback_floor: execution.fallback_floor,
                    fallback_step: execution.fallback_step,
                    failed_attempts,
                    measurement: measured,
                    profile: profile.clone(),
                });
            }
            JobTerminal::Failed(failure) => match failure.kind {
                JobFailureKind::NoGoodCrf { last } => {
                    let measured = measurement(last).map_err(|message| {
                        ItemOutcome::Failed(FailureFacts::new(FailureKind::SearchRun, message))
                    })?;
                    failed_attempts.push(AnalysisAttempt {
                        target: VmafTarget(target),
                        last_measurement: Some(measured),
                    });
                }
                JobFailureKind::Other => {
                    // Hardware→software retry (parse-free trigger): any
                    // non-NoGoodCrf search failure under hardware decode
                    // restarts the whole VMAF ladder once with the software
                    // profile. Attempts measured under hardware are discarded
                    // so the recorded result is honest about the profile it
                    // ran with; the widened `permitted_profiles` gate accepts
                    // the divergent profile while the JobSpec stays as
                    // requested.
                    if matches!(profile.decode_mode, DecodeMode::Hardware(_)) {
                        tracing::warn!(
                            "hardware-decode search failed ({}); retrying with software decode",
                            failure.message
                        );
                        profile.decode_mode = DecodeMode::Software;
                        target = execution.requested_target.0;
                        failed_attempts.clear();
                        continue;
                    }
                    return Err(ItemOutcome::Failed(FailureFacts::new(
                        FailureKind::SearchRun,
                        failure.message,
                    )));
                }
            },
            JobTerminal::Cancelled => return Err(ItemOutcome::Stopped),
            JobTerminal::Panicked { cleanup_failure } => {
                let facts = FailureFacts::new(
                    FailureKind::AdapterPanicked {
                        cleanup_failed: cleanup_failure.is_some(),
                    },
                    "analysis adapter panicked",
                );
                let facts = match cleanup_failure {
                    Some(cleanup) => facts.with_diagnostic(scrub_tail(
                        &cleanup,
                        &[(job.spec.input.as_path(), "<input>")],
                    )),
                    None => facts,
                };
                return Err(ItemOutcome::Failed(facts));
            }
        }
        if target <= execution.fallback_floor.0
            || execution.fallback_step == 0
            || target.saturating_sub(execution.fallback_step) < execution.fallback_floor.0
        {
            return Err(ItemOutcome::NotWorthwhile {
                attempts: failed_attempts,
            });
        }
        target = target.saturating_sub(execution.fallback_step);
    }
}

fn wait_for_report<T>(
    commands: &CommandSender,
    run_id: RunId,
    mut handle: JobHandle<T>,
    cancellation: &ActiveCancellation,
    phase: JobPhase,
    tracker: &mut PhaseTracker,
    total_work: Option<f64>,
) -> Result<JobReport<T>, String> {
    tracker.enter(phase);
    let _registration = cancellation.register(
        run_id,
        ActiveJobCancellation::AbAv1(handle.cancellation_handle()),
    );
    let started = Instant::now();
    let mut rates = RateTracker::new(total_work);
    let mut last_update = None;
    let mut last_published = None;
    loop {
        match handle
            .recv_report(ADAPTER_REPORT_POLL_INTERVAL)
            .map_err(|error| error.to_string())?
        {
            Some(report) => {
                return Ok(report);
            }
            None => {
                // Only a changed adapter value becomes a rate sample: the
                // poll re-reads the latest value every interval, and feeding
                // repeats would drag the window's slope down between the
                // adapter's real updates.
                if let Some(update) = handle.latest_telemetry()
                    && last_update.as_ref() != Some(&update)
                {
                    let elapsed = started.elapsed();
                    rates.record(elapsed, &rate_sample(&update));
                    let progress = telemetry_progress(&update);
                    last_update = Some(update);
                    let published = (progress, rates.fps_centi(), rates.eta_ms(elapsed));
                    if last_published.as_ref() != Some(&published) {
                        commands.publish_telemetry(Telemetry {
                            run_id,
                            sequence: tracker.next_sequence(),
                            phase,
                            progress: published.0.clone(),
                            fps_centi: published.1,
                            eta_ms: published.2,
                        });
                        last_published = Some(published);
                    }
                }
            }
        }
    }
}

fn wait_for_remux_report(
    commands: &CommandSender,
    run_id: RunId,
    mut handle: RemuxHandle,
    cancellation: &ActiveCancellation,
    tracker: &mut PhaseTracker,
    total_work: Option<f64>,
) -> Result<RemuxReport, String> {
    tracker.enter(JobPhase::Remuxing);
    let _registration = cancellation.register(
        run_id,
        ActiveJobCancellation::Remux(handle.cancellation_handle()),
    );
    let started = Instant::now();
    let mut rates = RateTracker::new(total_work);
    let mut last_update = None;
    let mut last_published = None;
    loop {
        match handle
            .recv_report(ADAPTER_REPORT_POLL_INTERVAL)
            .map_err(|error| error.to_string())?
        {
            Some(report) => return Ok(report),
            None => {
                if let Some(update) = handle.latest_telemetry()
                    && last_update != Some(update)
                {
                    let elapsed = started.elapsed();
                    // A remux reports no frame rate; only the position feeds
                    // the window, so fps stays absent and the ETA works.
                    rates.record(
                        elapsed,
                        &RateSample {
                            frames: None,
                            fps_gauge: None,
                            work_done: update.position_ms as f64,
                        },
                    );
                    last_update = Some(update);
                    let published = (
                        remux_progress(update),
                        rates.fps_centi(),
                        rates.eta_ms(elapsed),
                    );
                    if last_published.as_ref() != Some(&published) {
                        commands.publish_telemetry(Telemetry {
                            run_id,
                            sequence: tracker.next_sequence(),
                            phase: JobPhase::Remuxing,
                            progress: published.0.clone(),
                            fps_centi: published.1,
                            eta_ms: published.2,
                        });
                        last_published = Some(published);
                    }
                }
            }
        }
    }
}
