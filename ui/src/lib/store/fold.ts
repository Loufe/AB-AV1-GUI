// TypeScript mirror of the Rust structural fold: `fold`/`fold_config` in
// crfty-core/src/state.rs (including the output-ledger transitions from
// output.rs) and the ephemeral application in reducer.rs. Deltas on the wire
// are pre-validated by the Rust reducer, so every arm applies structurally —
// no validation lives here. Pure functions only: no store imports, no I/O
// (#36 D5). Golden fixtures exported from the Rust fold prove agreement
// (fold-fixtures.json, replayed by fold.test.ts).

import type {
  AnalysisProfile,
  AnalysisResult,
  CompletionEvidence,
  ConfigDelta,
  DecodeMode,
  DurableDelta,
  DurableState_Deserialize,
  EphemeralDelta,
  ExecutionSettings,
  FileRecord_Deserialize,
  ImportedProvenance,
  ItemOutcome,
  JobAction,
  MediaObservation,
  OutputDelta,
  OutputState,
  OutputTransaction,
  PhaseSpan,
  QueueItem,
  QueueItemId,
  QueueItemState,
  RunId,
  SessionState,
  Settings,
  Telemetry,
  UnixMillis,
  VerdictKind,
} from "@/lib/bindings";

export function emptyDurableState(): DurableState_Deserialize {
  return {
    queue: [],
    paths: {},
    records: {},
    outputs: {},
    conversion_runs: {},
    parked: {},
    adopted_imports: [],
  };
}

export function foldDurable(
  state: DurableState_Deserialize,
  delta: DurableDelta,
): DurableState_Deserialize {
  if ("QueueAdded" in delta && delta.QueueAdded !== undefined) {
    return { ...state, queue: [...state.queue, delta.QueueAdded.item] };
  }
  if ("QueueItemsRemoved" in delta && delta.QueueItemsRemoved !== undefined) {
    const removed = new Set(delta.QueueItemsRemoved.item_ids);
    return { ...state, queue: state.queue.filter((item) => !removed.has(item.id)) };
  }
  if ("QueueMoved" in delta && delta.QueueMoved !== undefined) {
    const { item_id, before } = delta.QueueMoved;
    return { ...state, queue: movedQueue(state.queue, item_id, before) };
  }
  if ("QueueReordered" in delta && delta.QueueReordered !== undefined) {
    return {
      ...state,
      queue: reorderedPendingQueue(state.queue, delta.QueueReordered.pending_order),
    };
  }
  if ("QueueRetried" in delta && delta.QueueRetried !== undefined) {
    const { item_id, operation, intent, output_target, overwrite } = delta.QueueRetried;
    const source = state.queue.findIndex((item) => item.id === item_id);
    if (source === -1) {
      return state;
    }
    const next = [...state.queue];
    const [item] = next.splice(source, 1);
    next.push({ ...item, operation, intent, output_target, overwrite, state: "Queued" });
    return { ...state, queue: next };
  }
  if ("QueueEdited" in delta && delta.QueueEdited !== undefined) {
    const { item_id, operation, intent, output_target, overwrite } = delta.QueueEdited;
    return {
      ...state,
      queue: state.queue.map((item) =>
        item.id === item_id ? { ...item, operation, intent, output_target, overwrite } : item,
      ),
    };
  }
  if ("ItemReserved" in delta && delta.ItemReserved !== undefined) {
    const { job } = delta.ItemReserved;
    return {
      ...state,
      queue: withItemState(state.queue, job.item_id, {
        Reserved: { claim_id: job.claim_id, run_id: job.run_id },
      }),
    };
  }
  if ("MediaObserved" in delta && delta.MediaObserved !== undefined) {
    return foldMediaObserved(state, delta.MediaObserved.observation);
  }
  if ("ItemPrepared" in delta && delta.ItemPrepared !== undefined) {
    const { spec } = delta.ItemPrepared;
    return {
      ...state,
      queue: withItemState(state.queue, spec.item_id, {
        Claimed: { claim_id: spec.claim_id, run_id: spec.run_id },
      }),
      conversion_runs: {
        ...state.conversion_runs,
        [spec.run_id]: {
          spec,
          analysis: selectedAnalysis(spec.action),
          output_content_key: null,
          outcome: null,
          started_at: null,
          finished_at: null,
          phase_spans: [],
        },
      },
    };
  }
  if ("ItemRunning" in delta && delta.ItemRunning !== undefined) {
    const { item_id, claim_id, run_id, at } = delta.ItemRunning;
    const run = state.conversion_runs[run_id];
    return {
      ...state,
      queue: withItemState(state.queue, item_id, { Running: { claim_id, run_id } }),
      conversion_runs:
        run === undefined
          ? state.conversion_runs
          : { ...state.conversion_runs, [run_id]: { ...run, started_at: at } },
    };
  }
  if ("AnalysisRecorded" in delta && delta.AnalysisRecorded !== undefined) {
    return foldAnalysisRecorded(
      state,
      delta.AnalysisRecorded.run_id,
      delta.AnalysisRecorded.result,
    );
  }
  if ("ItemFinished" in delta && delta.ItemFinished !== undefined) {
    return foldItemFinished(state, delta.ItemFinished);
  }
  if ("Output" in delta && delta.Output !== undefined) {
    return { ...state, outputs: foldOutput(state.outputs, delta.Output) };
  }
  if ("HistoryImported" in delta && delta.HistoryImported !== undefined) {
    const parked = { ...state.parked };
    for (const [importPath, record] of delta.HistoryImported.records) {
      parked[importPath] = record;
    }
    return { ...state, parked };
  }
  if ("ParkedAdopted" in delta && delta.ParkedAdopted !== undefined) {
    const { import_path, content_key, imported, verdict } = delta.ParkedAdopted;
    const parked = { ...state.parked };
    delete parked[import_path];
    const adoptedImports = Array.from(new Set([...state.adopted_imports, import_path])).sort();
    let records = state.records;
    const record = state.records[content_key];
    if (record !== undefined) {
      const candidate: ImportedProvenance = { import_path, record: imported };
      records = {
        ...state.records,
        [content_key]: {
          ...record,
          imported:
            record.imported === null || importedProvenanceOutranks(candidate, record.imported)
              ? candidate
              : record.imported,
          verdict: verdict ?? record.verdict,
        },
      };
    }
    return { ...state, parked, adopted_imports: adoptedImports, records };
  }
  if ("ParkedRetired" in delta && delta.ParkedRetired !== undefined) {
    const parked = { ...state.parked };
    delete parked[delta.ParkedRetired.import_path];
    return { ...state, parked };
  }
  return state;
}

