import { Channel } from "@tauri-apps/api/core";

import {
  commands,
  type AnalysisIntent,
  type CommandError,
  type CorruptionSignature,
  type Operation,
  type OutputTarget,
  type QueueItemEdit,
  type QueueItemId,
  type Settings,
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

export async function saveSettings(settings: Settings): Promise<void> {
  const result = await commands.setSettings(settings);
  if (result.status === "error") {
    throw new Error(`settings save failed (${result.error.code}): ${result.error.message}`);
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

export async function queueClearCompleted(): Promise<void> {
  expectAccepted(await commands.queueClearCompleted(), "clear completed");
}

export async function queueRetry(itemId: QueueItemId): Promise<void> {
  expectAccepted(await commands.queueRetry(itemId), "queue retry");
}

export async function queueEdit(itemId: QueueItemId, patch: QueueItemEdit): Promise<void> {
  expectAccepted(await commands.queueEdit(itemId, patch), "queue edit");
}
