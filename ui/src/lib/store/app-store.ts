// Zustand is only the container (#36 D5): state is written from outside React
// by the stream consumer in connect.ts, and no reduce logic lives in a store
// action — deltas apply through the pure functions in fold.ts.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { DurableState_Deserialize, SessionState, Settings } from "@/lib/bindings";
import { emptyDurableState } from "@/lib/store/fold";

/** Standing engine health from the stream; cleared by each snapshot. */
export interface Health {
  degraded: string | null;
  fatal: string | null;
}

export interface AppStoreState {
  durable: DurableState_Deserialize;
  /** Null until the first snapshot arrives. */
  settings: Settings | null;
  session: SessionState;
  health: Health;
}

export function initialAppState(): AppStoreState {
  return {
    durable: emptyDurableState(),
    settings: null,
    session: "Idle",
    health: { degraded: null, fatal: null },
  };
}

export const appStore = createStore<AppStoreState>(initialAppState);

export function useAppStore<T>(selector: (state: AppStoreState) => T): T {
  return useStore(appStore, selector);
}
