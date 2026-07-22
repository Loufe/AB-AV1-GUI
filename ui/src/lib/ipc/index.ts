import { Channel } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  commands,
  type AnalysisIntent,
  type CommandError,
  type CorruptionSignature,
  type Operation,
  type OutputTarget,
  type QueueItemEdit,
  type QueueItemId,
  type ShellEvent,
} from "@/lib/bindings";

/** True inside the Tauri webview; false in plain browser dev. */
export function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/**
 * Opens the shell's event stream. The first event is always a snapshot, and
 * ordering is structural: one channel, sequence numbers assigned per
 * connection by the shell forwarder (ADR-006). Re-invoking replaces the
 * previous subscription — there is deliberately no unsubscribe.
 */
export async function subscribeStream(onEvent: (event: ShellEvent) => void): Promise<void> {
  const channel = new Channel<ShellEvent>();
  channel.onmessage = onEvent;
  const result = await commands.subscribe(channel);
  if (result.status === "error") {
    throw new Error(`subscribe failed (${result.error.code}): ${result.error.message}`);
  }
}

export async function fetchAppVersion(): Promise<string> {
  const info = await commands.appInfo();
  return info.version;
}

/**
 * Ask the engine to compute Statistics using the caller's local-calendar
 * offset. This resolves when the command is accepted; the answer arrives
 * later on the sequenced stream and is stored by connect.ts.
 */
export async function requestStatistics(utcOffsetMinutes: number): Promise<void> {
  const result = await commands.requestStatistics(utcOffsetMinutes);
  if (result.status === "error") {
    throw new Error(`statistics request failed (${result.error.code}): ${result.error.message}`);
  }
}

/**
 * Consents to discarding a corrupt journal tail. The signature must be the
 * one observed on the `Degraded` payload, echoed back verbatim — the engine
 * rejects anything else, so a stale acknowledgement can never discard bytes
 * the operator was not shown.
 */
export async function acknowledgeCorruption(signature: CorruptionSignature): Promise<void> {
  const result = await commands.acknowledgeCorruption(signature);
  if (result.status === "error") {
    throw new Error(
      `corruption acknowledgement failed (${result.error.code}): ${result.error.message}`,
    );
  }
}

function expectAccepted(
  result: { status: "ok"; data: null } | { status: "error"; error: CommandError },
  action: string,
): void {
  if (result.status === "error") {
    throw new Error(`${action} failed (${result.error.code}): ${result.error.message}`);
  }
}

/**
 * Adds files and folders in one batch. Folders expand through the engine
 * scanner (filtered by the configured scan extensions); directly selected
 * files pass through unfiltered. The outcome arrives on the event stream as
 * one `QueueAddSummary`.
 */
export async function queueAddPaths(
  inputs: string[],
  operation: Operation,
  intent: AnalysisIntent,
  outputTarget: OutputTarget,
): Promise<void> {
  expectAccepted(
    await commands.queueAddPaths(inputs, operation, intent, outputTarget),
    "queue add",
  );
}

export async function queueClear(): Promise<void> {
  expectAccepted(await commands.queueClear(), "queue clear");
}

export async function queueRemoveMany(itemIds: QueueItemId[]): Promise<void> {
  expectAccepted(await commands.queueRemoveMany(itemIds), "queue remove");
}

export async function queueReorderPending(pendingOrder: QueueItemId[]): Promise<void> {
  expectAccepted(await commands.queueReorderPending(pendingOrder), "queue reorder");
}

export async function queueClearCompleted(): Promise<void> {
  expectAccepted(await commands.queueClearCompleted(), "clear completed");
}

export async function queueRetry(itemId: QueueItemId, patch: QueueItemEdit | null): Promise<void> {
  expectAccepted(await commands.queueRetry(itemId, patch), "queue retry");
}

export async function queueEdit(itemId: QueueItemId, patch: QueueItemEdit): Promise<void> {
  expectAccepted(await commands.queueEdit(itemId, patch), "queue edit");
}

export async function startQueue(): Promise<void> {
  expectAccepted(await commands.start(), "queue start");
}

export async function stopAfterCurrent(): Promise<void> {
  expectAccepted(await commands.stopAfterCurrent(), "stop after current");
}

export async function forceStop(): Promise<void> {
  expectAccepted(await commands.forceStop(), "force stop");
}

/**
 * Re-issues the window close the shell deferred. The shell re-runs its close
 * decision, so this only actually closes once the session is idle (#33 §12).
 */
export async function closeAppWindow(): Promise<void> {
  await getCurrentWindow().close();
}

/** Opens a file or folder with the operating system's default program. */
export async function openPath(path: string): Promise<void> {
  expectAccepted(await commands.openPath(path), "open");
}

/** Reveals a path selected in the system file manager. */
export async function revealInFileManager(path: string): Promise<void> {
  expectAccepted(await commands.revealInFileManager(path), "reveal in file manager");
}
