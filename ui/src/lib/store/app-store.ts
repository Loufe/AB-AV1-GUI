// Zustand is only the container (#36 D5): state is written from outside React
// by the stream consumer in connect.ts, and no reduce logic lives in a store
// action — deltas apply through the pure functions in fold.ts.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type {
  CorruptionReport,
  DurableState_Deserialize,
  SessionState,
  Settings,
  StatisticsPayload,
  ToolsState,
} from "@/lib/bindings";
import { emptyDurableState } from "@/lib/store/fold";

/** Standing engine health from the stream; cleared by each snapshot. */
export interface Health {
  /**
   * Journal corruption report while mutation is rejected; its signature is
   * what an acknowledgement must echo back. Null when healthy or recovered.
   */
  degraded: CorruptionReport | null;
  /** Engine never started; no commands can run. */
  unavailable: string | null;
  fatal: string | null;
  /** Lock path held by the running instance when this one is a duplicate. */
  secondInstance: string | null;
}

export interface AppStoreState {
  durable: DurableState_Deserialize;
  /** Null until the first snapshot arrives. */
  settings: Settings | null;
  session: SessionState;
  health: Health;
  /**
   * Standing tool state (availability, vendor activity, update flag); null
   * until the stream delivers it. The shell replays ToolsChanged right after
   * each snapshot (ADR-006 standing health), so the snapshot handler resets
   * this to null rather than guessing.
   */
  tools: ToolsState | null;
  /**
   * Latest Statistics answer. Fire-and-forget on the stream — never replayed
   * on subscribe — so each snapshot resets it to null and the statistics
   * view re-requests when it needs fresh numbers.
   */
  statistics: StatisticsPayload | null;
  /**
   * The shell deferred a window close because a session is active (#33 §12);
   * the close dialog is open while true.
   */
  closeRequested: boolean;
  /**
   * Quit once the session reaches Idle: set by every close-dialog choice
   * except "keep converting". App re-issues the window close, which the
   * shell then lets through.
   */
  quitAfterSession: boolean;
}

export function initialAppState(): AppStoreState {
  return {
    durable: emptyDurableState(),
    settings: null,
    session: "Idle",
    health: { degraded: null, unavailable: null, fatal: null, secondInstance: null },
    tools: null,
    statistics: null,
    closeRequested: false,
    quitAfterSession: false,
  };
}

export const appStore = createStore<AppStoreState>(initialAppState);

export function useAppStore<T>(selector: (state: AppStoreState) => T): T {
  return useStore(appStore, selector);
}
