//! Pure read-model projections over durable state: the flattened per-content
//! facts, the Statistics aggregates, and the History rows derived from them.
//!
//! Everything here is a context-free function of [`DurableState`] — no clock,
//! no filesystem, no caching. The reducer publishes [`StatisticsPayload`] as
//! an ephemeral delta answering a statistics request; [`HistoryRow`] is the
//! oracle definition mirrored by the frontend against exported fixtures and
//! never crosses IPC itself.
//!
//! Semantics deliberately diverge from the Python application where V2
//! behavior was an accident of loose `None` handling; the projection ADR
//! enumerates each divergence. The load-bearing ones: savings totals and the
//! cumulative series share one both-sizes-known rule, negative savings are
//! represented (never clamped into a bin), dates come from run completion,
//! and remux outcomes stay out of conversion-savings/VMAF aggregates.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    AnalysisResult, AudioCodec, CompletionEvidence, ContentKey, ConversionRun, Crf, DurableState,
    FailureKind, FileRecord, ImportedHistoryRecord, ItemOutcome, JobPhase, MediaContainer,
    OutputState, ParkedStatus, RunId, UnixMillis, VerdictKind, VideoCodec, VmafScore, VmafTarget,
};

const MILLIS_PER_DAY: i128 = 86_400_000;
const MILLIS_PER_MINUTE: i128 = 60_000;
const REDUCTION_BIN_COUNT: usize = 10;
const REDUCTION_BIN_WIDTH_PERCENT: f64 = 10.0;
const BYTES_PER_GIB: f64 = 1_073_741_824.0;
const MILLIS_PER_HOUR: f64 = 3_600_000.0;

/// What the current verdict says happened to this content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatFactKind {
    Converted,
    Remuxed,
    NotWorthwhile {
        requested: VmafTarget,
        floor: VmafTarget,
    },
}

/// One flattened fact per content with a standing verdict: the joined sizes,
/// measured times, and media facts that Statistics and estimation consume.
/// Adopted verdicts (#39) have no backing run; their summary comes from the
/// verdict-carried fields the fold absorbed at adoption time.
#[derive(Debug, Clone, PartialEq)]
pub struct StatFact {
    pub content_key: ContentKey,
    pub kind: StatFactKind,
    pub source_run: Option<RunId>,
    /// When the deciding run finished; falls back to the verdict decision
    /// time when the run itself is no longer present.
    pub finished_at: UnixMillis,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub input_size_bytes: Option<u64>,
    pub output_size_bytes: Option<u64>,
    pub analyzing_ms: u64,
    pub encoding_ms: u64,
    pub vmaf: Option<VmafScore>,
    pub crf: Option<Crf>,
}

impl StatFact {
    /// Savings in bytes when both sizes are known. Negative when the output
    /// grew; `None` never contributes to savings aggregates.
    #[must_use]
    pub fn saved_bytes(&self) -> Option<i128> {
        let input = self.input_size_bytes?;
        let output = self.output_size_bytes?;
        Some(i128::from(input) - i128::from(output))
    }

    /// Size reduction as a percentage of the input when both sizes are known
    /// and the input is nonempty. Negative when the output grew.
    #[must_use]
    pub fn reduction_percent(&self) -> Option<f64> {
        let input = self.input_size_bytes?;
        let output = self.output_size_bytes?;
        if input == 0 {
            return None;
        }
        let saved = i128::from(input) - i128::from(output);
        Some(100.0 * saved as f64 / input as f64)
    }
}

/// Flatten every content with a standing verdict into one [`StatFact`],
/// joining sizes in evidence → settled-transaction → metadata order.
#[must_use]
pub fn collect_stat_facts(state: &DurableState) -> Vec<StatFact> {
    let mut facts = Vec::new();
    for (content_key, record) in &state.records {
        let Some(verdict) = &record.verdict else {
            continue;
        };
        let run = verdict
            .source_run
            .and_then(|run_id| state.conversion_runs.get(&run_id));
        let (input_size_bytes, output_size_bytes) = joined_sizes(verdict, run, state, record);
        let (analyzing_ms, encoding_ms) = phase_totals(run);
        let measurement = run
            .and_then(|run| run.analysis.as_ref())
            .map(|analysis| &analysis.measurement);
        let (carried_encoding, carried_crf, carried_vmaf) = match &verdict.kind {
            VerdictKind::Converted {
                encoding_time,
                crf,
                vmaf,
                ..
            } => (*encoding_time, *crf, *vmaf),
            VerdictKind::Remuxed { .. } | VerdictKind::NotWorthwhile { .. } => (None, None, None),
        };
        let kind = match &verdict.kind {
            VerdictKind::Converted { .. } => StatFactKind::Converted,
            VerdictKind::Remuxed { .. } => StatFactKind::Remuxed,
            VerdictKind::NotWorthwhile { requested, floor } => StatFactKind::NotWorthwhile {
                requested: *requested,
                floor: *floor,
            },
        };
        let is_converted = kind == StatFactKind::Converted;
        facts.push(StatFact {
            content_key: content_key.clone(),
            kind,
            source_run: verdict.source_run,
            finished_at: run
                .and_then(|run| run.finished_at)
                .unwrap_or(verdict.decided_at),
            codec: record.metadata.codec.clone(),
            width: record.metadata.width,
            height: record.metadata.height,
            duration_ms: record.metadata.duration_ms,
            input_size_bytes,
            output_size_bytes,
            analyzing_ms,
            encoding_ms: if run.is_none() {
                carried_encoding.map_or(encoding_ms, |time| time.0)
            } else {
                encoding_ms
            },
            vmaf: measurement
                .filter(|_| is_converted)
                .map(|measurement| measurement.score)
                .or(carried_vmaf),
            crf: measurement
                .filter(|_| is_converted)
                .map(|measurement| measurement.crf)
                .or(carried_crf),
        });
    }
    facts
}

/// Input/output sizes for the run backing a verdict. Live evidence is the
/// authority; a crash-recovered success carries no sizes, so the settled
/// output transaction supplies them; failing that, the verdict-carried
/// summary covers adopted verdicts with no backing run, and the record's
/// inspected size covers the input while the output stays unknown.
fn joined_sizes(
    verdict: &crate::Verdict,
    run: Option<&ConversionRun>,
    state: &DurableState,
    record: &FileRecord,
) -> (Option<u64>, Option<u64>) {
    if let Some(run) = run
        && let Some(ItemOutcome::Converted(evidence) | ItemOutcome::Remuxed(evidence)) =
            &run.outcome
    {
        match evidence {
            CompletionEvidence::LiveEncode {
                input_size,
                output_size,
                ..
            }
            | CompletionEvidence::LiveRemux {
                input_size,
                output_size,
            } => return (Some(*input_size), Some(*output_size)),
            CompletionEvidence::RecoveredAtStartup => {}
        }
    }
    if let Some(run_id) = verdict.source_run
        && let Some(transaction) = state.outputs.get(&run_id)
    {
        let output = match &transaction.state {
            OutputState::Committed { final_identity }
            | OutputState::RetireIntent { final_identity }
            | OutputState::Retired { final_identity } => Some(final_identity.destructive.size),
            _ => None,
        };
        if output.is_some() {
            return (Some(transaction.input_identity.size), output);
        }
    }
    let (carried_input, carried_output) = match &verdict.kind {
        VerdictKind::Converted {
            input_size,
            output_size,
            ..
        }
        | VerdictKind::Remuxed {
            input_size,
            output_size,
            ..
        } => (*input_size, *output_size),
        VerdictKind::NotWorthwhile { .. } => (None, None),
    };
    (
        carried_input.or(Some(record.metadata.size_bytes)),
        carried_output,
    )
}

