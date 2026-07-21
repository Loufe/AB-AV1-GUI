// Analysis is standing, non-durable state. The shell replays one complete
// Reset after every durable snapshot; subsequent row batches are folded into
// this normalized store without making the application store's queue/history
// subscribers rerender.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AnalysisGeneration_Deserialize, AnalysisRow, AnalysisRowId } from "@/lib/bindings";

export interface NormalizedAnalysisGeneration extends Omit<AnalysisGeneration_Deserialize, "rows"> {
  /** Rows keyed by their generation-local id for direct replacement/lookups. */
  rows: Partial<Record<AnalysisRowId, AnalysisRow>>;
}

export interface AnalysisStoreState {
  current: NormalizedAnalysisGeneration | null;
}

export function emptyAnalysisState(): AnalysisStoreState {
  return { current: null };
}

export const analysisStore = createStore<AnalysisStoreState>(emptyAnalysisState);

export function useAnalysisStore<T>(selector: (state: AnalysisStoreState) => T): T {
  return useStore(analysisStore, selector);
}
