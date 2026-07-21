import type {
  ItemOutcome,
  JobPhase,
  Operation,
  OutputTarget,
  QueueItem,
  QueueItemState,
  SkipReason,
  Telemetry,
} from "@/lib/bindings";

/** D11 confidence ramp: values step down a muted-color ramp, no tildes. */
export type EstimateConfidence = "exact" | "estimate" | "rough";

/** Everything the status cell needs, derived once per row. */
export type RowStatus =
  | { kind: "queued" }
  | { kind: "starting" }
  | { kind: "working"; phase: JobPhase; percent: number | null }
  | { kind: "done"; outcome: "Analyzed" | "Converted" | "Remuxed"; savedBytes: number | null }
  | { kind: "skipped"; reason: string; detail: string | null }
  | { kind: "stopped" }
  | { kind: "failed"; message: string };

/**
 * Display model for one queue row. The stream/size/time/preciseCrf fields
 * come from joins the wiring layer performs (media metadata, cached
 * analyses, telemetry); they are null until known.
 */
export interface QueueRowData {
  item: QueueItem;
  streams: string | null;
  sizeBytes: number | null;
  timeSec: number | null;
  timeConfidence: EstimateConfidence;
  preciseCrf: boolean;
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
  savedBytes: number | null,
): RowStatus {
  if (state === "Queued") return { kind: "queued" };
  if ("Reserved" in state || "Claimed" in state) return { kind: "starting" };
  if ("Running" in state) {
    if (telemetry === null) return { kind: "working", phase: "Preparing", percent: null };
    return {
      kind: "working",
      phase: telemetry.phase,
      percent: progressPercent(telemetry, durationMs),
    };
  }
  return outcomeStatus(state.Finished, savedBytes);
}

function progressPercent(telemetry: Telemetry, durationMs: number | null): number | null {
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

function outcomeStatus(outcome: ItemOutcome, savedBytes: number | null): RowStatus {
  if (outcome === "Analyzed") return { kind: "done", outcome, savedBytes: null };
  if (outcome === "Stopped") return { kind: "stopped" };
  if ("Converted" in outcome && outcome.Converted !== undefined) {
    return { kind: "done", outcome: "Converted", savedBytes };
  }
  if ("Remuxed" in outcome && outcome.Remuxed !== undefined) {
    return { kind: "done", outcome: "Remuxed", savedBytes };
  }
  if (outcome.Failed !== undefined) return { kind: "failed", message: outcome.Failed.message };
  if (outcome.Skipped !== undefined) {
    return { kind: "skipped", ...skipReasonText(outcome.Skipped.reason) };
  }
  return { kind: "skipped", reason: "not worthwhile", detail: notWorthwhileDetail(outcome) };
}

function skipReasonText(reason: SkipReason): { reason: string; detail: string | null } {
  if (reason === "AlreadyAv1Matroska") {
    return { reason: "already AV1", detail: "Already AV1 in an MKV container — nothing to do" };
  }
  if (reason === "OutputExists") {
    return { reason: "output exists", detail: "The output file already exists" };
  }
  if (reason === "AlreadyQueued") {
    return { reason: "already queued", detail: "This path is already in the queue" };
  }
  if ("LowResolution" in reason && reason.LowResolution !== undefined) {
    return {
      reason: "below minimum resolution",
      detail: `${reason.LowResolution.pixels.toLocaleString()} px is under the ${reason.LowResolution.minimum.toLocaleString()} px minimum`,
    };
  }
  if ("AlreadyConverted" in reason && reason.AlreadyConverted !== undefined) {
    return {
      reason: "already converted",
      detail: "This file is the output of a previous conversion",
    };
  }
  if ("NotWorthwhile" in reason && reason.NotWorthwhile !== undefined) {
    return {
      reason: "not worthwhile",
      detail: "A previous analysis found no worthwhile savings",
    };
  }
  return {
    reason: "duplicate content",
    detail: "Identical content was already converted",
  };
}

function notWorthwhileDetail(outcome: Extract<ItemOutcome, object>): string | null {
  if (outcome.NotWorthwhile === undefined) return null;
  const attempts = outcome.NotWorthwhile.attempts;
  const last = attempts[attempts.length - 1];
  if (last === undefined || last.last_measurement === null) return null;
  const savedPercent = 100 - last.last_measurement.predicted_percent_basis_points / 100;
  return `Best attempt saved ${savedPercent.toFixed(0)}% at the VMAF ${last.target} floor`;
}

/** Output column text; Analyze items produce no file (parity rule). */
export function outputTargetLabel(operation: Operation, target: OutputTarget): string {
  if (operation === "Analyze") return "—";
  if (target === "Replace") return "Replace";
  if (target.Suffix !== undefined) return `Suffix ${target.Suffix.suffix}`;
  return basename(target.SeparateFolder.directory) || "Folder";
}
