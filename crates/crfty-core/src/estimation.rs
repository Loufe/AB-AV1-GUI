//! Job-duration estimation from an analysis prediction or historical rates.
//!
//! Two additive sources, best first:
//!
//! - **Analysis prediction** — ab-av1's own encode-duration prediction from a
//!   completed CRF search (`SearchMeasurement::predicted_duration_ms`). The
//!   caller selects the qualifying analysis for the job's execution settings
//!   via [`crate::select_analysis`]; estimation trusts that selection and
//!   never re-derives freshness.
//! - **Historical rates** — per-file rate = measured phase time / video
//!   duration, grouped along a specificity ladder: (codec, resolution bucket)
//!   → codec → global. The first group holding at least
//!   [`MIN_GROUP_SAMPLES`] samples answers with its exclusive (type-6)
//!   quartiles scaled by the video's duration. The ladder, thresholds,
//!   bucket cut-offs, and quantile method match the V2 Python estimator;
//!   its pinned quartile fixtures are re-pinned in the tests here.
//!
//! Convert estimates use encoding time, analyze estimates use CRF-search
//! time. Rate samples come from [`StatFact`]s. Parked imported records are
//! deliberately excluded because they have no content identity; an adopted
//! Converted verdict can contribute through the ordinary fact path. Imported
//! Analyzed summaries remain display-only. Native analyzed-only runs also
//! supply search samples despite producing no verdict and therefore no fact.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    AnalysisResult, DurableState, ItemOutcome, Operation, VideoCodec, VideoMeta,
    collect_stat_facts, projection::phase_totals,
};

/// A group answers only once it holds this many rate samples; sparser groups
/// defer to the next ladder tier.
const MIN_GROUP_SAMPLES: usize = 5;
/// A (codec, resolution) group at or above this many samples upgrades the
/// estimate's confidence from rough to estimate.
const HIGH_CONFIDENCE_SAMPLES: u32 = 10;

const UHD_4K_MIN_PIXELS: u64 = 8_294_400;
const QHD_1440_MIN_PIXELS: u64 = 3_686_400;
const FHD_1080_MIN_PIXELS: u64 = 2_073_600;
const HD_720_MIN_PIXELS: u64 = 921_600;

/// Resolution class by pixel count, so anamorphic and cropped variants group
/// with the class they cost like. The product is rotation-invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, specta::Type)]
pub enum ResolutionBucket {
    Sd,
    Hd720,
    Hd1080,
    Qhd1440,
    Uhd4k,
}