function importedProvenanceOutranks(
  candidate: ImportedProvenance,
  current: ImportedProvenance,
): boolean {
  if (candidate.record.decided_at !== current.record.decided_at) {
    return candidate.record.decided_at > current.record.decided_at;
  }
  const candidateRank = parkedStatusRank(candidate.record.status);
  const currentRank = parkedStatusRank(current.record.status);
  return candidateRank !== currentRank
    ? candidateRank > currentRank
    : candidate.import_path < current.import_path;
}

function parkedStatusRank(status: ImportedProvenance["record"]["status"]): number {
  switch (status) {
    case "Scanned":
      return 0;
    case "Analyzed":
      return 1;
    case "NotWorthwhile":
      return 2;
    case "Converted":
      return 3;
  }
}

export function foldConfig(_settings: Settings | null, delta: ConfigDelta): Settings {
  return delta.SettingsChanged.settings;
}

// Mirrors reducer.rs's ephemeral application: SessionChanged replaces the
// session; WorkerCrashed/CommandRejected are notifications, not state.
export function foldSession(session: SessionState, delta: EphemeralDelta): SessionState {
  if ("SessionChanged" in delta && delta.SessionChanged !== undefined) {
    return delta.SessionChanged;
  }
  return session;
}

export function foldTelemetry(
  telemetry: Record<RunId, Telemetry>,
  delta: EphemeralDelta,
): Record<RunId, Telemetry> {
  if ("Telemetry" in delta && delta.Telemetry !== undefined) {
    return { ...telemetry, [delta.Telemetry.run_id]: delta.Telemetry };
  }
  if ("TelemetryCleared" in delta && delta.TelemetryCleared !== undefined) {
    const next = { ...telemetry };
    delete next[delta.TelemetryCleared.run_id];
    return next;
  }
  return telemetry;
}