pub(crate) fn phase_totals(run: Option<&ConversionRun>) -> (u64, u64) {
    let mut analyzing_ms = 0u64;
    let mut encoding_ms = 0u64;
    let Some(run) = run else {
        return (analyzing_ms, encoding_ms);
    };
    for span in &run.phase_spans {
        match span.phase {
            JobPhase::Analyzing => analyzing_ms = analyzing_ms.saturating_add(span.duration.0),
            JobPhase::Encoding => encoding_ms = encoding_ms.saturating_add(span.duration.0),
            JobPhase::Preparing
            | JobPhase::Remuxing
            | JobPhase::Verifying
            | JobPhase::Finalizing => {}
        }
    }
    (analyzing_ms, encoding_ms)
}

/// Average and range of one aggregated value; absent when no samples exist.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct ValueSpread {
    pub average: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct CodecCount {
    pub codec: VideoCodec,
    pub count: u32,
}

/// One point of the cumulative savings series: a local calendar day (days
/// since the Unix epoch in the requester's timezone) and the running total
/// through that day. The series can dip when an output grew.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct CumulativeSavingsPoint {
    #[specta(type = crate::JsNumber)]
    pub epoch_day: i64,
    #[specta(type = crate::JsNumber)]
    pub cumulative_saved_bytes: i64,
}

/// Terminal run outcomes counted across every conversion run, independent of
/// the per-content current verdicts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub struct RunTotals {
    pub analyzed: u32,
    pub converted: u32,
    pub remuxed: u32,
    pub not_worthwhile: u32,
    pub stopped: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// The exhaustive Statistics answer. Conversion savings, VMAF, CRF, and time
/// aggregates cover converted verdicts only; remux facts are counted and
/// summed separately and never blend into conversion aggregates.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct StatisticsPayload {
    /// The requester-supplied offset the calendar bucketing used.
    pub utc_offset_minutes: i32,
    pub converted_files: u32,
    /// Converted facts that carried both sizes and therefore contribute to
    /// savings totals, bins, and the cumulative series.
    pub sized_converted_files: u32,
    pub remuxed_files: u32,
    pub not_worthwhile_files: u32,
    #[specta(type = crate::JsNumber)]
    pub total_input_bytes: u64,
    #[specta(type = crate::JsNumber)]
    pub total_output_bytes: u64,
    /// Negative when outputs grew past their inputs overall.
    #[specta(type = crate::JsNumber)]
    pub total_saved_bytes: i64,
    #[specta(type = crate::JsNumber)]
    pub remux_saved_bytes: i64,
    /// Analyzing plus encoding time across converted facts.
    #[specta(type = crate::JsNumber)]
    pub total_time_ms: u64,
    /// Input gigabytes processed per hour of conversion time.
    pub gigabytes_per_hour: Option<f64>,
    pub reduction_percent: Option<ValueSpread>,
    pub vmaf: Option<ValueSpread>,
    pub crf: Option<ValueSpread>,
    /// Ten 10%-wide bins over `[0, 100)`; a reduction of exactly 100% (an
    /// empty output) lands in the last bin.
    pub reduction_bins: Vec<u32>,
    /// Converted facts whose output grew — represented here, never clamped
    /// into the first bin.
    pub grew_count: u32,
    /// Source codecs of converted facts, most frequent first; ties use the
    /// codec enum's ascending canonical order.
    pub codecs: Vec<CodecCount>,
    pub cumulative_savings: Vec<CumulativeSavingsPoint>,
    #[specta(type = Option<crate::JsNumber>)]
    pub first_epoch_day: Option<i64>,
    #[specta(type = Option<crate::JsNumber>)]
    pub last_epoch_day: Option<i64>,
    pub runs: RunTotals,
}

/// Compute the full Statistics answer for the requester's timezone offset.
#[must_use]
pub fn statistics(state: &DurableState, utc_offset_minutes: i32) -> StatisticsPayload {
    let facts = collect_stat_facts(state);
    let mut accumulator = StatisticsAccumulator::new(utc_offset_minutes);
    for fact in &facts {
        let imported = state
            .records
            .get(&fact.content_key)
            .and_then(|record| record.imported.as_ref());
        if fact.source_run.is_none()
            && let Some(imported) = imported
            && matches!(
                imported.record.status,
                ParkedStatus::Converted | ParkedStatus::NotWorthwhile
            )
        {
            accumulator.add_imported(&imported.record);
            continue;
        }
        accumulator.add_native(fact);
    }
    for imported in state.parked.values() {
        accumulator.add_imported(imported);
    }
    let mut runs = RunTotals::default();
    for run in state.conversion_runs.values() {
        match &run.outcome {
            Some(ItemOutcome::Analyzed) => runs.analyzed = runs.analyzed.saturating_add(1),
            Some(ItemOutcome::Converted(_)) => runs.converted = runs.converted.saturating_add(1),
            Some(ItemOutcome::Remuxed(_)) => runs.remuxed = runs.remuxed.saturating_add(1),
            Some(ItemOutcome::NotWorthwhile { .. }) => {
                runs.not_worthwhile = runs.not_worthwhile.saturating_add(1);
            }
            Some(ItemOutcome::Stopped) => runs.stopped = runs.stopped.saturating_add(1),
            Some(ItemOutcome::Skipped { .. }) => runs.skipped = runs.skipped.saturating_add(1),
            Some(ItemOutcome::Failed(_)) => runs.failed = runs.failed.saturating_add(1),
            None => {}
        }
    }

    accumulator.finish(runs)
}

struct StatisticsAccumulator {
    utc_offset_minutes: i32,
    converted_files: u32,
    sized_converted_files: u32,
    remuxed_files: u32,
    not_worthwhile_files: u32,
    total_input: u128,
    total_output: u128,
    remux_saved: i128,
    total_time_ms: u64,
    reductions: Vec<f64>,
    vmaf_values: Vec<f64>,
    crf_values: Vec<f64>,
    reduction_bins: Vec<u32>,
    grew_count: u32,
    codecs: BTreeMap<VideoCodec, u32>,
    daily_savings: BTreeMap<i64, i128>,
    first_epoch_day: Option<i64>,
    last_epoch_day: Option<i64>,
}

impl StatisticsAccumulator {
    fn new(utc_offset_minutes: i32) -> Self {
        Self {
            utc_offset_minutes,
            converted_files: 0,
            sized_converted_files: 0,
            remuxed_files: 0,
            not_worthwhile_files: 0,
            total_input: 0,
            total_output: 0,
            remux_saved: 0,
            total_time_ms: 0,
            reductions: Vec::new(),
            vmaf_values: Vec::new(),
            crf_values: Vec::new(),
            reduction_bins: vec![0; REDUCTION_BIN_COUNT],
            grew_count: 0,
            codecs: BTreeMap::new(),
            daily_savings: BTreeMap::new(),
            first_epoch_day: None,
            last_epoch_day: None,
        }
    }