impl ResolutionBucket {
    /// `None` when either dimension is unknown (zero): such samples still
    /// feed the codec and global tiers, they just never form a resolution
    /// group.
    #[must_use]
    pub fn from_dimensions(width: u32, height: u32) -> Option<Self> {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        if pixels == 0 {
            return None;
        }
        Some(if pixels >= UHD_4K_MIN_PIXELS {
            Self::Uhd4k
        } else if pixels >= QHD_1440_MIN_PIXELS {
            Self::Qhd1440
        } else if pixels >= FHD_1080_MIN_PIXELS {
            Self::Hd1080
        } else if pixels >= HD_720_MIN_PIXELS {
            Self::Hd720
        } else {
            Self::Sd
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quartiles {
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
}

/// Exclusive (type-6) quartiles: rank `q * (n + 1)`, linearly interpolated,
/// clamped to the extremes. `None` when `values` is empty. Input order does
/// not matter.
#[must_use]
pub fn exclusive_quartiles(values: &[f64]) -> Option<Quartiles> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Quartiles {
        p25: exclusive_quantile(&sorted, 0.25)?,
        p50: exclusive_quantile(&sorted, 0.5)?,
        p75: exclusive_quantile(&sorted, 0.75)?,
    })
}

fn exclusive_quantile(sorted: &[f64], q: f64) -> Option<f64> {
    let first = *sorted.first()?;
    let last = *sorted.last()?;
    let count = sorted.len() as f64;
    let rank = q * (count + 1.0);
    if rank <= 1.0 {
        return Some(first);
    }
    if rank >= count {
        return Some(last);
    }
    let below = rank.floor();
    let fraction = rank - below;
    let lower_index = below as usize - 1;
    let lower = *sorted.get(lower_index)?;
    let upper = *sorted.get(lower_index + 1)?;
    Some(lower + fraction * (upper - lower))
}

/// Which ladder tier answered a historical estimate, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub enum HistoricalTier {
    CodecResolution,
    Codec,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum EstimateBasis {
    /// ab-av1's own prediction from a completed CRF search of this content.
    AnalysisPrediction,
    Historical {
        tier: HistoricalTier,
        samples: u32,
    },
}

/// Presentation classes for an estimate's trustworthiness; the UI maps these
/// to its exact/estimate/rough markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub enum EstimateConfidence {
    Exact,
    Estimate,
    Rough,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct TimeEstimate {
    #[specta(type = crate::JsNumber)]
    pub min_ms: u64,
    #[specta(type = crate::JsNumber)]
    pub best_ms: u64,
    #[specta(type = crate::JsNumber)]
    pub max_ms: u64,
    pub basis: EstimateBasis,
}

impl TimeEstimate {
    #[must_use]
    pub fn confidence(&self) -> EstimateConfidence {
        match &self.basis {
            EstimateBasis::AnalysisPrediction => EstimateConfidence::Exact,
            EstimateBasis::Historical {
                tier: HistoricalTier::CodecResolution,
                samples,
            } if *samples >= HIGH_CONFIDENCE_SAMPLES => EstimateConfidence::Estimate,
            EstimateBasis::Historical { .. } => EstimateConfidence::Rough,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct GroupedRates {
    by_codec_and_bucket: BTreeMap<VideoCodec, BTreeMap<ResolutionBucket, Vec<f64>>>,
    by_codec: BTreeMap<VideoCodec, Vec<f64>>,
    all: Vec<f64>,
}

impl GroupedRates {
    fn insert(&mut self, codec: &VideoCodec, bucket: Option<ResolutionBucket>, rate: f64) {
        if let Some(bucket) = bucket {
            self.by_codec_and_bucket
                .entry(codec.clone())
                .or_default()
                .entry(bucket)
                .or_default()
                .push(rate);
        }
        self.by_codec.entry(codec.clone()).or_default().push(rate);
        self.all.push(rate);
    }

    fn lookup(
        &self,
        codec: &VideoCodec,
        bucket: Option<ResolutionBucket>,
    ) -> Option<(HistoricalTier, &[f64])> {
        if let Some(bucket) = bucket
            && let Some(rates) = self
                .by_codec_and_bucket
                .get(codec)
                .and_then(|buckets| buckets.get(&bucket))
            && rates.len() >= MIN_GROUP_SAMPLES
        {
            return Some((HistoricalTier::CodecResolution, rates));
        }
        if let Some(rates) = self.by_codec.get(codec)
            && rates.len() >= MIN_GROUP_SAMPLES
        {
            return Some((HistoricalTier::Codec, rates));
        }
        if self.all.len() >= MIN_GROUP_SAMPLES {
            return Some((HistoricalTier::Global, &self.all));
        }
        None
    }
}

/// Historical rate groups for both operations, built once per estimation
/// round from durable state. Cheap to rebuild — no caching until profiling
/// says otherwise.
#[derive(Debug, Clone, Default)]
pub struct EstimationModel {
    convert: GroupedRates,
    analyze: GroupedRates,
}

impl EstimationModel {
    #[must_use]
    pub fn from_state(state: &DurableState) -> Self {
        let mut model = Self::default();
        for fact in collect_stat_facts(state) {
            let bucket = ResolutionBucket::from_dimensions(fact.width, fact.height);
            if let Some(rate) = rate(fact.encoding_ms, fact.duration_ms) {
                model.convert.insert(&fact.codec, bucket, rate);
            }
            if let Some(rate) = rate(fact.analyzing_ms, fact.duration_ms) {
                model.analyze.insert(&fact.codec, bucket, rate);
            }
        }
        // Analyzed-only runs decide no verdict and so produce no fact, but
        // their search time is a real observation. They cannot double-count
        // fact sources: only Converted/Remuxed/NotWorthwhile runs back
        // verdicts.
        for run in state.conversion_runs.values() {
            if !matches!(run.outcome, Some(ItemOutcome::Analyzed)) {
                continue;
            }
            let Some(record) = run
                .spec
                .content_key
                .as_ref()
                .and_then(|content_key| state.records.get(content_key))
            else {
                continue;
            };
            let (analyzing_ms, _) = phase_totals(Some(run));
            if let Some(rate) = rate(analyzing_ms, record.metadata.duration_ms) {
                let metadata = &record.metadata;
                let bucket = ResolutionBucket::from_dimensions(metadata.width, metadata.height);
                model.analyze.insert(&metadata.codec, bucket, rate);
            }
        }
        model
    }

    /// Estimate how long `operation` will take on a video, best source first:
    /// a caller-selected fresh analysis (Convert only — the prediction covers
    /// encoding), then the historical ladder, then `None`.
    #[must_use]
    pub fn estimate(
        &self,
        operation: Operation,
        metadata: &VideoMeta,
        fresh_analysis: Option<&AnalysisResult>,
    ) -> Option<TimeEstimate> {
        if operation == Operation::Convert
            && let Some(analysis) = fresh_analysis
            && analysis.measurement.predicted_duration_ms > 0
        {
            let predicted_ms = analysis.measurement.predicted_duration_ms;
            return Some(TimeEstimate {
                min_ms: predicted_ms,
                best_ms: predicted_ms,
                max_ms: predicted_ms,
                basis: EstimateBasis::AnalysisPrediction,
            });
        }
        if metadata.duration_ms == 0 {
            return None;
        }
        let rates = match operation {
            Operation::Convert => &self.convert,
            Operation::Analyze => &self.analyze,
        };
        let bucket = ResolutionBucket::from_dimensions(metadata.width, metadata.height);
        let (tier, samples) = rates.lookup(&metadata.codec, bucket)?;
        let quartiles = exclusive_quartiles(samples)?;
        let duration_ms = metadata.duration_ms as f64;
        Some(TimeEstimate {
            min_ms: scale(duration_ms, quartiles.p25),
            best_ms: scale(duration_ms, quartiles.p50),
            max_ms: scale(duration_ms, quartiles.p75),
            basis: EstimateBasis::Historical {
                tier,
                samples: u32::try_from(samples.len()).unwrap_or(u32::MAX),
            },
        })
    }
}

/// Phase time per second of video; `None` keeps unmeasured and zero-duration
/// samples out of every group.
fn rate(elapsed_ms: u64, duration_ms: u64) -> Option<f64> {
    if elapsed_ms == 0 || duration_ms == 0 {
        return None;
    }
    Some(elapsed_ms as f64 / duration_ms as f64)
}

fn scale(duration_ms: f64, rate: f64) -> u64 {
    (duration_ms * rate).round() as u64
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;

    use super::*;
    use crate::{
        AnalysisProfile, ClaimId, CompletionEvidence, ContentKey, ConversionRun, Crf, DecodeMode,
        DurationMs, ExecutionSettings, FileRecord, JobAction, JobPhase, JobSpec, MediaContainer,
        OutputTarget, PhaseSpan, QueueItemId, RunId, SearchMeasurement, StreamByteSizes,
        UnixMillis, Verdict, VerdictKind, VideoMeta, VmafScore, VmafTarget,
    };

    fn meta(codec: VideoCodec, width: u32, height: u32, duration_ms: u64) -> VideoMeta {
        VideoMeta {
            codec,
            container: MediaContainer::Matroska,
            width,
            height,
            rotation_degrees: 0,
            duration_ms,
            size_bytes: 1_000_000,
            audio: Vec::new(),
            subtitle_count: 0,
        }
    }

    fn spec(run: u64, content_key: &ContentKey, operation: Operation) -> JobSpec {
        JobSpec {
            item_id: QueueItemId(run),
            claim_id: ClaimId(run),
            run_id: RunId(run),
            input: PathBuf::from(format!("input-{run}.mkv")),
            content_key: Some(content_key.clone()),
            operation,
            intent: crate::AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Suffix {
                suffix: "-av1".to_owned(),
            },
            execution: ExecutionSettings::production(AnalysisProfile::production(), false),
            action: JobAction::Encode {
                selected_analysis: None,
            },
        }
    }

    fn spans(analyzing_ms: u64, encoding_ms: u64) -> Vec<PhaseSpan> {
        let mut spans = Vec::new();
        if analyzing_ms > 0 {
            spans.push(PhaseSpan {
                phase: JobPhase::Analyzing,
                duration: DurationMs(analyzing_ms),
            });
        }
        if encoding_ms > 0 {
            spans.push(PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(encoding_ms),
            });
        }
        spans
    }

    fn run_shell(run: u64, content_key: &ContentKey, operation: Operation) -> ConversionRun {
        ConversionRun {
            spec: spec(run, content_key, operation),
            analysis: None,
            output_content_key: None,
            outcome: None,
            started_at: Some(UnixMillis(0)),
            finished_at: Some(UnixMillis(1_000_000)),
            phase_spans: Vec::new(),
        }
    }

    fn add_converted(
        state: &mut DurableState,
        run: u64,
        source: VideoMeta,
        analyzing_ms: u64,
        encoding_ms: u64,
    ) {
        let content_key = ContentKey(format!("content-{run:04}"));
        let mut record = FileRecord::new(source);
        record.verdict = Some(Verdict {
            kind: VerdictKind::Converted {
                output_content_key: Some(ContentKey(format!("out-{run:04}"))),
                input_size: None,
                output_size: None,
                encoding_time: None,
                crf: None,
                vmaf: None,
                target: None,
            },
            source_run: Some(RunId(run)),
            decided_at: UnixMillis(1_000_000),
        });
        state.records.insert(content_key.clone(), record);
        let mut conversion = run_shell(run, &content_key, Operation::Convert);
        conversion.outcome = Some(ItemOutcome::Converted(CompletionEvidence::LiveEncode {
            input_size: 1_000_000,
            output_size: 400_000,
            stream_sizes: StreamByteSizes {
                video: 400_000,
                audio: 0,
                subtitle: 0,
                other: 0,
            },
            encode_decode: DecodeMode::Software,
        }));
        conversion.phase_spans = spans(analyzing_ms, encoding_ms);
        state.conversion_runs.insert(RunId(run), conversion);
    }

    fn add_analyzed(state: &mut DurableState, run: u64, source: VideoMeta, analyzing_ms: u64) {
        let content_key = ContentKey(format!("content-{run:04}"));
        state
            .records
            .insert(content_key.clone(), FileRecord::new(source));
        let mut analyzed = run_shell(run, &content_key, Operation::Analyze);
        analyzed.outcome = Some(ItemOutcome::Analyzed);
        analyzed.phase_spans = spans(analyzing_ms, 0);
        state.conversion_runs.insert(RunId(run), analyzed);
    }

    fn add_not_worthwhile(
        state: &mut DurableState,
        run: u64,
        source: VideoMeta,
        analyzing_ms: u64,
    ) {
        let content_key = ContentKey(format!("content-{run:04}"));
        let mut record = FileRecord::new(source);
        record.verdict = Some(Verdict {
            kind: VerdictKind::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
            },
            source_run: Some(RunId(run)),
            decided_at: UnixMillis(1_000_000),
        });
        state.records.insert(content_key.clone(), record);
        let mut declined = run_shell(run, &content_key, Operation::Convert);
        declined.outcome = Some(ItemOutcome::NotWorthwhile {
            attempts: Vec::new(),
        });
        declined.phase_spans = spans(analyzing_ms, 0);
        state.conversion_runs.insert(RunId(run), declined);
    }

    fn analysis_with_prediction(predicted_duration_ms: u64) -> crate::AnalysisResult {
        crate::AnalysisResult {
            requested_target: VmafTarget(95),
            successful_target: VmafTarget(95),
            fallback_floor: VmafTarget(90),
            fallback_step: 1,
            failed_attempts: Vec::new(),
            measurement: SearchMeasurement {
                crf: Crf(24_000),
                score: VmafScore(9_512),
                predicted_size: 400_000,
                predicted_percent_basis_points: 4_000,
                predicted_duration_ms,
                from_cache: false,
            },
            profile: AnalysisProfile::production(),
        }
    }

    /// Five HEVC 1080p conversions of a 600s video at rates 1..=5.
    fn hevc_ladder_state() -> DurableState {
        let mut state = DurableState::default();
        for step in 1..=5u64 {
            add_converted(
                &mut state,
                step,
                meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                0,
                step * 600_000,
            );
        }
        state
    }

    #[test]
    fn resolution_buckets_match_the_pixel_cutoffs() {
        let bucket = |width, height| ResolutionBucket::from_dimensions(width, height);
        assert_eq!(bucket(3840, 2160), Some(ResolutionBucket::Uhd4k));
        assert_eq!(bucket(2560, 1440), Some(ResolutionBucket::Qhd1440));
        assert_eq!(bucket(1920, 1080), Some(ResolutionBucket::Hd1080));
        assert_eq!(bucket(1280, 720), Some(ResolutionBucket::Hd720));
        assert_eq!(bucket(720, 480), Some(ResolutionBucket::Sd));
        // Rotation-invariant: the product decides, not the orientation.
        assert_eq!(bucket(1080, 1920), Some(ResolutionBucket::Hd1080));
        assert_eq!(bucket(0, 1080), None);
    }

    #[test]
    fn quartiles_match_the_pinned_python_fixture() {
        // Pinned in main:tests/test_estimation.py — exclusive method.
        let quartiles = exclusive_quartiles(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap();
        assert_eq!(quartiles.p25, 1.5);
        assert_eq!(quartiles.p50, 3.0);
        assert_eq!(quartiles.p75, 4.5);
    }

    #[test]
    fn quartiles_interpolate_and_clamp() {
        // n=4: ranks 1.25 / 2.5 / 3.75.
        let four = exclusive_quartiles(&[1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(four.p25, 1.25);
        assert_eq!(four.p50, 2.5);
        assert_eq!(four.p75, 3.75);
        // n=2: p25 clamps to the first value, p75 to the last.
        let two = exclusive_quartiles(&[10.0, 20.0]).unwrap();
        assert_eq!(two.p25, 10.0);
        assert_eq!(two.p50, 15.0);
        assert_eq!(two.p75, 20.0);
        let one = exclusive_quartiles(&[7.0]).unwrap();
        assert_eq!((one.p25, one.p50, one.p75), (7.0, 7.0, 7.0));
        assert_eq!(exclusive_quartiles(&[]), None);
    }

    #[test]
    fn quartiles_ignore_input_order() {
        assert_eq!(
            exclusive_quartiles(&[5.0, 1.0, 4.0, 2.0, 3.0]),
            exclusive_quartiles(&[1.0, 2.0, 3.0, 4.0, 5.0]),
        );
    }

    #[test]
    fn fresh_analysis_prediction_wins_for_convert() {
        let model = EstimationModel::from_state(&hevc_ladder_state());
        let analysis = analysis_with_prediction(123_456);
        let estimate = model
            .estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                Some(&analysis),
            )
            .unwrap();
        assert_eq!(estimate.basis, EstimateBasis::AnalysisPrediction);
        assert_eq!(
            (estimate.min_ms, estimate.best_ms, estimate.max_ms),
            (123_456, 123_456, 123_456)
        );
        assert_eq!(estimate.confidence(), EstimateConfidence::Exact);
    }

    #[test]
    fn analysis_prediction_never_answers_analyze_estimates() {
        // The prediction covers encoding; analyze estimates fall through to
        // the ladder, which has no analyze samples here.
        let model = EstimationModel::from_state(&hevc_ladder_state());
        let analysis = analysis_with_prediction(123_456);
        assert_eq!(
            model.estimate(
                Operation::Analyze,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                Some(&analysis),
            ),
            None
        );
    }

    #[test]
    fn historical_estimate_scales_quartiles_by_duration() {
        let model = EstimationModel::from_state(&hevc_ladder_state());
        let estimate = model
            .estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                None,
            )
            .unwrap();
        // Rates 1..=5 → quartiles 1.5 / 3.0 / 4.5 scaled by 600s.
        assert_eq!(estimate.min_ms, 900_000);
        assert_eq!(estimate.best_ms, 1_800_000);
        assert_eq!(estimate.max_ms, 2_700_000);
        assert_eq!(
            estimate.basis,
            EstimateBasis::Historical {
                tier: HistoricalTier::CodecResolution,
                samples: 5,
            }
        );
        assert_eq!(estimate.confidence(), EstimateConfidence::Rough);
    }

    #[test]
    fn ladder_falls_back_by_bucket_then_codec() {
        let model = EstimationModel::from_state(&hevc_ladder_state());
        // Same codec, different bucket: the resolution group is empty, the
        // codec group answers.
        let by_codec = model
            .estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 3840, 2160, 600_000),
                None,
            )
            .unwrap();
        assert_eq!(
            by_codec.basis,
            EstimateBasis::Historical {
                tier: HistoricalTier::Codec,
                samples: 5,
            }
        );
        // Different codec: only the global pool remains.
        let global = model
            .estimate(
                Operation::Convert,
                &meta(VideoCodec::H264, 1920, 1080, 600_000),
                None,
            )
            .unwrap();
        assert_eq!(
            global.basis,
            EstimateBasis::Historical {
                tier: HistoricalTier::Global,
                samples: 5,
            }
        );
    }

    #[test]
    fn sparse_history_estimates_nothing() {
        let mut state = DurableState::default();
        for step in 1..=4u64 {
            add_converted(
                &mut state,
                step,
                meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                0,
                step * 600_000,
            );
        }
        let model = EstimationModel::from_state(&state);
        assert_eq!(
            model.estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                None,
            ),
            None
        );
    }

    #[test]
    fn ten_resolution_samples_upgrade_confidence() {
        let mut state = DurableState::default();
        for step in 1..=10u64 {
            add_converted(
                &mut state,
                step,
                meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                0,
                step * 100_000,
            );
        }
        let model = EstimationModel::from_state(&state);
        let precise = model
            .estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                None,
            )
            .unwrap();
        assert_eq!(precise.confidence(), EstimateConfidence::Estimate);
        // The same ten samples through the codec tier stay rough: only the
        // resolution-specific group earns the upgrade.
        let coarse = model
            .estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 3840, 2160, 600_000),
                None,
            )
            .unwrap();
        assert_eq!(coarse.confidence(), EstimateConfidence::Rough);
    }

    #[test]
    fn analyze_rates_come_from_facts_and_analyzed_runs() {
        let mut state = DurableState::default();
        // Two not-worthwhile verdicts, one converted fact with search time,
        // and two analyzed-only runs: five analyze samples, zero convert
        // samples short of a group... (the converted fact also adds one
        // convert sample, still below the threshold).
        add_not_worthwhile(
            &mut state,
            1,
            meta(VideoCodec::Hevc, 1920, 1080, 600_000),
            600_000,
        );
        add_not_worthwhile(
            &mut state,
            2,
            meta(VideoCodec::Hevc, 1920, 1080, 600_000),
            1_200_000,
        );
        add_converted(
            &mut state,
            3,
            meta(VideoCodec::Hevc, 1920, 1080, 600_000),
            1_800_000,
            2_400_000,
        );
        add_analyzed(
            &mut state,
            4,
            meta(VideoCodec::Hevc, 1920, 1080, 600_000),
            2_400_000,
        );
        add_analyzed(
            &mut state,
            5,
            meta(VideoCodec::Hevc, 1920, 1080, 600_000),
            3_000_000,
        );
        let model = EstimationModel::from_state(&state);
        let estimate = model
            .estimate(
                Operation::Analyze,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                None,
            )
            .unwrap();
        // Analyze rates 1, 2, 3, 4, 5 → best = 3.0 × 600s.
        assert_eq!(estimate.best_ms, 1_800_000);
        assert_eq!(
            estimate.basis,
            EstimateBasis::Historical {
                tier: HistoricalTier::CodecResolution,
                samples: 5,
            }
        );
        // One convert sample is not a group.
        assert_eq!(
            model.estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                None,
            ),
            None
        );
    }

    #[test]
    fn zero_durations_produce_no_samples_and_no_estimates() {
        let mut state = DurableState::default();
        for step in 1..=5u64 {
            add_converted(
                &mut state,
                step,
                meta(VideoCodec::Hevc, 1920, 1080, 0),
                0,
                600_000,
            );
        }
        let model = EstimationModel::from_state(&state);
        assert_eq!(
            model.estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 1920, 1080, 600_000),
                None,
            ),
            None
        );
        // A zero-duration request cannot be scaled either.
        let populated = EstimationModel::from_state(&hevc_ladder_state());
        assert_eq!(
            populated.estimate(
                Operation::Convert,
                &meta(VideoCodec::Hevc, 1920, 1080, 0),
                None,
            ),
            None
        );
    }

    proptest! {
        #[test]
        fn quartiles_are_ordered(
            values in proptest::collection::vec(0.001f64..1_000.0, 1..50)
        ) {
            let quartiles = exclusive_quartiles(&values).unwrap();
            prop_assert!(quartiles.p25 <= quartiles.p50);
            prop_assert!(quartiles.p50 <= quartiles.p75);
            let smallest = values.iter().copied().fold(f64::INFINITY, f64::min);
            let largest = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            prop_assert!(quartiles.p25 >= smallest);
            prop_assert!(quartiles.p75 <= largest);
        }

        #[test]
        fn estimates_scale_linearly_with_duration(duration_ms in 1_000u64..10_000_000) {
            let model = EstimationModel::from_state(&hevc_ladder_state());
            let estimate = model
                .estimate(
                    Operation::Convert,
                    &meta(VideoCodec::Hevc, 1920, 1080, duration_ms),
                    None,
                )
                .unwrap();
            // Rates 1..=5: best = duration × 3.0 exactly.
            prop_assert_eq!(estimate.best_ms, duration_ms * 3);
            prop_assert!(estimate.min_ms <= estimate.best_ms);
            prop_assert!(estimate.best_ms <= estimate.max_ms);
        }
    }
}