function movedQueue(
  queue: QueueItem[],
  itemId: QueueItemId,
  before: QueueItemId | null,
): QueueItem[] {
  const source = queue.findIndex((item) => item.id === itemId);
  if (source === -1) {
    return queue;
  }
  const next = [...queue];
  const [item] = next.splice(source, 1);
  const target = before === null ? -1 : next.findIndex((entry) => entry.id === before);
  next.splice(target === -1 ? next.length : target, 0, item);
  return next;
}

function reorderedPendingQueue(queue: QueueItem[], pendingOrder: QueueItemId[]): QueueItem[] {
  const frozen = queue.filter((item) => item.state !== "Queued");
  const pending = queue.filter((item) => item.state === "Queued");
  const reordered: QueueItem[] = [];
  for (const itemId of pendingOrder) {
    const source = pending.findIndex((item) => item.id === itemId);
    if (source !== -1) {
      const [item] = pending.splice(source, 1);
      reordered.push(item);
    }
  }
  return [...frozen, ...reordered, ...pending];
}

function withItemState(
  queue: QueueItem[],
  itemId: QueueItemId,
  itemState: QueueItemState,
): QueueItem[] {
  return queue.map((item) => (item.id === itemId ? { ...item, state: itemState } : item));
}

function foldMediaObserved(
  state: DurableState_Deserialize,
  observation: MediaObservation,
): DurableState_Deserialize {
  const { path_hash, binding, metadata } = observation;
  const existing = state.records[binding.content_key];
  const record: FileRecord_Deserialize =
    existing === undefined
      ? { metadata, analyses: [], verdict: null, imported: null }
      : { ...existing, metadata };
  return {
    ...state,
    paths: { ...state.paths, [path_hash]: binding },
    records: { ...state.records, [binding.content_key]: record },
  };
}

function foldAnalysisRecorded(
  state: DurableState_Deserialize,
  runId: RunId,
  result: AnalysisResult,
): DurableState_Deserialize {
  const run = state.conversion_runs[runId];
  if (run === undefined) {
    return state;
  }
  let records = state.records;
  const contentKey = run.spec.content_key;
  if (contentKey !== null) {
    const record = state.records[contentKey];
    if (record !== undefined) {
      records = { ...state.records, [contentKey]: withAnalysis(record, result) };
    }
  }
  return {
    ...state,
    records,
    conversion_runs: { ...state.conversion_runs, [runId]: { ...run, analysis: result } },
  };
}

function foldItemFinished(
  state: DurableState_Deserialize,
  finished: {
    item_id: QueueItemId;
    run_id: RunId;
    outcome: ItemOutcome;
    at: UnixMillis;
    phase_spans: PhaseSpan[];
  },
): DurableState_Deserialize {
  const { item_id, run_id, outcome, at, phase_spans } = finished;
  const queue = withItemState(state.queue, item_id, { Finished: outcome });
  const run = state.conversion_runs[run_id];
  if (run === undefined) {
    return { ...state, queue };
  }
  let outputContentKey = run.output_content_key;
  const successful =
    typeof outcome === "object" &&
    (("Converted" in outcome && outcome.Converted !== undefined) ||
      ("Remuxed" in outcome && outcome.Remuxed !== undefined));
  if (successful) {
    const transaction = state.outputs[run_id];
    if (transaction !== undefined) {
      outputContentKey = committedContentKey(transaction.state);
    }
  }
  // Decisive outcomes upsert the record's verdict; the latest run wins
  // because deltas fold in order. A success with no settled output content
  // key sets no verdict, and the verdict absorbs the measured summary
  // (mirrors the Rust fold).
  let records = state.records;
  const kind = verdictKind(
    outcome,
    outputContentKey,
    run.spec.execution,
    run.analysis,
    phase_spans,
  );
  const contentKey = run.spec.content_key;
  if (kind !== null && contentKey !== null) {
    const record = state.records[contentKey];
    if (record !== undefined) {
      records = {
        ...state.records,
        [contentKey]: {
          ...record,
          verdict: { kind, source_run: run_id, decided_at: at },
        },
      };
    }
  }
  return {
    ...state,
    queue,
    records,
    conversion_runs: {
      ...state.conversion_runs,
      [run_id]: {
        ...run,
        output_content_key: outputContentKey,
        outcome,
        finished_at: at,
        phase_spans,
      },
    },
  };
}

