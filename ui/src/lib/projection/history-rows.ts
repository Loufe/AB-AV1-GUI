// TypeScript mirror of the Rust History projection: `history_rows` and its
// size join in crfty-core/src/projection.rs. The rows never cross IPC — the
// snapshot already carries the full durable state, so the frontend derives
// them locally; the Rust definition stays the oracle. Pure functions only:
// no store imports, no I/O. Golden fixtures exported from the Rust
// projection prove agreement (projection-fixtures.json, replayed by
// history-rows.test.ts).

import type {
  AnalysisResult,
  ContentKey,
  ConversionRun,
  DurableState_Deserialize,
  FileRecord_Deserialize,
  HistoryRow,
  HistoryStatus,
  ImportedHistoryRecord,
  RunId,
  Verdict,
  VideoMeta,
} from "@/lib/bindings";

/**
 * Input/output sizes for the run backing a verdict, joined in evidence →
 * settled-transaction → verdict-carried-summary → metadata order (mirrors
 * `joined_sizes`). The carried tier covers adopted verdicts with no backing
 * run. Queue and analysis views share this join.
 */
export function joinedSizes(
  verdict: Verdict,
  run: ConversionRun | undefined,
  state: DurableState_Deserialize,
  record: FileRecord_Deserialize,
): { input: number | null; output: number | null } {
  const outcome = run?.outcome;
  if (outcome !== undefined && outcome !== null && typeof outcome === "object") {
    const evidence =
      "Converted" in outcome && outcome.Converted !== undefined
        ? outcome.Converted
        : "Remuxed" in outcome && outcome.Remuxed !== undefined
          ? outcome.Remuxed
          : null;
    if (evidence !== null && typeof evidence === "object") {
      if ("LiveEncode" in evidence && evidence.LiveEncode !== undefined) {
        return { input: evidence.LiveEncode.input_size, output: evidence.LiveEncode.output_size };
      }
      if ("LiveRemux" in evidence && evidence.LiveRemux !== undefined) {
        return { input: evidence.LiveRemux.input_size, output: evidence.LiveRemux.output_size };
      }
    }
  }
  const transaction = verdict.source_run === null ? undefined : state.outputs[verdict.source_run];
  if (transaction !== undefined) {
    const transactionState = transaction.state;
    if (typeof transactionState === "object") {
      const settled =
        "Committed" in transactionState && transactionState.Committed !== undefined
          ? transactionState.Committed.final_identity
          : "RetireIntent" in transactionState && transactionState.RetireIntent !== undefined
            ? transactionState.RetireIntent.final_identity
            : "Retired" in transactionState && transactionState.Retired !== undefined
              ? transactionState.Retired.final_identity
              : null;
      if (settled !== null) {
        return { input: transaction.input_identity.size, output: settled.destructive.size };
      }
    }
  }
  const carried = carriedSizes(verdict);
  return { input: carried.input ?? record.metadata.size_bytes, output: carried.output };
}

function carriedSizes(verdict: Verdict): { input: number | null; output: number | null } {
  const kind = verdict.kind;
  if ("Converted" in kind && kind.Converted !== undefined) {
    return { input: kind.Converted.input_size, output: kind.Converted.output_size };
  }
  if ("Remuxed" in kind && kind.Remuxed !== undefined) {
    return { input: kind.Remuxed.input_size, output: kind.Remuxed.output_size };
  }
  return { input: null, output: null };
}

/**
 * Project native/adopted content rows in content-key order, followed by
 * unresolved imported rows in normalized import-path order.
 * Filtering and sorting are frontend concerns. Mirrors `history_rows`: a
 * standing verdict wins; without one, the latest failed or stopped run
 * reports with its reason; without that, completed analyses report as
 * Analyzed; scanned-only content gets no row.
 */