    fn add_native(&mut self, fact: &StatFact) {
        match fact.kind {
            StatFactKind::Converted => self.add_converted(
                fact.finished_at,
                Some(&fact.codec),
                fact.input_size_bytes,
                fact.output_size_bytes,
                fact.analyzing_ms.saturating_add(fact.encoding_ms),
                fact.vmaf,
                fact.crf,
            ),
            StatFactKind::Remuxed => {
                self.remuxed_files = self.remuxed_files.saturating_add(1);
                if let Some(saved) = fact.saved_bytes() {
                    self.remux_saved = self.remux_saved.saturating_add(saved);
                }
            }
            StatFactKind::NotWorthwhile { .. } => {
                self.not_worthwhile_files = self.not_worthwhile_files.saturating_add(1);
            }
        }
    }

    fn add_imported(&mut self, imported: &ImportedHistoryRecord) {
        match imported.status {
            ParkedStatus::Converted => self.add_converted(
                imported.decided_at,
                imported.video_codec.as_ref(),
                imported.size,
                imported.output_size,
                imported.encoding_time.map_or(0, |duration| duration.0),
                imported.vmaf,
                imported.crf,
            ),
            ParkedStatus::NotWorthwhile => {
                self.not_worthwhile_files = self.not_worthwhile_files.saturating_add(1);
            }
            ParkedStatus::Scanned | ParkedStatus::Analyzed => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_converted(
        &mut self,
        finished_at: UnixMillis,
        codec: Option<&VideoCodec>,
        input: Option<u64>,
        output: Option<u64>,
        time_ms: u64,
        vmaf: Option<VmafScore>,
        crf: Option<Crf>,
    ) {
        self.converted_files = self.converted_files.saturating_add(1);
        self.total_time_ms = self.total_time_ms.saturating_add(time_ms);
        if let Some(codec) = codec {
            let count = self.codecs.entry(codec.clone()).or_default();
            *count = count.saturating_add(1);
        }
        if let Some(score) = vmaf {
            self.vmaf_values
                .push(f64::from(score.0) / f64::from(crate::VMAF_SCORE_FIXED_SCALE));
        }
        if let Some(crf) = crf {
            self.crf_values
                .push(f64::from(crf.0) / f64::from(crate::CRF_FIXED_SCALE));
        }

        let day = local_epoch_day(finished_at, self.utc_offset_minutes);
        self.first_epoch_day = Some(self.first_epoch_day.map_or(day, |first| first.min(day)));
        self.last_epoch_day = Some(self.last_epoch_day.map_or(day, |last| last.max(day)));

        let (Some(input), Some(output)) = (input, output) else {
            return;
        };
        self.sized_converted_files = self.sized_converted_files.saturating_add(1);
        self.total_input = self.total_input.saturating_add(u128::from(input));
        self.total_output = self.total_output.saturating_add(u128::from(output));
        let saved = i128::from(input) - i128::from(output);
        let entry = self.daily_savings.entry(day).or_default();
        *entry = entry.saturating_add(saved);

        if input == 0 {
            return;
        }
        let percent = 100.0 * saved as f64 / input as f64;
        self.reductions.push(percent);
        if percent < 0.0 {
            self.grew_count = self.grew_count.saturating_add(1);
        } else {
            let bin =
                ((percent / REDUCTION_BIN_WIDTH_PERCENT) as usize).min(REDUCTION_BIN_COUNT - 1);
            if let Some(slot) = self.reduction_bins.get_mut(bin) {
                *slot = slot.saturating_add(1);
            }
        }
    }

    fn finish(self, runs: RunTotals) -> StatisticsPayload {
        let total_saved = i128::try_from(self.total_input).unwrap_or(i128::MAX)
            - i128::try_from(self.total_output).unwrap_or(i128::MAX);
        let mut running = 0i128;
        let cumulative_savings = self
            .daily_savings
            .iter()
            .map(|(day, saved)| {
                running = running.saturating_add(*saved);
                CumulativeSavingsPoint {
                    epoch_day: *day,
                    cumulative_saved_bytes: clamp_to_i64(running),
                }
            })
            .collect();
        let gigabytes_per_hour = if self.total_time_ms > 0 && self.total_input > 0 {
            let gib = u64::try_from(self.total_input).unwrap_or(u64::MAX) as f64 / BYTES_PER_GIB;
            let hours = self.total_time_ms as f64 / MILLIS_PER_HOUR;
            Some(gib / hours)
        } else {
            None
        };
        let mut codecs: Vec<CodecCount> = self
            .codecs
            .into_iter()
            .map(|(codec, count)| CodecCount { codec, count })
            .collect();
        codecs.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.codec.cmp(&right.codec))
        });

        StatisticsPayload {
            utc_offset_minutes: self.utc_offset_minutes,
            converted_files: self.converted_files,
            sized_converted_files: self.sized_converted_files,
            remuxed_files: self.remuxed_files,
            not_worthwhile_files: self.not_worthwhile_files,
            total_input_bytes: u64::try_from(self.total_input).unwrap_or(u64::MAX),
            total_output_bytes: u64::try_from(self.total_output).unwrap_or(u64::MAX),
            total_saved_bytes: clamp_to_i64(total_saved),
            remux_saved_bytes: clamp_to_i64(self.remux_saved),
            total_time_ms: self.total_time_ms,
            gigabytes_per_hour,
            reduction_percent: spread(&self.reductions),
            vmaf: spread(&self.vmaf_values),
            crf: spread(&self.crf_values),
            reduction_bins: self.reduction_bins,
            grew_count: self.grew_count,
            codecs,
            cumulative_savings,
            first_epoch_day: self.first_epoch_day,
            last_epoch_day: self.last_epoch_day,
            runs,
        }
    }
}

fn spread(values: &[f64]) -> Option<ValueSpread> {
    let first = *values.first()?;
    let mut minimum = first;
    let mut maximum = first;
    let mut sum = 0.0;
    for &value in values {
        minimum = minimum.min(value);
        maximum = maximum.max(value);
        sum += value;
    }
    Some(ValueSpread {
        average: sum / values.len() as f64,
        minimum,
        maximum,
        count: u32::try_from(values.len()).unwrap_or(u32::MAX),
    })
}

fn clamp_to_i64(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

/// Calendar day (days since the Unix epoch) of an instant in the timezone
/// described by `utc_offset_minutes`.
#[must_use]
pub fn local_epoch_day(at: UnixMillis, utc_offset_minutes: i32) -> i64 {
    let local_ms = i128::from(at.0) + i128::from(utc_offset_minutes) * MILLIS_PER_MINUTE;
    let day = local_ms.div_euclid(MILLIS_PER_DAY);
    i64::try_from(day).unwrap_or_default()
}

/// The current standing of one content in History terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum HistoryStatus {
    Converted,
    Remuxed,
    NotWorthwhile {
        requested: VmafTarget,
        floor: VmafTarget,
    },
    Analyzed,
    Failed {
        kind: FailureKind,
        message: String,
    },
    Stopped,
}

