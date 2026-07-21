/**
 * Pure display formatters. Their semantics were frozen from the V2 app into
 * parity-fixtures.json, which is now the spec: hand-maintained data, changed
 * only as a deliberate, reviewed edit — never regenerated.
 *
 * Deliberate divergence: invalid input (negative or non-finite sizes) renders
 * "—"; the V2 original returned "-" only there, "—" everywhere else.
 */

export type TimeConfidence = "high" | "precise" | "medium" | "low" | "none";

/** A per-hour GiB rate at or above this renders without decimals. */
export const EFFICIENCY_DECIMAL_THRESHOLD = 10;

const EM_DASH = "—";
const GIB = 1024 ** 3;

/**
 * Compact duration for tree rows, e.g. "2h 15m", "~45m", "~~< 1m".
 * Confidence prefixes: none for exact/unknown, "~" for a similar-file
 * estimate, "~~" for a statistical codec/duration estimate.
 */
export function formatCompactTime(seconds: number, confidence: TimeConfidence = "none"): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return EM_DASH;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const prefix = confidence === "medium" ? "~" : confidence === "low" ? "~~" : "";
  if (hours > 0) return `${prefix}${hours}h ${minutes}m`;
  if (minutes > 0) return `${prefix}${minutes}m`;
  return `${prefix}< 1m`;
}

/** Savings-per-time as "2.5 GB/h" (one decimal below 10, none at or above). */
export function formatEfficiency(savingsBytes: number, timeSeconds: number): string {
  if (savingsBytes <= 0 || timeSeconds <= 0) return EM_DASH;
  const gbPerHr = savingsBytes / GIB / (timeSeconds / 3600);
  if (gbPerHr >= EFFICIENCY_DECIMAL_THRESHOLD) return `${gbPerHr.toFixed(0)} GB/h`;
  return `${gbPerHr.toFixed(1)} GB/h`;
}

/**
 * An already-computed input throughput in GiB/h. Statistics owns this value;
 * unlike `formatEfficiency`, this formatter never derives a rate from bytes.
 */
export function formatInputThroughput(gibPerHour: number | null): string {
  if (gibPerHour === null || !Number.isFinite(gibPerHour) || gibPerHour <= 0) return EM_DASH;
  if (gibPerHour >= EFFICIENCY_DECIMAL_THRESHOLD) return `${gibPerHour.toFixed(0)} GiB/h`;
  return `${gibPerHour.toFixed(1)} GiB/h`;
}

/** Clock-style duration: "h:mm:ss" above an hour, "m:ss" below. */
export function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "--:--:--";
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);
  const ss = String(secs).padStart(2, "0");
  if (hours > 0) return `${hours}:${String(minutes).padStart(2, "0")}:${ss}`;
  return `${minutes}:${ss}`;
}

/** Byte count with binary units: "512 B", "1.50 KB", "2.34 GB". */
export function formatFileSize(sizeBytes: number): string {
  if (!Number.isFinite(sizeBytes) || sizeBytes < 0) return EM_DASH;
  if (sizeBytes < 1024) return `${Math.trunc(sizeBytes)} B`;
  if (sizeBytes < 1024 ** 2) return `${(sizeBytes / 1024).toFixed(2)} KB`;
  if (sizeBytes < GIB) return `${(sizeBytes / 1024 ** 2).toFixed(2)} MB`;
  return `${(sizeBytes / GIB).toFixed(2)} GB`;
}

/**
 * CRF without a trailing ".0": 23 → "23", 23.25 → "23.25". ab-av1 0.11+
 * searches in quarter-CRF steps, so fractional values are real.
 */
export function formatCrf(crf: number | null): string {
  if (crf === null) return "?";
  return String(Number.parseFloat(crf.toPrecision(6)));
}

/** Audio streams beyond this count collapse to "N audio". */
const MAX_AUDIO_STREAMS_TO_LIST = 3;

/** Stream summary for the format column: "H264 / AAC, AC3", "AV1 / no audio". */
export function formatStreamDisplay(
  videoCodec: string | null,
  audioCodecs: readonly string[],
): string {
  const video = (videoCodec ?? "?").toUpperCase();
  const audio = audioCodecs.length === 0 ? "no audio" : formatAudioCodecs(audioCodecs);
  return `${video} / ${audio}`;
}

/** History audio column: "AAC, AC3", "5 audio", or "—" for a file with none. */
export function formatAudioCodecs(audioCodecs: readonly string[]): string {
  if (audioCodecs.length === 0) return EM_DASH;
  if (audioCodecs.length <= MAX_AUDIO_STREAMS_TO_LIST) {
    return audioCodecs.map((c) => c.toUpperCase()).join(", ");
  }
  return `${audioCodecs.length} audio`;
}

/**
 * Wall-clock date "YYYY-MM-DD" in local time from epoch milliseconds (the
 * engine stamps instants as wall-clock ms). Fixed format per #36 D8; the
 * The V2 column sliced an ISO string, so parity here is by-construction
 * rather than fixture-generated.
 */
export function formatDate(epochMs: number): string {
  if (!Number.isFinite(epochMs)) return EM_DASH;
  const d = new Date(epochMs);
  const month = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${month}-${day}`;
}

/** "1920x1080", or "—" when either dimension is missing or zero. */
export function formatResolution(width: number | null, height: number | null): string {
  if (!width || !height) return EM_DASH;
  return `${width}x${height}`;
}

/**
 * Source bitrate: "4.2 Mbps" at or above 1000 kbps, "850 kbps" below.
 * Deliberate divergence: negative or non-finite input renders "—".
 */
export function formatBitrate(kbps: number | null): string {
  if (kbps === null || !Number.isFinite(kbps) || kbps <= 0) return EM_DASH;
  if (kbps >= 1000) return `${(kbps / 1000).toFixed(1)} Mbps`;
  return `${kbps.toFixed(0)} kbps`;
}

/** Size reduction with one decimal: "45.3%" (0 is a real value, "0.0%"). */
export function formatReductionPercent(percent: number | null): string {
  if (percent === null || !Number.isFinite(percent)) return EM_DASH;
  return `${percent.toFixed(1)}%`;
}

/** VMAF score with one decimal: "95.1". */
export function formatVmaf(score: number | null): string {
  if (score === null || !Number.isFinite(score)) return EM_DASH;
  return score.toFixed(1);
}
