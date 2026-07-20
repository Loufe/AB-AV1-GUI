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
  ConfigDelta,
  DecodeMode,
  DurableDelta,
  DurableState_Deserialize,
  EphemeralDelta,
  FileRecord_Deserialize,
  ItemOutcome,
  JobAction,
  MediaObservation,
  OutputDelta,
  OutputState,
  OutputTransaction,
  QueueItem,
  QueueItemId,
  QueueItemState,
  RunId,
  SessionState,
  Settings,
  Telemetry,
} from "@/lib/bindings";

export function emptyDurableState(): DurableState_Deserialize {
  return { queue: [], paths: {}, records: {}, outputs: {}, conversion_runs: {} };
}

export function foldDurable(
  state: DurableState_Deserialize,
  delta: DurableDelta,
): DurableState_Deserialize {
  if ("QueueAdded" in delta && delta.QueueAdded !== undefined) {
    return { ...state, queue: [...state.queue, delta.QueueAdded.item] };
  }
  if ("QueueRemoved" in delta && delta.QueueRemoved !== undefined) {
    const { item_id } = delta.QueueRemoved;
    return { ...state, queue: state.queue.filter((item) => item.id !== item_id) };
  }
  if ("QueueMoved" in delta && delta.QueueMoved !== undefined) {
    const { item_id, before } = delta.QueueMoved;
    return { ...state, queue: movedQueue(state.queue, item_id, before) };
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
        },
      },
    };
  }
  if ("ItemRunning" in delta && delta.ItemRunning !== undefined) {
    const { item_id, claim_id, run_id } = delta.ItemRunning;
    return {
      ...state,
      queue: withItemState(state.queue, item_id, { Running: { claim_id, run_id } }),
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
  return state;
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
    existing === undefined ? { metadata, analyses: [] } : { ...existing, metadata };
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
  finished: { item_id: QueueItemId; run_id: RunId; outcome: ItemOutcome },
): DurableState_Deserialize {
  const { item_id, run_id, outcome } = finished;
  const queue = withItemState(state.queue, item_id, { Finished: outcome });
  const run = state.conversion_runs[run_id];
  if (run === undefined) {
    return { ...state, queue };
  }
  let outputContentKey = run.output_content_key;
  if (outcome === "Converted" || outcome === "Remuxed") {
    const transaction = state.outputs[run_id];
    if (transaction !== undefined) {
      outputContentKey = committedContentKey(transaction.state);
    }
  }
  return {
    ...state,
    queue,
    conversion_runs: {
      ...state.conversion_runs,
      [run_id]: { ...run, output_content_key: outputContentKey, outcome },
    },
  };
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
    const { run_id, reason } = delta.Conflict;
    return withOutputState(outputs, run_id, { Conflict: { reason } });
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
