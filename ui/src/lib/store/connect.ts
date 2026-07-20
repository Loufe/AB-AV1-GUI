// The single stream consumer: subscribes to the shell's ordered event channel
// and routes every payload into the stores through the pure folds. The shell
// replays Snapshot → SessionChanged → standing health on every subscribe
// (ADR-006), so reconnecting is the one and only resync primitive.

import { toast } from "sonner";

import type { ShellEvent_Deserialize, StreamPayload_Deserialize } from "@/lib/bindings";
import { subscribeStream } from "@/lib/ipc";
import { appStore } from "@/lib/store/app-store";
import { foldConfig, foldDurable, foldSession, foldTelemetry } from "@/lib/store/fold";
import { progressStore } from "@/lib/store/progress-store";

// The transport is ordered by construction; seq is a tripwire, not a recovery
// protocol (#33 §11). Each connection's numbering starts at 0, so a fresh
// subscription is recognized by seq 0 rather than continuity with the last.
export function hasSequenceGap(last: number | null, next: number): boolean {
  return next !== 0 && next !== (last === null ? 0 : last + 1);
}

export function applyPayload(payload: StreamPayload_Deserialize): void {
  if ("Snapshot" in payload && payload.Snapshot !== undefined) {
    const { durable, settings } = payload.Snapshot;
    appStore.setState((state) => ({
      ...state,
      durable,
      settings,
      health: { degraded: null, fatal: null },
      tools: null,
    }));
    // Telemetry for pre-snapshot runs never gets a TelemetryCleared on this
    // connection; the snapshot is the fresh baseline.
    progressStore.setState({ telemetry: {} });
    return;
  }
  if ("Durable" in payload && payload.Durable !== undefined) {
    const delta = payload.Durable;
    appStore.setState((state) => ({ ...state, durable: foldDurable(state.durable, delta) }));
    return;
  }
  if ("Config" in payload && payload.Config !== undefined) {
    const delta = payload.Config;
    appStore.setState((state) => ({ ...state, settings: foldConfig(state.settings, delta) }));
    return;
  }
  if ("Ephemeral" in payload && payload.Ephemeral !== undefined) {
    const delta = payload.Ephemeral;
    if ("SessionChanged" in delta && delta.SessionChanged !== undefined) {
      appStore.setState((state) => ({ ...state, session: foldSession(state.session, delta) }));
      return;
    }
    if ("WorkerCrashed" in delta && delta.WorkerCrashed !== undefined) {
      toast.error(`Worker crashed: ${delta.WorkerCrashed.message}`);
      return;
    }
    if ("CommandRejected" in delta && delta.CommandRejected !== undefined) {
      // Command results already surface rejections at the call site; this is
      // the observability backstop, not a user-facing notification (#33 §11).
      console.warn("command rejected by the engine", delta.CommandRejected.reason);
      return;
    }
    if ("ToolsChanged" in delta && delta.ToolsChanged !== undefined) {
      const tools = delta.ToolsChanged;
      appStore.setState((state) => ({ ...state, tools }));
      return;
    }
    progressStore.setState((state) => ({ telemetry: foldTelemetry(state.telemetry, delta) }));
    return;
  }
  if ("Degraded" in payload && payload.Degraded !== undefined) {
    const { reason } = payload.Degraded;
    appStore.setState((state) => ({ ...state, health: { ...state.health, degraded: reason } }));
    return;
  }
  if ("EngineFatal" in payload && payload.EngineFatal !== undefined) {
    const { message } = payload.EngineFatal;
    appStore.setState((state) => ({ ...state, health: { ...state.health, fatal: message } }));
  }
}

let started = false;
let currentConnection = 0;

/**
 * Connects the stores to the shell stream. Idempotent: React StrictMode's
 * double-mount and repeated calls reuse the first connection. A detected
 * sequence gap re-subscribes; the replayed snapshot supersedes anything the
 * gap lost.
 */
export function connectStream(): void {
  if (started) {
    return;
  }
  started = true;
  void connect();
}

async function connect(): Promise<void> {
  const connection = ++currentConnection;
  let lastSeq: number | null = null;
  try {
    await subscribeStream((event: ShellEvent_Deserialize) => {
      if (connection !== currentConnection) {
        // A stale channel the shell has already replaced; a fresh
        // subscription's snapshot supersedes anything it still delivers.
        return;
      }
      if (hasSequenceGap(lastSeq, event.seq)) {
        console.error(
          `stream sequence gap: expected ${lastSeq === null ? 0 : lastSeq + 1}, got ${event.seq}; resubscribing`,
        );
        void connect();
        return;
      }
      lastSeq = event.seq;
      applyPayload(event.payload);
    });
  } catch (error: unknown) {
    console.error("failed to subscribe to the shell event stream", error);
  }
}
