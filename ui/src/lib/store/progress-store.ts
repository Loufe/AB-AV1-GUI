// Telemetry lives in its own store so progress ticks touch no tree
// subscription (#33 §11): components rendering queue/analysis trees subscribe
// to the app store only, and per-row progress reads from here. Session
// aggregates ride along: they update at item-finish frequency and belong to
// the same live-run display surface.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { RunId, SessionAggregates, Telemetry } from "@/lib/bindings";

export interface ProgressStoreState {
  telemetry: Record<RunId, Telemetry>;
  /** Latest per-session totals; the reducer zeroes them at session start. */
  aggregates: SessionAggregates;
}

export function emptySessionAggregates(): SessionAggregates {
  return {
    completed: 0,
    failed: 0,
    skipped: 0,
    stopped: 0,
    not_worthwhile: 0,
    analyzed: 0,
    remuxed: 0,
    input_bytes: 0,
    output_bytes: 0,
    encode_duration_ms: 0,
  };
}

export const progressStore = createStore<ProgressStoreState>(() => ({
  telemetry: {},
  aggregates: emptySessionAggregates(),
}));

export function useProgressStore<T>(selector: (state: ProgressStoreState) => T): T {
  return useStore(progressStore, selector);
}
