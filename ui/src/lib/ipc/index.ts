import { Channel } from "@tauri-apps/api/core";

import { commands, type ShellEvent } from "@/lib/bindings";

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
