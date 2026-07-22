import type {
  CodecCount,
  CumulativeSavingsPoint,
  RunTotals,
  StatisticsPayload,
  VideoCodec,
} from "@/lib/bindings";
import { formatFileSize } from "@/lib/format/format";

const MILLIS_PER_DAY = 86_400_000;

export interface ReductionRow {
  label: string;
  files: number;
}

export interface CodecRow {
  label: string;
  files: number;
}

export interface CumulativeRow {
  date: string;
  savedBytes: number;
}

export interface OutcomeRow {
  label: string;
  count: number;
}

const RUN_LABELS: ReadonlyArray<[keyof RunTotals, string]> = [
  ["analyzed", "Analyzed"],
  ["converted", "Converted"],
  ["remuxed", "Remuxed"],
  ["not_worthwhile", "Not worthwhile"],
  ["stopped", "Stopped"],
  ["skipped", "Skipped"],
  ["failed", "Failed"],
];

/** Empty means no standing outcome and no terminal run of any kind. */
export function hasStatisticsData(payload: StatisticsPayload): boolean {
  return (
    payload.converted_files > 0 ||
    payload.remuxed_files > 0 ||
    payload.not_worthwhile_files > 0 ||
    RUN_LABELS.some(([key]) => payload.runs[key] > 0)
  );
}

/**
 * Savings are signed in Statistics. The shared file-size formatter rejects
 * negatives by design, so preserve the sign and format only the magnitude.
 */
export function formatSignedFileSize(bytes: number): string {
  if (!Number.isFinite(bytes)) return "—";
  if (bytes < 0) return `−${formatFileSize(Math.abs(bytes))}`;
  return formatFileSize(bytes);
}

/** A local calendar day from the engine, not an instant to timezone-convert. */
export function formatEpochDay(epochDay: number): string {
  if (!Number.isFinite(epochDay)) return "—";
  const date = new Date(Math.trunc(epochDay) * MILLIS_PER_DAY);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toISOString().slice(0, 10);
}

export function reductionRows(bins: readonly number[]): ReductionRow[] {
  return bins.map((files, index) => ({
    label: `${index * 10}–${(index + 1) * 10}%`,
    files,
  }));
}

/** Preserve the backend's count-descending, canonical tie order verbatim. */
export function codecRows(codecs: readonly CodecCount[]): CodecRow[] {
  return codecs.map(({ codec, count }) => ({ label: formatVideoCodec(codec), files: count }));
}

/** Preserve the backend's daily local-calendar ordering and signed movement. */
export function cumulativeRows(points: readonly CumulativeSavingsPoint[]): CumulativeRow[] {
  return points.map(({ epoch_day, cumulative_saved_bytes }) => ({
    date: formatEpochDay(epoch_day),
    savedBytes: cumulative_saved_bytes,
  }));
}

export function runOutcomeRows(runs: RunTotals): OutcomeRow[] {
  return RUN_LABELS.map(([key, label]) => ({ label, count: runs[key] }));
}

export function formatVideoCodec(codec: VideoCodec): string {
  if (typeof codec === "object") return codec.Other;
  switch (codec) {
    case "Av1":
      return "AV1";
    case "H264":
      return "H.264";
    case "Hevc":
      return "HEVC";
    case "Vp9":
      return "VP9";
  }
}

export function coverageMessage(payload: StatisticsPayload): string | null {
  if (payload.sized_converted_files === payload.converted_files) return null;
  return `${payload.sized_converted_files.toLocaleString()} of ${payload.converted_files.toLocaleString()} converted standings include both sizes; savings and reduction statistics cover only those files.`;
}
