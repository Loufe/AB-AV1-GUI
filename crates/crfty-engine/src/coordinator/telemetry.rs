//! Pure conversions from adapter facts to domain values: progress,
//! throughput samples, and the measurement a completed search reports.

use crfty_core::{
    CRF_FIXED_SCALE, Crf, JobProgress, MAX_PERCENT_BASIS_POINTS, MAX_VMAF_SCORE,
    PERCENT_BASIS_POINTS_SCALE, SearchMeasurement, VMAF_SCORE_FIXED_SCALE, VmafScore,
};

use crate::{
    ab_av1::{SearchOutcome, Telemetry as AdapterTelemetry},
    rate::RateSample,
    remux::RemuxTelemetry,
};

const NORMALIZED_PROGRESS_MIN: f32 = 0.0;

pub(super) const NORMALIZED_PROGRESS_MAX: f32 = 1.0;

/// Normalizes one adapter update for the rate window: search progress runs
/// toward [`NORMALIZED_PROGRESS_MAX`] with a reported fps gauge, an encode's
/// output position runs toward the input duration with a frame counter (the
/// reported gauge covers the window's first sample).
pub(super) fn rate_sample(update: &AdapterTelemetry) -> RateSample {
    match update {
        AdapterTelemetry::Search(search) => RateSample {
            frames: None,
            fps_gauge: Some(search.fps),
            work_done: f64::from(
                search
                    .progress
                    .clamp(NORMALIZED_PROGRESS_MIN, NORMALIZED_PROGRESS_MAX),
            ),
        },
        AdapterTelemetry::Encode(encode) => RateSample {
            frames: Some(encode.frame),
            fps_gauge: Some(encode.fps),
            work_done: encode.position.as_millis() as f64,
        },
    }
}

pub(super) fn telemetry_progress(telemetry: &AdapterTelemetry) -> JobProgress {
    match telemetry {
        AdapterTelemetry::Search(search) => JobProgress::SearchBasisPoints(
            (search
                .progress
                .clamp(NORMALIZED_PROGRESS_MIN, NORMALIZED_PROGRESS_MAX)
                * MAX_PERCENT_BASIS_POINTS as f32) as u32,
        ),
        AdapterTelemetry::Encode(encode) => JobProgress::OutputPositionMs(
            encode.position.as_millis().try_into().unwrap_or(u64::MAX),
        ),
    }
}

pub(super) fn remux_progress(telemetry: RemuxTelemetry) -> JobProgress {
    JobProgress::OutputPositionMs(telemetry.position_ms)
}

pub(super) fn measurement(outcome: SearchOutcome) -> Result<SearchMeasurement, String> {
    if !outcome.crf.is_finite()
        || outcome.crf < 0.0
        || !outcome.vmaf.is_finite()
        || !(0.0..=f32::from(MAX_VMAF_SCORE)).contains(&outcome.vmaf)
        || !outcome.predicted_percent.is_finite()
        || outcome.predicted_percent < 0.0
    {
        return Err("ab-av1 returned a non-finite or out-of-range analysis value".to_owned());
    }
    let scaled_crf = outcome.crf * CRF_FIXED_SCALE as f32;
    let scaled_percent = outcome.predicted_percent * f64::from(PERCENT_BASIS_POINTS_SCALE);
    if scaled_crf > u32::MAX as f32 || scaled_percent > f64::from(u32::MAX) {
        return Err("ab-av1 returned an analysis value too large to persist".to_owned());
    }
    Ok(SearchMeasurement {
        crf: Crf((outcome.crf * CRF_FIXED_SCALE as f32).round().max(0.0) as u32),
        score: VmafScore(
            (outcome.vmaf * f32::from(VMAF_SCORE_FIXED_SCALE))
                .round()
                .clamp(
                    0.0,
                    f32::from(VMAF_SCORE_FIXED_SCALE) * f32::from(MAX_VMAF_SCORE),
                ) as u16,
        ),
        predicted_size: outcome.predicted_size,
        predicted_percent_basis_points: scaled_percent.round() as u32,
        predicted_duration_ms: outcome.predicted_duration.as_millis() as u64,
        from_cache: outcome.from_cache,
    })
}

pub(super) fn crf_to_f32(crf: Crf) -> f32 {
    crf.0 as f32 / CRF_FIXED_SCALE as f32
}