/// Stable identity for either an observed content row or an unresolved
/// imported path row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", content = "value")]
pub enum HistoryRowKey {
    Content(ContentKey),
    Parked(crate::ImportPath),
}

/// One History row per native/adopted content or reportable parked import.
/// Facts only — units, prefixes, and labels are presentation. Native width
/// and height are post-rotation. Bitrate is derived in views from size and
/// duration, matching the [`crate::VideoMeta`] contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct HistoryRow {
    pub key: HistoryRowKey,
    pub status: HistoryStatus,
    pub source_run: Option<RunId>,
    pub happened_at: Option<UnixMillis>,
    pub codec: Option<VideoCodec>,
    pub container: Option<MediaContainer>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[specta(type = Option<crate::JsNumber>)]
    pub duration_ms: Option<u64>,
    /// `None` means the source did not record audio metadata; `Some([])`
    /// means probing established that the file has no audio streams.
    pub audio: Option<Vec<AudioCodec>>,
    #[specta(type = Option<crate::JsNumber>)]
    pub input_size_bytes: Option<u64>,
    #[specta(type = Option<crate::JsNumber>)]
    pub output_size_bytes: Option<u64>,
    #[specta(type = Option<crate::JsNumber>)]
    pub encoding_time_ms: Option<u64>,
    pub vmaf: Option<VmafScore>,
    pub crf: Option<Crf>,
}

/// Project one row per content worth reporting, in content-key order.
/// Filtering and sorting are frontend concerns.
///
/// Status derivation: a standing verdict wins — it is the record's judgment
/// about the content (see [`crate::Verdict`]). Without one, the latest
/// failed or stopped run for the content reports with its reason; without
/// that, completed analyses report as `Analyzed`. Content that was only
/// scanned has nothing to report and gets no row. Skipped runs decide
/// nothing and never surface here — their reasons live on queue items.
#[must_use]
pub fn history_rows(state: &DurableState) -> Vec<HistoryRow> {
    let mut latest_analysis: BTreeMap<&ContentKey, (RunId, &AnalysisResult)> = BTreeMap::new();
    let mut latest_interruption: BTreeMap<&ContentKey, (RunId, &ConversionRun)> = BTreeMap::new();
    for (run_id, run) in &state.conversion_runs {
        let Some(content_key) = &run.spec.content_key else {
            continue;
        };
        if let Some(analysis) = &run.analysis {
            latest_analysis.insert(content_key, (*run_id, analysis));
        }
        if matches!(
            run.outcome,
            Some(ItemOutcome::Failed(_) | ItemOutcome::Stopped)
        ) {
            latest_interruption.insert(content_key, (*run_id, run));
        }
    }

    let mut rows = Vec::new();
    for (content_key, record) in &state.records {
        let row = if let Some(verdict) = &record.verdict {
            if verdict.source_run.is_none()
                && let Some(imported) = &record.imported
                && let Some(status) = imported_status(&imported.record)
            {
                imported_row(
                    HistoryRowKey::Content(content_key.clone()),
                    &imported.record,
                    status,
                )
            } else {
                verdict_row(content_key, record, verdict, state)
            }
        } else if let Some((run_id, run)) = latest_interruption.get(content_key) {
            interruption_row(content_key, record, *run_id, run)
        } else if !record.analyses.is_empty() {
            analyzed_row(
                content_key,
                record,
                latest_analysis.get(content_key).copied(),
            )
        } else if let Some(imported) = &record.imported
            && let Some(status) = imported_status(&imported.record)
        {
            imported_row(
                HistoryRowKey::Content(content_key.clone()),
                &imported.record,
                status,
            )
        } else {
            continue;
        };
        rows.push(row);
    }
    for (import_path, imported) in &state.parked {
        if let Some(status) = imported_status(imported) {
            rows.push(imported_row(
                HistoryRowKey::Parked(import_path.clone()),
                imported,
                status,
            ));
        }
    }
    rows
}

fn base_row(content_key: &ContentKey, record: &FileRecord, status: HistoryStatus) -> HistoryRow {
    let (width, height) = record.metadata.post_rotation_dimensions();
    HistoryRow {
        key: HistoryRowKey::Content(content_key.clone()),
        status,
        source_run: None,
        happened_at: None,
        codec: Some(record.metadata.codec.clone()),
        container: Some(record.metadata.container.clone()),
        width: Some(width),
        height: Some(height),
        duration_ms: Some(record.metadata.duration_ms),
        audio: Some(
            record
                .metadata
                .audio
                .iter()
                .map(|stream| stream.codec.clone())
                .collect(),
        ),
        input_size_bytes: Some(record.metadata.size_bytes),
        output_size_bytes: None,
        encoding_time_ms: None,
        vmaf: None,
        crf: None,
    }
}

fn verdict_row(
    content_key: &ContentKey,
    record: &FileRecord,
    verdict: &crate::Verdict,
    state: &DurableState,
) -> HistoryRow {
    let status = match &verdict.kind {
        VerdictKind::Converted { .. } => HistoryStatus::Converted,
        VerdictKind::Remuxed { .. } => HistoryStatus::Remuxed,
        VerdictKind::NotWorthwhile { requested, floor } => HistoryStatus::NotWorthwhile {
            requested: *requested,
            floor: *floor,
        },
    };
    let run = verdict
        .source_run
        .and_then(|run_id| state.conversion_runs.get(&run_id));
    let (input_size, output_size) = joined_sizes(verdict, run, state, record);
    let measurement = run
        .and_then(|run| run.analysis.as_ref())
        .map(|analysis| &analysis.measurement)
        .filter(|_| status == HistoryStatus::Converted);
    let (carried_time, carried_crf, carried_vmaf) = match &verdict.kind {
        VerdictKind::Converted {
            encoding_time,
            crf,
            vmaf,
            ..
        } => (*encoding_time, *crf, *vmaf),
        VerdictKind::Remuxed { .. } | VerdictKind::NotWorthwhile { .. } => (None, None, None),
    };
    let mut row = base_row(content_key, record, status);
    row.source_run = verdict.source_run;
    row.happened_at = Some(
        run.and_then(|run| run.finished_at)
            .unwrap_or(verdict.decided_at),
    );
    row.input_size_bytes = input_size;
    row.output_size_bytes = output_size;
    row.encoding_time_ms = run
        .map(|run| phase_totals(Some(run)).1)
        .or(carried_time.map(|duration| duration.0));
    row.vmaf = measurement
        .map(|measurement| measurement.score)
        .or(carried_vmaf);
    row.crf = measurement
        .map(|measurement| measurement.crf)
        .or(carried_crf);
    row
}

fn interruption_row(
    content_key: &ContentKey,
    record: &FileRecord,
    run_id: RunId,
    run: &ConversionRun,
) -> HistoryRow {
    let status = match &run.outcome {
        Some(ItemOutcome::Failed(facts)) => HistoryStatus::Failed {
            kind: facts.kind,
            message: facts.message.clone(),
        },
        _ => HistoryStatus::Stopped,
    };
    let mut row = base_row(content_key, record, status);
    row.source_run = Some(run_id);
    row.happened_at = run.finished_at;
    row
}