export function historyRows(state: DurableState_Deserialize): HistoryRow[] {
  const latestAnalysis = new Map<ContentKey, { runId: RunId; analysis: AnalysisResult }>();
  const latestInterruption = new Map<ContentKey, { runId: RunId; run: ConversionRun }>();
  // Ascending run-id order so later runs overwrite earlier ones, matching
  // the Rust BTreeMap iteration.
  const runIds = Object.keys(state.conversion_runs)
    .map(Number)
    .sort((a, b) => a - b);
  for (const runId of runIds) {
    const run = state.conversion_runs[runId];
    const contentKey = run.spec.content_key;
    if (contentKey === null) {
      continue;
    }
    if (run.analysis !== null) {
      latestAnalysis.set(contentKey, { runId, analysis: run.analysis });
    }
    const outcome = run.outcome;
    if (
      outcome === "Stopped" ||
      (outcome !== null &&
        typeof outcome === "object" &&
        "Failed" in outcome &&
        outcome.Failed !== undefined)
    ) {
      latestInterruption.set(contentKey, { runId, run });
    }
  }

  const rows: HistoryRow[] = [];
  // Rust iterates records in ContentKey order; keys are ASCII hashes, so
  // code-unit sorting reproduces the byte-wise BTreeMap order.
  const contentKeys = Object.keys(state.records).sort();
  for (const contentKey of contentKeys) {
    const record = state.records[contentKey];
    const interruption = latestInterruption.get(contentKey);
    if (record.verdict !== null) {
      const imported = record.imported?.record;
      const status = imported === undefined ? null : importedStatus(imported);
      rows.push(
        record.verdict.source_run === null && imported !== undefined && status !== null
          ? importedRow({ kind: "Content", value: contentKey }, imported, status)
          : verdictRow(contentKey, record, record.verdict, state),
      );
    } else if (interruption !== undefined) {
      rows.push(interruptionRow(contentKey, record, interruption.runId, interruption.run));
    } else if (record.analyses.length > 0) {
      rows.push(analyzedRow(contentKey, record, latestAnalysis.get(contentKey)));
    } else if (record.imported !== null) {
      const status = importedStatus(record.imported.record);
      if (status !== null) {
        rows.push(
          importedRow({ kind: "Content", value: contentKey }, record.imported.record, status),
        );
      }
    }
  }
  for (const importPath of Object.keys(state.parked).sort()) {
    const imported = state.parked[importPath];
    const status = importedStatus(imported);
    if (status !== null) {
      rows.push(importedRow({ kind: "Parked", value: importPath }, imported, status));
    }
  }
  return rows;
}

function postRotationDimensions(metadata: VideoMeta): { width: number; height: number } {
  const remainder = ((metadata.rotation_degrees % 180) + 180) % 180;
  return remainder === 90
    ? { width: metadata.height, height: metadata.width }
    : { width: metadata.width, height: metadata.height };
}

function baseRow(
  contentKey: ContentKey,
  record: FileRecord_Deserialize,
  status: HistoryStatus,
): HistoryRow {
  const { width, height } = postRotationDimensions(record.metadata);
  return {
    key: { kind: "Content", value: contentKey },
    status,
    source_run: null,
    happened_at: null,
    codec: record.metadata.codec,
    container: record.metadata.container,
    width,
    height,
    duration_ms: record.metadata.duration_ms,
    audio: record.metadata.audio.map((stream) => stream.codec),
    input_size_bytes: record.metadata.size_bytes,
    output_size_bytes: null,
    encoding_time_ms: null,
    vmaf: null,
    crf: null,
  };
}