function verdictKind(
  outcome: ItemOutcome,
  outputContentKey: string | null,
  execution: ExecutionSettings,
  analysis: AnalysisResult | null,
  phaseSpans: PhaseSpan[],
): VerdictKind | null {
  if (typeof outcome !== "object") {
    return null;
  }
  if ("Converted" in outcome && outcome.Converted !== undefined) {
    if (outputContentKey === null) {
      return null;
    }
    const [input_size, output_size] = evidenceSizes(outcome.Converted);
    return {
      Converted: {
        output_content_key: outputContentKey,
        input_size,
        output_size,
        encoding_time: encodingDuration(phaseSpans),
        crf: analysis === null ? null : analysis.measurement.crf,
        vmaf: analysis === null ? null : analysis.measurement.score,
        target: analysis === null ? null : analysis.successful_target,
      },
    };
  }
  if ("Remuxed" in outcome && outcome.Remuxed !== undefined) {
    if (outputContentKey === null) {
      return null;
    }
    const [input_size, output_size] = evidenceSizes(outcome.Remuxed);
    return { Remuxed: { output_content_key: outputContentKey, input_size, output_size } };
  }
  if ("NotWorthwhile" in outcome && outcome.NotWorthwhile !== undefined) {
    return {
      NotWorthwhile: {
        requested: execution.requested_target,
        floor: execution.fallback_floor,
      },
    };
  }
  return null;
}

// Measured byte sizes from live evidence; a crash-recovered success carries
// none (mirrors evidence_sizes in the Rust fold).
function evidenceSizes(evidence: CompletionEvidence): [number | null, number | null] {
  if (typeof evidence === "object") {
    if ("LiveEncode" in evidence && evidence.LiveEncode !== undefined) {
      return [evidence.LiveEncode.input_size, evidence.LiveEncode.output_size];
    }
    if ("LiveRemux" in evidence && evidence.LiveRemux !== undefined) {
      return [evidence.LiveRemux.input_size, evidence.LiveRemux.output_size];
    }
  }
  return [null, null];
}

// Total measured encoding time across the run's phase spans; null when no
// encoding phase was measured (mirrors encoding_duration in the Rust fold).
function encodingDuration(spans: PhaseSpan[]): number | null {
  let measured = false;
  let total = 0;
  for (const span of spans) {
    if (span.phase === "Encoding") {
      measured = true;
      total += span.duration;
    }
  }
  return measured ? total : null;
}

function committedContentKey(state: OutputState): string | null {
  if (typeof state === "object") {
    if ("Committed" in state && state.Committed !== undefined) {
      return state.Committed.final_identity.content_key;
    }
    if ("Retired" in state && state.Retired !== undefined) {
      return state.Retired.final_identity.content_key;
    }
  }
  return null;
}

