// Telemetry lives in its own store so progress ticks touch no tree
// subscription (#33 §11): components rendering queue/analysis trees subscribe
// to the app store only, and per-row progress reads from here.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { RunId, Telemetry } from "@/lib/bindings";

export interface ProgressStoreState {
  telemetry: Record<RunId, Telemetry>;
}

export const progressStore = createStore<ProgressStoreState>(() => ({ telemetry: {} }));

export function useProgressStore<T>(selector: (state: ProgressStoreState) => T): T {
  return useStore(progressStore, selector);
}