fn analyzed_row(
    content_key: &ContentKey,
    record: &FileRecord,
    latest: Option<(RunId, &AnalysisResult)>,
) -> HistoryRow {
    let mut row = base_row(content_key, record, HistoryStatus::Analyzed);
    // Prefer the analysis attached to the latest run; a record can also carry
    // analyses with no surviving run (future legacy adoption), in which case
    // the deterministic last index entry — highest profile, highest target —
    // stands in.
    let analysis = latest.map(|(_, analysis)| analysis).or_else(|| {
        record
            .analyses
            .iter()
            .next_back()
            .and_then(|(_, by_target)| by_target.values().next_back())
    });
    row.source_run = latest.map(|(run_id, _)| run_id);
    row.vmaf = analysis.map(|analysis| analysis.measurement.score);
    row.crf = analysis.map(|analysis| analysis.measurement.crf);
    row
}

fn imported_status(imported: &ImportedHistoryRecord) -> Option<HistoryStatus> {
    match imported.status {
        ParkedStatus::Converted => Some(HistoryStatus::Converted),
        ParkedStatus::NotWorthwhile => Some(HistoryStatus::NotWorthwhile {
            requested: imported
                .requested_target
                .unwrap_or(crate::DEFAULT_VMAF_TARGET),
            floor: imported
                .floor_target
                .unwrap_or(crate::MIN_VMAF_FALLBACK_TARGET),
        }),
        ParkedStatus::Analyzed => Some(HistoryStatus::Analyzed),
        ParkedStatus::Scanned => None,
    }
}