function foldOutput(
  outputs: Record<RunId, OutputTransaction>,
  delta: OutputDelta,
): Record<RunId, OutputTransaction> {
  if ("OutputStarted" in delta && delta.OutputStarted !== undefined) {
    const { transaction } = delta.OutputStarted;
    return { ...outputs, [transaction.run_id]: transaction };
  }
  if ("StagingCreated" in delta && delta.StagingCreated !== undefined) {
    const { run_id, initial } = delta.StagingCreated;
    return withOutputState(outputs, run_id, { StagingCreated: { initial } });
  }
  if ("OutputReady" in delta && delta.OutputReady !== undefined) {
    const { run_id, staging_identity } = delta.OutputReady;
    return withOutputState(outputs, run_id, { Ready: { staging_identity } });
  }
  if ("OutputCommitted" in delta && delta.OutputCommitted !== undefined) {
    const { run_id, final_identity } = delta.OutputCommitted;
    return withOutputState(outputs, run_id, { Committed: { final_identity } });
  }
  if ("RetireOriginalIntent" in delta && delta.RetireOriginalIntent !== undefined) {
    const { run_id } = delta.RetireOriginalIntent;
    const state = outputs[run_id]?.state;
    if (
      state !== undefined &&
      typeof state === "object" &&
      "Committed" in state &&
      state.Committed !== undefined
    ) {
      return withOutputState(outputs, run_id, {
        RetireIntent: { final_identity: state.Committed.final_identity },
      });
    }
    return outputs;
  }
  if ("OriginalRetired" in delta && delta.OriginalRetired !== undefined) {
    const { run_id } = delta.OriginalRetired;
    const state = outputs[run_id]?.state;
    if (
      state !== undefined &&
      typeof state === "object" &&
      "RetireIntent" in state &&
      state.RetireIntent !== undefined
    ) {
      return withOutputState(outputs, run_id, {
        Retired: { final_identity: state.RetireIntent.final_identity },
      });
    }
    return outputs;
  }
  if ("AbandonStagingIntent" in delta && delta.AbandonStagingIntent !== undefined) {
    const { run_id, staging_identity } = delta.AbandonStagingIntent;
    return withOutputState(outputs, run_id, { AbandonIntent: { staging_identity } });
  }
  if ("Abandoned" in delta && delta.Abandoned !== undefined) {
    return withOutputState(outputs, delta.Abandoned.run_id, "Abandoned");
  }
  if ("Conflict" in delta && delta.Conflict !== undefined) {
    const { run_id, kind, detail } = delta.Conflict;
    return withOutputState(outputs, run_id, { Conflict: { kind, detail } });
  }
  return outputs;
}

function withOutputState(
  outputs: Record<RunId, OutputTransaction>,
  runId: RunId,
  state: OutputState,
): Record<RunId, OutputTransaction> {
  const transaction = outputs[runId];
  if (transaction === undefined) {
    return outputs;
  }
  return { ...outputs, [runId]: { ...transaction, state } };
}

function selectedAnalysis(action: JobAction): AnalysisResult | null {
  if (typeof action === "object") {
    if ("Analyze" in action && action.Analyze !== undefined) {
      return action.Analyze.selected_analysis;
    }
    if ("Encode" in action && action.Encode !== undefined) {
      return action.Encode.selected_analysis;
    }
  }
  return null;
}

// Rust stores analyses in a BTreeMap sorted by AnalysisProfile's Ord; the wire
// carries them as an entry list. This mirror appends newly seen profiles at
// the end instead of reproducing Rust's sort order — entry order is not
// semantic (lookups scan by profile equality), and the fixture test compares
// analyses order-insensitively for exactly this reason.
function withAnalysis(
  record: FileRecord_Deserialize,
  result: AnalysisResult,
): FileRecord_Deserialize {
  const index = record.analyses.findIndex(([profile]) => profileEquals(profile, result.profile));
  if (index === -1) {
    return {
      ...record,
      analyses: [...record.analyses, [result.profile, { [result.successful_target]: result }]],
    };
  }
  return {
    ...record,
    analyses: record.analyses.map((entry, position) =>
      position === index ? [entry[0], { ...entry[1], [result.successful_target]: result }] : entry,
    ),
  };
}

function profileEquals(a: AnalysisProfile, b: AnalysisProfile): boolean {
  return (
    a.preset === b.preset &&
    a.max_encoded_percent_basis_points === b.max_encoded_percent_basis_points &&
    a.samples === b.samples &&
    a.sample_duration_ms === b.sample_duration_ms &&
    a.thorough === b.thorough &&
    decodeModeEquals(a.decode_mode, b.decode_mode) &&
    a.ab_av1_revision === b.ab_av1_revision &&
    a.ffmpeg_revision === b.ffmpeg_revision &&
    a.encoder_revision === b.encoder_revision
  );
}

function decodeModeEquals(a: DecodeMode, b: DecodeMode): boolean {
  if (typeof a === "string" || typeof b === "string") {
    return a === b;
  }
  return a.Hardware === b.Hardware;
}