function verdictRow(
  contentKey: ContentKey,
  record: FileRecord_Deserialize,
  verdict: Verdict,
  state: DurableState_Deserialize,
): HistoryRow {
  const kind = verdict.kind;
  let status: HistoryStatus;
  if ("Converted" in kind && kind.Converted !== undefined) {
    status = "Converted";
  } else if ("Remuxed" in kind && kind.Remuxed !== undefined) {
    status = "Remuxed";
  } else {
    status = {
      NotWorthwhile: {
        requested: kind.NotWorthwhile.requested,
        floor: kind.NotWorthwhile.floor,
      },
    };
  }
  const run: ConversionRun | undefined =
    verdict.source_run === null ? undefined : state.conversion_runs[verdict.source_run];
  const { input, output } = joinedSizes(verdict, run, state, record);
  const measurement = status === "Converted" ? (run?.analysis?.measurement ?? null) : null;
  const carried =
    "Converted" in kind && kind.Converted !== undefined
      ? { crf: kind.Converted.crf, vmaf: kind.Converted.vmaf }
      : { crf: null, vmaf: null };
  const row = baseRow(contentKey, record, status);
  row.source_run = verdict.source_run;
  row.happened_at = run?.finished_at ?? verdict.decided_at;
  if (input !== null) {
    row.input_size_bytes = input;
  }
  row.output_size_bytes = output;
  row.encoding_time_ms =
    run === undefined
      ? "Converted" in kind && kind.Converted !== undefined
        ? kind.Converted.encoding_time
        : null
      : encodingDuration(run);
  row.vmaf = measurement === null ? carried.vmaf : measurement.score;
  row.crf = measurement === null ? carried.crf : measurement.crf;
  return row;
}

function encodingDuration(run: ConversionRun): number {
  return run.phase_spans.reduce(
    (total, span) => (span.phase === "Encoding" ? total + span.duration : total),
    0,
  );
}

function interruptionRow(
  contentKey: ContentKey,
  record: FileRecord_Deserialize,
  runId: RunId,
  run: ConversionRun,
): HistoryRow {
  const outcome = run.outcome;
  const status: HistoryStatus =
    outcome !== null &&
    typeof outcome === "object" &&
    "Failed" in outcome &&
    outcome.Failed !== undefined
      ? { Failed: { kind: outcome.Failed.kind, message: outcome.Failed.message } }
      : "Stopped";
  const row = baseRow(contentKey, record, status);
  row.source_run = runId;
  row.happened_at = run.finished_at;
  return row;
}

function analyzedRow(
  contentKey: ContentKey,
  record: FileRecord_Deserialize,
  latest: { runId: RunId; analysis: AnalysisResult } | undefined,
): HistoryRow {
  const row = baseRow(contentKey, record, "Analyzed");
  // Prefer the analysis attached to the latest run; a record can also carry
  // analyses with no surviving run (future legacy adoption), in which case
  // the last index entry's highest-target analysis stands in. Rust sorts the
  // index by profile Ord — snapshot-derived entry lists preserve that order,
  // which is the case this fallback exists for.
  const analysis = latest?.analysis ?? lastRecordedAnalysis(record);
  row.source_run = latest === undefined ? null : latest.runId;
  row.vmaf = analysis === null ? null : analysis.measurement.score;
  row.crf = analysis === null ? null : analysis.measurement.crf;
  return row;
}

function lastRecordedAnalysis(record: FileRecord_Deserialize): AnalysisResult | null {
  const entry = record.analyses.at(-1);
  if (entry === undefined) {
    return null;
  }
  const byTarget = entry[1];
  const targets = Object.keys(byTarget)
    .map(Number)
    .sort((a, b) => a - b);
  const highest = targets.at(-1);
  return highest === undefined ? null : byTarget[highest];
}

function importedStatus(imported: ImportedHistoryRecord): HistoryStatus | null {
  switch (imported.status) {
    case "Converted":
      return "Converted";
    case "NotWorthwhile":
      return {
        NotWorthwhile: {
          requested: imported.requested_target ?? 95,
          floor: imported.floor_target ?? 90,
        },
      };
    case "Analyzed":
      return "Analyzed";
    case "Scanned":
      return null;
  }
}

function importedRow(
  key: HistoryRow["key"],
  imported: ImportedHistoryRecord,
  status: HistoryStatus,
): HistoryRow {
  return {
    key,
    status,
    source_run: null,
    happened_at: imported.decided_at,
    codec: imported.video_codec,
    container: null,
    width: imported.width,
    height: imported.height,
    duration_ms: imported.duration_ms,
    audio: null,
    input_size_bytes: imported.size,
    output_size_bytes: imported.output_size,
    encoding_time_ms: imported.encoding_time,
    vmaf: imported.vmaf,
    crf: imported.crf,
  };
}