fn imported_row(
    key: HistoryRowKey,
    imported: &ImportedHistoryRecord,
    status: HistoryStatus,
) -> HistoryRow {
    HistoryRow {
        key,
        status,
        source_run: None,
        happened_at: Some(imported.decided_at),
        codec: imported.video_codec.clone(),
        container: None,
        width: imported.width,
        height: imported.height,
        duration_ms: imported.duration_ms,
        audio: None,
        input_size_bytes: imported.size,
        output_size_bytes: imported.output_size,
        encoding_time_ms: imported.encoding_time.map(|duration| duration.0),
        vmaf: imported.vmaf,
        crf: imported.crf,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;

    use super::*;
    use crate::{
        AnalysisProfile, AnalysisResult, ArtifactIdentity, AudioStreamMeta, ClaimId,
        CompletionEvidence, ContentKey, ConversionRun, Crf, DestructiveIdentity, DurableDelta,
        DurableState, DurationMs, ExecutionSettings, FailureFacts, FileRecord, FileSystemId,
        FileTimeNs, ImportPath, ImportedHistoryRecord, ItemOutcome, JobAction, JobPhase, JobSpec,
        MediaContainer, Operation, OutputState, OutputTarget, OutputTransaction, ParkedStatus,
        PhaseSpan, QueueItemId, Replacement, RunId, SearchMeasurement, UnixMillis, Verdict,
        VerdictKind, VideoCodec, VideoMeta, VmafScore, VmafTarget,
    };

    const DAY_MS: u64 = 86_400_000;

    fn key(name: &str) -> ContentKey {
        ContentKey(name.to_owned())
    }

    fn meta(codec: VideoCodec, size_bytes: u64) -> VideoMeta {
        VideoMeta {
            codec,
            container: MediaContainer::Matroska,
            width: 1920,
            height: 1080,
            rotation_degrees: 0,
            duration_ms: 600_000,
            size_bytes,
            audio: vec![AudioStreamMeta {
                codec: crate::AudioCodec::Aac,
                channels: 2,
            }],
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

    fn analysis(crf_milli: u32, score_centi: u16) -> AnalysisResult {
        AnalysisResult {
            requested_target: VmafTarget(95),
            successful_target: VmafTarget(95),
            fallback_floor: VmafTarget(90),
            fallback_step: 1,
            failed_attempts: Vec::new(),
            measurement: SearchMeasurement {
                crf: Crf(crf_milli),
                score: VmafScore(score_centi),
                predicted_size: 1_000,
                predicted_percent_basis_points: 5_000,
                predicted_duration_ms: 60_000,
                from_cache: false,
            },
            profile: AnalysisProfile::production(),
        }
    }

    fn finished_run(
        run: u64,
        content_key: &ContentKey,
        outcome: ItemOutcome,
        finished_at: UnixMillis,
    ) -> ConversionRun {
        ConversionRun {
            spec: spec(run, content_key, Operation::Convert),
            analysis: Some(analysis(24_000, 9_512)),
            output_content_key: None,
            outcome: Some(outcome),
            started_at: Some(UnixMillis(finished_at.0.saturating_sub(1_000))),
            finished_at: Some(finished_at),
            phase_spans: vec![
                PhaseSpan {
                    phase: JobPhase::Analyzing,
                    duration: DurationMs(60_000),
                },
                PhaseSpan {
                    phase: JobPhase::Encoding,
                    duration: DurationMs(240_000),
                },
            ],
        }
    }

    fn converted(evidence: CompletionEvidence) -> ItemOutcome {
        ItemOutcome::Converted(evidence)
    }

    fn live(input_size: u64, output_size: u64) -> CompletionEvidence {
        CompletionEvidence::LiveEncode {
            input_size,
            output_size,
            stream_sizes: crate::StreamByteSizes {
                video: output_size,
                audio: 0,
                subtitle: 0,
                other: 0,
            },
            encode_decode: crate::DecodeMode::Software,
        }
    }

    fn imported(status: ParkedStatus, codec: Option<VideoCodec>) -> ImportedHistoryRecord {
        ImportedHistoryRecord {
            status,
            size: Some(10_000),
            modified_ns: Some(FileTimeNs(1)),
            video_codec: codec,
            width: Some(1_280),
            height: Some(720),
            duration_ms: Some(60_000),
            output_size: Some(4_000),
            encoding_time: Some(DurationMs(120_000)),
            crf: Some(Crf(30_000)),
            vmaf: Some(VmafScore(9_512)),
            target: Some(VmafTarget(95)),
            requested_target: Some(VmafTarget(94)),
            floor_target: Some(VmafTarget(91)),
            decided_at: UnixMillis(DAY_MS * 20_000),
        }
    }

    fn adopted_verdict(imported: &ImportedHistoryRecord) -> Verdict {
        Verdict {
            kind: VerdictKind::Converted {
                output_content_key: None,
                input_size: imported.size,
                output_size: imported.output_size,
                encoding_time: imported.encoding_time,
                crf: imported.crf,
                vmaf: imported.vmaf,
                target: imported.target,
            },
            source_run: None,
            decided_at: imported.decided_at,
        }
    }

    /// A state with one converted content per (input, output) pair, finished
    /// one day apart starting at `first_day_ms`.
    fn converted_state(sizes: &[(u64, u64)], first_day_ms: u64) -> DurableState {
        let mut state = DurableState::default();
        for (index, (input, output)) in sizes.iter().enumerate() {
            let run = index as u64 + 1;
            let content_key = key(&format!("content-{run:04}"));
            let finished = UnixMillis(first_day_ms + index as u64 * DAY_MS);
            let mut record = FileRecord::new(meta(VideoCodec::Hevc, *input));
            record.verdict = Some(Verdict {
                kind: VerdictKind::Converted {
                    output_content_key: Some(key(&format!("output-{run:04}"))),
                    input_size: None,
                    output_size: None,
                    encoding_time: None,
                    crf: None,
                    vmaf: None,
                    target: None,
                },
                source_run: Some(RunId(run)),
                decided_at: finished,
            });
            state.records.insert(content_key.clone(), record);
            state.conversion_runs.insert(
                RunId(run),
                finished_run(
                    run,
                    &content_key,
                    converted(live(*input, *output)),
                    finished,
                ),
            );
        }
        state
    }

    fn identity(size: u64) -> DestructiveIdentity {
        DestructiveIdentity {
            file_id: FileSystemId::Unix {
                device: 1,
                inode: size,
            },
            size,
            modified_ns: None,
        }
    }

    #[test]
    fn empty_state_projects_to_zeroes_and_no_rows() {
        let state = DurableState::default();
        let payload = statistics(&state, 0);
        assert_eq!(payload.converted_files, 0);
        assert_eq!(payload.total_saved_bytes, 0);
        assert_eq!(payload.reduction_percent, None);
        assert_eq!(payload.vmaf, None);
        assert_eq!(payload.gigabytes_per_hour, None);
        assert_eq!(payload.reduction_bins, vec![0; 10]);
        assert!(payload.cumulative_savings.is_empty());
        assert_eq!(payload.first_epoch_day, None);
        assert!(history_rows(&state).is_empty());
    }

    #[test]
    fn parked_imports_project_complete_sparse_history_and_statistics() {
        let mut state = DurableState::default();
        state.parked.insert(
            ImportPath("c:/history/analyzed.mkv".to_owned()),
            imported(ParkedStatus::Analyzed, Some(VideoCodec::Hevc)),
        );
        state.parked.insert(
            ImportPath("c:/history/converted.mkv".to_owned()),
            imported(ParkedStatus::Converted, Some(VideoCodec::H264)),
        );
        state.parked.insert(
            ImportPath("c:/history/declined.mkv".to_owned()),
            imported(ParkedStatus::NotWorthwhile, Some(VideoCodec::Vp9)),
        );
        state.parked.insert(
            ImportPath("c:/history/scanned.mkv".to_owned()),
            imported(ParkedStatus::Scanned, None),
        );

        let payload = statistics(&state, 0);
        assert_eq!(payload.converted_files, 1);
        assert_eq!(payload.sized_converted_files, 1);
        assert_eq!(payload.not_worthwhile_files, 1);
        assert_eq!(payload.total_saved_bytes, 6_000);
        assert_eq!(payload.total_time_ms, 120_000);
        assert_eq!(payload.runs, RunTotals::default());

        let rows = history_rows(&state);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.source_run.is_none()));
        let analyzed = rows
            .iter()
            .find(|row| row.status == HistoryStatus::Analyzed)
            .expect("parked analyzed row");
        assert!(matches!(analyzed.key, HistoryRowKey::Parked(_)));
        assert_eq!(analyzed.container, None);
        assert_eq!(analyzed.audio, None);
    }

    #[test]
    fn one_to_one_adoption_preserves_statistics_and_imported_analysis_is_display_only() {
        let import_path = ImportPath("c:/history/movie.mkv".to_owned());
        let converted_import = imported(ParkedStatus::Converted, Some(VideoCodec::H264));
        let mut state = DurableState::default();
        state
            .parked
            .insert(import_path.clone(), converted_import.clone());
        let before = statistics(&state, 0);

        let content_key = key("movie-content");
        state.records.insert(
            content_key.clone(),
            FileRecord::new(meta(VideoCodec::H264, 10_000)),
        );
        crate::fold(
            &mut state,
            &DurableDelta::ParkedAdopted {
                import_path: import_path.clone(),
                content_key: content_key.clone(),
                imported: converted_import.clone(),
                verdict: Some(adopted_verdict(&converted_import)),
            },
        );

        assert_eq!(statistics(&state, 0), before);
        assert_eq!(state.adopted_imports.len(), 1);
        assert!(state.parked.is_empty());
        assert_eq!(
            history_rows(&state)[0].key,
            HistoryRowKey::Content(content_key)
        );

        let analyzed_path = ImportPath("c:/history/analyzed-only.mkv".to_owned());
        let analyzed = imported(ParkedStatus::Analyzed, Some(VideoCodec::Hevc));
        state.parked.insert(analyzed_path.clone(), analyzed.clone());
        crate::fold(
            &mut state,
            &DurableDelta::ParkedAdopted {
                import_path: analyzed_path,
                content_key: key("movie-content"),
                imported: analyzed,
                verdict: None,
            },
        );
        assert!(state.records[&key("movie-content")].analyses.is_empty());
    }

    #[test]
    fn many_import_paths_collapsing_to_one_content_count_once_after_adoption() {
        let path_a = ImportPath("c:/history/a.mkv".to_owned());
        let path_b = ImportPath("c:/history/b.mkv".to_owned());
        let mut older = imported(ParkedStatus::Converted, Some(VideoCodec::H264));
        older.decided_at = UnixMillis(DAY_MS * 19_999);
        let newer = imported(ParkedStatus::Converted, Some(VideoCodec::Hevc));
        let mut state = DurableState::default();
        state.parked.insert(path_a.clone(), older.clone());
        state.parked.insert(path_b.clone(), newer.clone());
        assert_eq!(statistics(&state, 0).converted_files, 2);

        let content_key = key("shared-content");
        state.records.insert(
            content_key.clone(),
            FileRecord::new(meta(VideoCodec::Av1, 10_000)),
        );
        for (path, record) in [(path_a, older), (path_b.clone(), newer.clone())] {
            crate::fold(
                &mut state,
                &DurableDelta::ParkedAdopted {
                    import_path: path,
                    content_key: content_key.clone(),
                    imported: record.clone(),
                    verdict: Some(adopted_verdict(&record)),
                },
            );
        }

        let payload = statistics(&state, 0);
        assert_eq!(payload.converted_files, 1);
        assert_eq!(payload.runs, RunTotals::default());
        assert_eq!(state.adopted_imports.len(), 2);
        assert_eq!(
            state.records[&content_key]
                .imported
                .as_ref()
                .map(|provenance| &provenance.import_path),
            Some(&path_b)
        );
    }

    #[test]
    fn codec_count_ties_use_canonical_ascending_order() {
        let mut state = DurableState::default();
        for (path, codec) in [
            ("vp9.mkv", VideoCodec::Vp9),
            ("h264.mkv", VideoCodec::H264),
            ("av1.mkv", VideoCodec::Av1),
        ] {
            state.parked.insert(
                ImportPath(path.to_owned()),
                imported(ParkedStatus::Converted, Some(codec)),
            );
        }
        assert_eq!(
            statistics(&state, 0)
                .codecs
                .into_iter()
                .map(|entry| entry.codec)
                .collect::<Vec<_>>(),
            vec![VideoCodec::Av1, VideoCodec::H264, VideoCodec::Vp9]
        );
    }

    #[test]
    fn single_conversion_aggregates_from_live_evidence() {
        let state = converted_state(&[(10_000_000_000, 4_000_000_000)], DAY_MS * 20_000);
        let payload = statistics(&state, 0);
        assert_eq!(payload.converted_files, 1);
        assert_eq!(payload.sized_converted_files, 1);
        assert_eq!(payload.total_input_bytes, 10_000_000_000);
        assert_eq!(payload.total_output_bytes, 4_000_000_000);
        assert_eq!(payload.total_saved_bytes, 6_000_000_000);
        assert_eq!(payload.total_time_ms, 300_000);
        // 60% reduction lands in the 60-70% bin.
        assert_eq!(payload.reduction_bins[6], 1);
        assert_eq!(payload.reduction_bins.iter().sum::<u32>(), 1);
        assert_eq!(payload.grew_count, 0);
        let vmaf = payload.vmaf.unwrap();
        assert_eq!(vmaf.average, 95.12);
        let crf = payload.crf.unwrap();
        assert_eq!(crf.average, 24.0);
        // 10 GB input in 300s: 9.3132 GiB / (1/12) h.
        let throughput = payload.gigabytes_per_hour.unwrap();
        assert!((throughput - 111.76).abs() < 0.01);
        assert_eq!(payload.cumulative_savings.len(), 1);
        assert_eq!(payload.first_epoch_day, Some(20_000));
        assert_eq!(payload.runs.converted, 1);
    }

    #[test]
    fn recovered_evidence_falls_back_to_the_settled_transaction() {
        let mut state = converted_state(&[(0, 0)], DAY_MS * 20_000);
        if let Some(run) = state.conversion_runs.get_mut(&RunId(1)) {
            run.outcome = Some(converted(CompletionEvidence::RecoveredAtStartup));
        }
        state.outputs.insert(
            RunId(1),
            OutputTransaction {
                run_id: RunId(1),
                input: PathBuf::from("input-1.mkv"),
                input_identity: identity(8_000),
                staging: PathBuf::from("staging"),
                final_path: PathBuf::from("final.mkv"),
                final_preimage: None,
                replacement: Replacement::KeepOriginal,
                state: OutputState::Committed {
                    final_identity: ArtifactIdentity {
                        content_key: key("output-0001"),
                        destructive: identity(3_000),
                    },
                },
            },
        );
        let facts = collect_stat_facts(&state);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].input_size_bytes, Some(8_000));
        assert_eq!(facts[0].output_size_bytes, Some(3_000));
    }

    #[test]
    fn verdict_without_run_or_transaction_keeps_input_from_metadata() {
        let mut state = DurableState::default();
        let content_key = key("adopted");
        let mut record = FileRecord::new(meta(VideoCodec::H264, 5_000));
        record.verdict = Some(Verdict {
            kind: VerdictKind::Converted {
                output_content_key: Some(key("adopted-out")),
                input_size: None,
                output_size: None,
                encoding_time: None,
                crf: None,
                vmaf: None,
                target: None,
            },
            source_run: Some(RunId(77)),
            decided_at: UnixMillis(DAY_MS * 19_000),
        });
        state.records.insert(content_key.clone(), record);

        let facts = collect_stat_facts(&state);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].input_size_bytes, Some(5_000));
        assert_eq!(facts[0].output_size_bytes, None);
        assert_eq!(facts[0].finished_at, UnixMillis(DAY_MS * 19_000));

        let payload = statistics(&state, 0);
        // Counted as converted, but never enters savings totals or bins.
        assert_eq!(payload.converted_files, 1);
        assert_eq!(payload.sized_converted_files, 0);
        assert_eq!(payload.total_saved_bytes, 0);
        assert_eq!(payload.reduction_bins.iter().sum::<u32>(), 0);

        let rows = history_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, HistoryStatus::Converted);
        assert_eq!(rows[0].input_size_bytes, Some(5_000));
        assert_eq!(rows[0].output_size_bytes, None);
    }

    #[test]
    fn remux_stays_out_of_conversion_aggregates() {
        let mut state = converted_state(&[(10_000, 5_000)], DAY_MS * 20_000);
        let content_key = key("remuxed");
        let mut record = FileRecord::new(meta(VideoCodec::Av1, 9_000));
        record.verdict = Some(Verdict {
            kind: VerdictKind::Remuxed {
                output_content_key: key("remuxed-out"),
                input_size: None,
                output_size: None,
            },
            source_run: Some(RunId(50)),
            decided_at: UnixMillis(DAY_MS * 20_000),
        });
        state.records.insert(content_key.clone(), record);
        let mut run = finished_run(
            50,
            &content_key,
            ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
                input_size: 9_000,
                output_size: 8_900,
            }),
            UnixMillis(DAY_MS * 20_000),
        );
        run.analysis = None;
        state.conversion_runs.insert(RunId(50), run);

        let payload = statistics(&state, 0);
        assert_eq!(payload.converted_files, 1);
        assert_eq!(payload.remuxed_files, 1);
        assert_eq!(payload.total_saved_bytes, 5_000);
        assert_eq!(payload.remux_saved_bytes, 100);
        assert_eq!(payload.vmaf.map(|spread| spread.count), Some(1));
        assert_eq!(payload.codecs.len(), 1);

        let rows = history_rows(&state);
        let remux_row = rows
            .iter()
            .find(|row| row.status == HistoryStatus::Remuxed)
            .unwrap();
        assert_eq!(remux_row.output_size_bytes, Some(8_900));
        assert_eq!(remux_row.vmaf, None);
    }

    #[test]
    fn grown_outputs_count_separately_and_dip_the_cumulative_series() {
        let state = converted_state(
            &[(10_000, 4_000), (5_000, 8_000)], // second one grew by 3000
            DAY_MS * 20_000,
        );
        let payload = statistics(&state, 0);
        assert_eq!(payload.grew_count, 1);
        assert_eq!(payload.reduction_bins.iter().sum::<u32>(), 1);
        assert_eq!(payload.total_saved_bytes, 3_000);
        assert_eq!(payload.cumulative_savings.len(), 2);
        assert_eq!(payload.cumulative_savings[0].cumulative_saved_bytes, 6_000);
        assert_eq!(payload.cumulative_savings[1].cumulative_saved_bytes, 3_000);
        let reduction = payload.reduction_percent.unwrap();
        assert_eq!(reduction.minimum, -60.0);
        assert_eq!(reduction.maximum, 60.0);
    }

    #[test]
    fn reduction_bin_boundaries_belong_to_the_upper_bin() {
        // Exactly 30% reduction: bin index 3, matching Python's floor rule.
        let state = converted_state(&[(10_000, 7_000)], DAY_MS * 20_000);
        let payload = statistics(&state, 0);
        assert_eq!(payload.reduction_bins[3], 1);
        // A 100% reduction clamps into the last bin instead of overflowing.
        let full = converted_state(&[(10_000, 0)], DAY_MS * 20_000);
        assert_eq!(statistics(&full, 0).reduction_bins[9], 1);
    }

    #[test]
    fn timezone_offset_shifts_the_day_bucket() {
        // 23:30 UTC on epoch day 20000.
        let at = UnixMillis(DAY_MS * 20_000 + 23 * 3_600_000 + 30 * 60_000);
        assert_eq!(local_epoch_day(at, 0), 20_000);
        assert_eq!(local_epoch_day(at, 60), 20_001); // UTC+1 is past midnight
        assert_eq!(local_epoch_day(UnixMillis(DAY_MS * 20_000), -60), 19_999);
    }

    #[test]
    fn not_worthwhile_counts_and_rows_carry_the_targets() {
        let mut state = DurableState::default();
        let content_key = key("declined");
        let mut record = FileRecord::new(meta(VideoCodec::H264, 5_000));
        record.verdict = Some(Verdict {
            kind: VerdictKind::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
            },
            source_run: Some(RunId(9)),
            decided_at: UnixMillis(DAY_MS * 20_000),
        });
        state.records.insert(content_key.clone(), record);
        state.conversion_runs.insert(
            RunId(9),
            finished_run(
                9,
                &content_key,
                ItemOutcome::NotWorthwhile {
                    attempts: Vec::new(),
                },
                UnixMillis(DAY_MS * 20_000),
            ),
        );

        let payload = statistics(&state, 0);
        assert_eq!(payload.not_worthwhile_files, 1);
        assert_eq!(payload.converted_files, 0);
        let rows = history_rows(&state);
        assert_eq!(
            rows[0].status,
            HistoryStatus::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
            }
        );
    }

    #[test]
    fn failure_reports_only_for_verdictless_content() {
        let mut state = converted_state(&[(10_000, 4_000)], DAY_MS * 20_000);
        let converted_key = key("content-0001");
        // A later failed run on already-converted content: the verdict is the
        // standing judgment, so the row stays Converted.
        let mut failed_retry = finished_run(
            2,
            &converted_key,
            ItemOutcome::Failed(FailureFacts::new(
                crate::FailureKind::EncodeRun,
                "encoder crashed",
            )),
            UnixMillis(DAY_MS * 20_001),
        );
        failed_retry.analysis = None;
        state.conversion_runs.insert(RunId(2), failed_retry);
        // A failed run on fresh content produces a Failed row.
        let fresh_key = key("fresh");
        state.records.insert(
            fresh_key.clone(),
            FileRecord::new(meta(VideoCodec::Vp9, 7_000)),
        );
        let mut failed_fresh = finished_run(
            3,
            &fresh_key,
            ItemOutcome::Failed(FailureFacts::new(
                crate::FailureKind::SearchRun,
                "probe failed",
            )),
            UnixMillis(DAY_MS * 20_001),
        );
        failed_fresh.analysis = None;
        state.conversion_runs.insert(RunId(3), failed_fresh);

        let rows = history_rows(&state);
        assert_eq!(rows.len(), 2);
        let converted_row = rows
            .iter()
            .find(|row| row.key == HistoryRowKey::Content(converted_key.clone()))
            .unwrap();
        assert_eq!(converted_row.status, HistoryStatus::Converted);
        let failed_row = rows
            .iter()
            .find(|row| row.key == HistoryRowKey::Content(fresh_key.clone()))
            .unwrap();
        assert_eq!(
            failed_row.status,
            HistoryStatus::Failed {
                kind: crate::FailureKind::SearchRun,
                message: "probe failed".to_owned(),
            }
        );
        assert_eq!(failed_row.source_run, Some(RunId(3)));
        assert_eq!(statistics(&state, 0).runs.failed, 2);
    }

    #[test]
    fn analyzed_content_reports_the_latest_run_measurement() {
        let mut state = DurableState::default();
        let content_key = key("studied");
        let mut record = FileRecord::new(meta(VideoCodec::Hevc, 5_000));
        record.record_analysis(analysis(30_000, 9_400));
        record.record_analysis(analysis(26_000, 9_600));
        state.records.insert(content_key.clone(), record);
        let mut run = finished_run(
            4,
            &content_key,
            ItemOutcome::Analyzed,
            UnixMillis(DAY_MS * 20_000),
        );
        run.analysis = Some(analysis(26_000, 9_600));
        state.conversion_runs.insert(RunId(4), run);

        let rows = history_rows(&state);
        assert_eq!(rows[0].status, HistoryStatus::Analyzed);
        assert_eq!(rows[0].crf, Some(Crf(26_000)));
        assert_eq!(rows[0].vmaf, Some(VmafScore(9_600)));
        assert_eq!(rows[0].source_run, Some(RunId(4)));
        // Analyses alone never create a StatFact.
        assert!(collect_stat_facts(&state).is_empty());
        assert_eq!(statistics(&state, 0).runs.analyzed, 1);
    }

    #[test]
    fn scanned_only_content_gets_no_row() {
        let mut state = DurableState::default();
        state
            .records
            .insert(key("seen"), FileRecord::new(meta(VideoCodec::H264, 1_000)));
        assert!(history_rows(&state).is_empty());
    }

    #[test]
    fn rotated_dimensions_present_post_rotation() {
        let mut state = DurableState::default();
        let content_key = key("portrait");
        let mut portrait = meta(VideoCodec::H264, 1_000);
        portrait.rotation_degrees = 90;
        let mut record = FileRecord::new(portrait);
        record.verdict = Some(Verdict {
            kind: VerdictKind::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
            },
            source_run: Some(RunId(1)),
            decided_at: UnixMillis(0),
        });
        state.records.insert(content_key, record);
        let rows = history_rows(&state);
        assert_eq!((rows[0].width, rows[0].height), (Some(1080), Some(1920)));
    }

    proptest! {
        #[test]
        fn savings_identities_hold(
            sizes in proptest::collection::vec(
                (0u64..1_000_000_000_000, 0u64..1_000_000_000_000),
                0..40,
            )
        ) {
            let state = converted_state(&sizes, DAY_MS * 20_000);
            let payload = statistics(&state, 0);

            let expected_input: u128 = sizes.iter().map(|(input, _)| u128::from(*input)).sum();
            let expected_output: u128 = sizes.iter().map(|(_, output)| u128::from(*output)).sum();
            let expected_saved =
                i128::try_from(expected_input).unwrap() - i128::try_from(expected_output).unwrap();
            prop_assert_eq!(u128::from(payload.total_input_bytes), expected_input);
            prop_assert_eq!(u128::from(payload.total_output_bytes), expected_output);
            prop_assert_eq!(i128::from(payload.total_saved_bytes), expected_saved);

            // Every sized fact lands in exactly one bin or the grew counter.
            let binned: u32 = payload.reduction_bins.iter().sum();
            let with_reduction = sizes.iter().filter(|(input, _)| *input > 0).count() as u32;
            prop_assert_eq!(binned + payload.grew_count, with_reduction);

            // The cumulative series ends at the total (all facts carry sizes).
            if let Some(last) = payload.cumulative_savings.last() {
                prop_assert_eq!(last.cumulative_saved_bytes, payload.total_saved_bytes);
            } else {
                prop_assert_eq!(payload.total_saved_bytes, 0);
            }
        }

        #[test]
        fn statistics_are_deterministic(count in 0usize..20) {
            let sizes: Vec<(u64, u64)> = (0..count)
                .map(|index| ((index as u64 + 1) * 1_000, (index as u64 + 1) * 400))
                .collect();
            let state = converted_state(&sizes, DAY_MS * 20_000);
            prop_assert_eq!(statistics(&state, 0), statistics(&state, 0));
            let rows_a = history_rows(&state);
            let rows_b = history_rows(&state);
            prop_assert_eq!(rows_a, rows_b);
        }
    }
}
