import { skipReasonText } from "@/lib/format/skip-reason";
import type {
  Crf,
  DurationMs,
  ItemOutcome,
  JobPhase,
  Operation,
  OutputTarget,
  QueueItem,
  QueueItemState,
  RunId,
  Telemetry,
  MetricScore,
} from "@/lib/bindings";
import { formatVmafTarget } from "@/lib/format/engine-values";

/** D11 confidence ramp: values step down a muted-color ramp, no tildes. */
export type EstimateConfidence = "exact" | "estimate" | "rough";

/** Everything the status cell needs, derived once per row. */
export type RowStatus =
  | { kind: "queued" }
  | { kind: "starting" }
  | { kind: "working"; phase: JobPhase; percent: number | null }
  | {
      kind: "done";
      outcome: "Analyzed" | "Converted" | "Remuxed";
      sizeDeltaBytes: number | null;
      recovered: boolean;
    }
  | { kind: "skipped"; reason: string; detail: string | null }
  | { kind: "stopped" }
  | { kind: "incomplete" }
  | { kind: "failed"; message: string; diagnostic: string | null };

/**
 * Durable display model for one queue row. Live telemetry is deliberately
 * absent: each row subscribes to its own RunId so progress ticks do not
 * rebuild the table or disturb selection.
 */
export interface QueueRowData {
  item: QueueItem;
  runId: RunId | null;
  streams: string | null;
  sizeBytes: number | null;
  mediaDurationMs: DurationMs | null;
  timeMs: DurationMs | null;
  timeConfidence: EstimateConfidence;
  crf: Crf | null;
  quality: MetricScore | null;
  status: RowStatus;
}

const BASIS_POINTS_PER_PERCENT = 100;

/** Last path segment, tolerating both separators: queue inputs are OS paths. */
export function basename(path: string): string {
  const segments = path.split(/[\\/]/).filter((segment) => segment.length > 0);
  return segments[segments.length - 1] ?? path;
}

export function deriveRowStatus(
  state: QueueItemState,
  telemetry: Telemetry | null,
  durationMs: number | null,
  sizeDeltaBytes: number | null,
): RowStatus {
  if (state === "Queued") return { kind: "queued" };
  if ("Reserved" in state || "Claimed" in state) return { kind: "starting" };
  if ("Running" in state) {
    if (telemetry === null) return { kind: "working", phase: "Preparing", percent: null };
    return {
      kind: "working",
      phase: telemetry.phase,
      percent: telemetryPercent(telemetry, durationMs),
    };
  }
  return outcomeStatus(state.Finished, sizeDeltaBytes);
}

function telemetryPercent(telemetry: Telemetry, durationMs: number | null): number | null {
  const { progress } = telemetry;
  if (progress === "Phase") return null;
  if (progress.SearchBasisPoints !== undefined) {
    return clampPercent(progress.SearchBasisPoints / BASIS_POINTS_PER_PERCENT);
  }
  if (durationMs === null || durationMs <= 0) return null;
  return clampPercent((progress.OutputPositionMs / durationMs) * 100);
}

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, Math.round(value)));
}

function outcomeStatus(outcome: ItemOutcome, sizeDeltaBytes: number | null): RowStatus {
  if (outcome === "Analyzed") {
    return { kind: "done", outcome, sizeDeltaBytes: null, recovered: false };
  }
  if (outcome === "Stopped") return { kind: "stopped" };
  if (outcome === "Incomplete") return { kind: "incomplete" };
  if ("Converted" in outcome && outcome.Converted !== undefined) {
    return {
      kind: "done",
      outcome: "Converted",
      sizeDeltaBytes,
      recovered: outcome.Converted === "RecoveredAtStartup",
    };
  }
  if ("Remuxed" in outcome && outcome.Remuxed !== undefined) {
    return {
      kind: "done",
      outcome: "Remuxed",
      sizeDeltaBytes,
      recovered: outcome.Remuxed === "RecoveredAtStartup",
    };
  }
  if (outcome.Failed !== undefined) {
    return {
      kind: "failed",
      message: outcome.Failed.message,
      diagnostic: outcome.Failed.diagnostic || null,
    };
  }
  if (outcome.Skipped !== undefined) {
    return { kind: "skipped", ...skipReasonText(outcome.Skipped.reason) };
  }
  return { kind: "skipped", reason: "not worthwhile", detail: notWorthwhileDetail(outcome) };
}

function notWorthwhileDetail(outcome: Extract<ItemOutcome, object>): string | null {
  if (outcome.NotWorthwhile === undefined) return null;
  const attempts = outcome.NotWorthwhile.attempts;
  const last = attempts[attempts.length - 1];
  if (last === undefined || last.last_measurement === null) return null;
  const savedPercent = 100 - last.last_measurement.predicted_percent_basis_points / 100;
  return `Best attempt saved ${savedPercent.toFixed(0)}% at the VMAF ${formatVmafTarget(last.target)} floor`;
}

/** Output column text; Analyze items produce no file (parity rule). */
export function outputTargetLabel(operation: Operation, target: OutputTarget): string {
  if (operation === "Analyze") return "—";
  if (target === "Replace") return "Replace";
  if (target.Suffix !== undefined) return `Suffix ${target.Suffix.suffix}`;
  return basename(target.SeparateFolder.directory) || "Folder";
}
