import type {
  AudioCodec,
  CompletionEvidence,
  ConversionRun,
  DurableState_Deserialize,
  ItemOutcome,
  OutputTransaction_Deserialize,
  QueueItem,
  QueueItemId,
  QueueItemState,
  RunId,
  VideoCodec,
} from "@/lib/bindings";
import { formatStreamDisplay } from "@/lib/format/format";

import { deriveRowStatus, type QueueRowData } from "./queue-status";

interface RunEntry {
  id: RunId;
  run: ConversionRun | undefined;
}

interface CompletionSizes {
  input: number | null;
  output: number | null;
}

function codecName(codec: VideoCodec | AudioCodec): string {
  return typeof codec === "string" ? codec : codec.Other;
}

function runIdFromState(state: QueueItemState): RunId | null {
  if (state === "Queued" || "Finished" in state) return null;
  if ("Reserved" in state && state.Reserved !== undefined) return state.Reserved.run_id;
  if ("Claimed" in state && state.Claimed !== undefined) return state.Claimed.run_id;
  return state.Running.run_id;
}

function latestRunsByItem(state: DurableState_Deserialize): Map<QueueItemId, RunEntry> {
  const latest = new Map<QueueItemId, RunEntry>();
  const runIds = Object.keys(state.conversion_runs)
    .map(Number)
    .sort((left, right) => left - right);
  for (const id of runIds) {
    const run = state.conversion_runs[id];
    latest.set(run.spec.item_id, { id, run });
  }
  return latest;
}

function completionEvidence(outcome: ItemOutcome): CompletionEvidence | null {
  if (typeof outcome === "string") return null;
  if (outcome.Converted !== undefined) return outcome.Converted;
  if (outcome.Remuxed !== undefined) return outcome.Remuxed;
  return null;
}

function liveSizes(evidence: CompletionEvidence | null): CompletionSizes {
  if (evidence === null || evidence === "RecoveredAtStartup") {
    return { input: null, output: null };
  }
  if (evidence.LiveEncode !== undefined) {
    return { input: evidence.LiveEncode.input_size, output: evidence.LiveEncode.output_size };
  }
  return { input: evidence.LiveRemux.input_size, output: evidence.LiveRemux.output_size };
}

function settledOutputSize(transaction: OutputTransaction_Deserialize | undefined): number | null {
  if (transaction === undefined || typeof transaction.state === "string") return null;
  const state = transaction.state;
  const identity =
    state.Committed?.final_identity ??
    state.RetireIntent?.final_identity ??
    state.Retired?.final_identity;
  return identity?.destructive.size ?? null;
}

function completionSizes(
  item: QueueItem,
  run: RunEntry | undefined,
  state: DurableState_Deserialize,
): CompletionSizes {
  if (item.state === "Queued" || !("Finished" in item.state)) {
    return { input: null, output: null };
  }
  const outcome = item.state.Finished;
  if (outcome === undefined) return { input: null, output: null };
  const live = liveSizes(completionEvidence(outcome));
  const transaction = run === undefined ? undefined : state.outputs[run.id];
  return {
    input: live.input ?? transaction?.input_identity.size ?? null,
    output: live.output ?? settledOutputSize(transaction),
  };
}

function totalRunTime(run: ConversionRun | undefined): number | null {
  if (run === undefined || run.phase_spans.length === 0) return null;
  return run.phase_spans.reduce((total, span) => total + span.duration, 0);
}

/**
 * Project the authoritative durable Queue in its stored order. The result is
 * recreated only for durable changes; QueueRow performs its own per-RunId
 * telemetry subscription for high-frequency progress.
 */
export function queueRows(state: DurableState_Deserialize): QueueRowData[] {
  const latestRuns = latestRunsByItem(state);
  return state.queue.map((item) => {
    const stateRunId = runIdFromState(item.state);
    const latest = latestRuns.get(item.id);
    const run =
      stateRunId === null ? latest : { id: stateRunId, run: state.conversion_runs[stateRunId] };
    const contentKey = run?.run?.spec.content_key;
    const metadata =
      contentKey === null || contentKey === undefined
        ? null
        : (state.records[contentKey]?.metadata ?? null);
    const sizes = completionSizes(item, run, state);
    const inputSize = sizes.input ?? metadata?.size_bytes ?? null;
    const sizeDelta =
      sizes.input === null || sizes.output === null ? null : sizes.input - sizes.output;
    const finished = item.state !== "Queued" && "Finished" in item.state;
    const analysis = run?.run?.analysis ?? null;
    return {
      item,
      runId: run?.id ?? null,
      streams:
        metadata === null
          ? null
          : formatStreamDisplay(
              codecName(metadata.codec),
              metadata.audio.map((stream) => codecName(stream.codec)),
            ),
      sizeBytes: inputSize,
      mediaDurationMs: metadata?.duration_ms ?? null,
      timeMs: finished ? totalRunTime(run?.run) : null,
      timeConfidence: "exact",
      crf: analysis?.measurement.crf ?? null,
      vmaf: analysis?.measurement.score ?? null,
      status: deriveRowStatus(item.state, null, metadata?.duration_ms ?? null, sizeDelta),
    };
  });
}
