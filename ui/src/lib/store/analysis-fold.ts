import type {
  AnalysisDelta_Deserialize,
  AnalysisGeneration_Deserialize,
  AnalysisSnapshot_Deserialize,
} from "@/lib/bindings";
import type { AnalysisStoreState, NormalizedAnalysisGeneration } from "@/lib/store/analysis-store";

function normalizeGeneration(
  generation: AnalysisGeneration_Deserialize,
): NormalizedAnalysisGeneration {
  const rows: NormalizedAnalysisGeneration["rows"] = {};
  for (const row of generation.rows) {
    rows[row.id] = row;
  }
  return { ...generation, rows };
}

export function normalizeAnalysisSnapshot(
  snapshot: AnalysisSnapshot_Deserialize,
): AnalysisStoreState {
  return {
    current: snapshot.current === null ? null : normalizeGeneration(snapshot.current),
  };
}

/**
 * Pure fold for the generated Analysis wire contract. Generation ids are the
 * stale-work boundary: live deltas for any superseded generation are ignored.
 */
export function foldAnalysis(
  state: AnalysisStoreState,
  delta: AnalysisDelta_Deserialize,
): AnalysisStoreState {
  if ("Reset" in delta && delta.Reset !== undefined) {
    return normalizeAnalysisSnapshot(delta.Reset.snapshot);
  }

  const current = state.current;
  if (current === null) {
    return state;
  }

  if ("RowsUpserted" in delta && delta.RowsUpserted !== undefined) {
    if (delta.RowsUpserted.generation !== current.id || delta.RowsUpserted.rows.length === 0) {
      return state;
    }
    const rows = { ...current.rows };
    for (const row of delta.RowsUpserted.rows) {
      rows[row.id] = row;
    }
    return { current: { ...current, rows } };
  }

  if ("ActivityChanged" in delta && delta.ActivityChanged !== undefined) {
    if (delta.ActivityChanged.generation !== current.id) {
      return state;
    }
    return {
      current: { ...current, activity: delta.ActivityChanged.activity },
    };
  }

  return state;
}
