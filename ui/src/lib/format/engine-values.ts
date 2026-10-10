/**
 * Presentation boundary for serialized engine values.
 *
 * Generated bindings, the event fold, and Zustand retain the engine's wire
 * representation. Components use these entry points instead of remembering
 * fixed-point scales or converting milliseconds ad hoc.
 *
 * Statistics is deliberately different: its CRF, VMAF, and reduction
 * `ValueSpread` members are already normalized human-scale floats, and
 * `gigabytes_per_hour` is already the authoritative input-throughput rate.
 */

import type {
  Crf,
  DurationMs,
  StatisticsPayload,
  UnixMillis,
  QualityScore,
  QualityTarget,
} from "@/lib/bindings";

import {
  formatCompactTime,
  formatCrf,
  formatDate,
  formatInputThroughput,
  formatReductionPercent,
  formatTime,
  formatVmaf,
  type TimeConfidence,
} from "./format";

const CRF_FIXED_SCALE = 1_000;
const VMAF_SCORE_FIXED_SCALE = 100;
const MILLIS_PER_SECOND = 1_000;
const EM_DASH = "—";

function isNonNegativeFinite(value: number): boolean {
  return Number.isFinite(value) && value >= 0;
}

/** Format a serialized fixed-point `Crf` (24,000 means CRF 24). */
export function formatEngineCrf(crf: Crf | null): string {
  if (crf === null || !isNonNegativeFinite(crf)) return formatCrf(null);
  return formatCrf(crf / CRF_FIXED_SCALE);
}

/** VMAF scores carry hundredths of a point; the metric selects the scale. */
export function formatEngineVmafScore(score: QualityScore | null): string {
  if (score === null || !isNonNegativeFinite(score.value)) return formatVmaf(null);
  switch (score.metric) {
    case "Vmaf":
      return formatVmaf(score.value / VMAF_SCORE_FIXED_SCALE);
  }
}

/** Format an engine duration in milliseconds as compact human time. */
export function formatDurationMsCompact(
  durationMs: DurationMs | null,
  confidence: TimeConfidence = "none",
): string {
  if (durationMs === null || !isNonNegativeFinite(durationMs)) return EM_DASH;
  return formatCompactTime(durationMs / MILLIS_PER_SECOND, confidence);
}

/** Format an engine duration in milliseconds as clock-style human time. */
export function formatDurationMsClock(durationMs: DurationMs | null): string {
  if (durationMs === null || !isNonNegativeFinite(durationMs)) return EM_DASH;
  return formatTime(durationMs / MILLIS_PER_SECOND);
}

/** Format a `UnixMillis` instant directly; it is not a duration. */
export function formatUnixMillisDate(instant: UnixMillis | null): string {
  if (instant === null || !isNonNegativeFinite(instant)) return EM_DASH;
  return formatDate(instant);
}

/** VMAF targets carry whole points, unlike measured scores. */
export function formatVmafTarget(target: QualityTarget | null): string {
  if (target === null || !isNonNegativeFinite(target.value)) return EM_DASH;
  switch (target.metric) {
    case "Vmaf":
      return String(target.value);
  }
}

/** Statistics CRF spreads are already normalized; do not divide again. */
export function formatStatisticsCrf(crf: number | null): string {
  if (crf === null || !isNonNegativeFinite(crf)) return formatCrf(null);
  return formatCrf(crf);
}

/** Statistics VMAF spreads are already normalized; do not divide again. */
export function formatStatisticsVmaf(score: number | null): string {
  if (score === null || !isNonNegativeFinite(score)) return formatVmaf(null);
  return formatVmaf(score);
}

/** Statistics reductions are normalized percentages; negatives mean growth. */
export function formatStatisticsReduction(percent: number | null): string {
  return formatReductionPercent(percent);
}

/** Format the backend-owned input throughput without recomputing it. */
export function formatStatisticsInputThroughput(
  throughput: StatisticsPayload["gigabytes_per_hour"],
): string {
  return formatInputThroughput(throughput);
}
