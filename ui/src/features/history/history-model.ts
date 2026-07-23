import type {
  AudioCodec,
  DurableState_Deserialize,
  HistoryRow,
  HistoryStatus,
  MediaContainer,
  VideoCodec,
} from "@/lib/bindings";

export const HISTORY_STATUSES = [
  "Converted",
  "Remuxed",
  "Not Worthwhile",
  "Analyzed",
  "Failed",
  "Stopped",
] as const;

export type HistoryStatusLabel = (typeof HISTORY_STATUSES)[number];
type HistoryProvenance = "native" | "adopted" | "parked" | "unknown";

export interface HistoryDisplayRow {
  /** Tagged domain identity. Never derived from a visible index or label. */
  id: string;
  row: HistoryRow;
  label: string;
  basename: string;
  path: string | null;
  provenance: HistoryProvenance;
  status: HistoryStatusLabel;
  statusDetail: string | null;
  /** Positive means saved space; negative explicitly records output growth. */
  reductionPercent: number | null;
}

export interface HistoryTotals {
  records: number;
  sizedRecords: number;
  inputBytes: number;
  outputBytes: number;
  savedBytes: number;
}

const UNKNOWN_FILE = "Unknown file";

/** Serialize both union arms so identical values cannot collide across tags. */
export function historyRowId(key: HistoryRow["key"]): string {
  return JSON.stringify([key.kind, key.value]);
}

/** Last path segment while accepting both Windows and Unix separators. */
function historyBasename(path: string): string {
  const segments = path.split(/[\\/]/).filter((segment) => segment.length > 0);
  return segments.at(-1) ?? path;
}

export function statusPresentation(status: HistoryStatus): {
  label: HistoryStatusLabel;
  detail: string | null;
} {
  if (status === "Converted" || status === "Remuxed" || status === "Stopped") {
    return { label: status, detail: null };
  }
  if (status === "Analyzed") {
    return {
      label: "Analyzed",
      detail: "Historical analysis result; current reuse is checked when the file is queued.",
    };
  }
  if ("NotWorthwhile" in status && status.NotWorthwhile !== undefined) {
    return {
      label: "Not Worthwhile",
      detail: `No worthwhile result from VMAF ${status.NotWorthwhile.requested} through ${status.NotWorthwhile.floor}.`,
    };
  }
  return { label: "Failed", detail: status.Failed.message };
}

function resolveLabel(
  row: HistoryRow,
  state: DurableState_Deserialize,
): { label: string; path: string | null; provenance: HistoryProvenance } {
  if (row.source_run !== null) {
    const runPath = state.conversion_runs[row.source_run]?.spec.input;
    if (runPath !== undefined && runPath.length > 0) {
      return { label: runPath, path: runPath, provenance: "native" };
    }
  }

  if (row.key.kind === "Content") {
    const importPath = state.records[row.key.value]?.imported?.import_path;
    if (importPath !== undefined && importPath.length > 0) {
      return { label: importPath, path: importPath, provenance: "adopted" };
    }
  } else if (row.key.value.length > 0) {
    return { label: row.key.value, path: row.key.value, provenance: "parked" };
  }

  return { label: UNKNOWN_FILE, path: null, provenance: "unknown" };
}

export function reductionPercent(row: HistoryRow): number | null {
  const input = row.input_size_bytes;
  const output = row.output_size_bytes;
  if (
    input === null ||
    output === null ||
    !Number.isFinite(input) ||
    !Number.isFinite(output) ||
    input <= 0 ||
    output < 0
  ) {
    return null;
  }
  return ((input - output) / input) * 100;
}

export function historyDisplayRows(
  rows: readonly HistoryRow[],
  state: DurableState_Deserialize,
): HistoryDisplayRow[] {
  return rows.map((row) => {
    const resolved = resolveLabel(row, state);
    const status = statusPresentation(row.status);
    return {
      id: historyRowId(row.key),
      row,
      label: resolved.label,
      basename: resolved.path === null ? resolved.label : historyBasename(resolved.label),
      path: resolved.path,
      provenance: resolved.provenance,
      status: status.label,
      statusDetail: status.detail,
      reductionPercent: reductionPercent(row),
    };
  });
}

/** Newest first, missing dates last, then tagged key ascending. */
export function compareHistoryDefault(a: HistoryDisplayRow, b: HistoryDisplayRow): number {
  const aDate = a.row.happened_at;
  const bDate = b.row.happened_at;
  if (aDate === null && bDate !== null) return 1;
  if (aDate !== null && bDate === null) return -1;
  if (aDate !== null && bDate !== null && aDate !== bDate) return bDate - aDate;
  return a.id.localeCompare(b.id);
}

/** Totals are presentation-only summaries over already-deduplicated rows. */
export function historyTotals(rows: readonly HistoryDisplayRow[]): HistoryTotals {
  return rows.reduce<HistoryTotals>(
    (totals, displayRow) => {
      totals.records += 1;
      const input = displayRow.row.input_size_bytes;
      const output = displayRow.row.output_size_bytes;
      if (
        input !== null &&
        output !== null &&
        Number.isFinite(input) &&
        Number.isFinite(output) &&
        input > 0 &&
        output >= 0
      ) {
        totals.sizedRecords += 1;
        totals.inputBytes += input;
        totals.outputBytes += output;
        totals.savedBytes += input - output;
      }
      return totals;
    },
    { records: 0, sizedRecords: 0, inputBytes: 0, outputBytes: 0, savedBytes: 0 },
  );
}

function otherLabel(value: { Other: string }): string {
  return value.Other.length > 0 ? value.Other.toUpperCase() : "OTHER";
}

export function videoCodecLabel(codec: VideoCodec | null): string {
  if (codec === null) return "—";
  if (typeof codec === "object") return otherLabel(codec);
  const labels: Record<Exclude<VideoCodec, object>, string> = {
    Av1: "AV1",
    H264: "H.264",
    Hevc: "HEVC",
    Vp9: "VP9",
  };
  return labels[codec];
}

export function containerLabel(container: MediaContainer | null): string {
  if (container === null) return "—";
  if (typeof container === "object") return otherLabel(container);
  return "MKV";
}

function audioCodecLabel(codec: AudioCodec): string {
  if (typeof codec === "object") return otherLabel(codec);
  const labels: Record<Exclude<AudioCodec, object>, string> = {
    Aac: "AAC",
    Ac3: "AC3",
    Eac3: "E-AC3",
    Dts: "DTS",
    Opus: "Opus",
    Flac: "FLAC",
    Mp3: "MP3",
  };
  return labels[codec];
}

export function audioSummary(audio: AudioCodec[] | null): string {
  if (audio === null) return "—";
  if (audio.length === 0) return "No audio";
  if (audio.length > 3) return `${audio.length} audio`;
  return audio.map(audioCodecLabel).join(", ");
}
