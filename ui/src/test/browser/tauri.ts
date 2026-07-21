import { Channel, type InvokeArgs } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";

import type {
  CommandError,
  ShellEvent,
  ShellEvent_Deserialize,
  StreamPayload_Deserialize,
} from "@/lib/bindings";

export interface TauriCall {
  command: string;
  payload: InvokeArgs | undefined;
}

export type TauriCommandHandler = (payload: InvokeArgs | undefined) => unknown;

function channelFrom(payload: InvokeArgs | undefined): Channel<ShellEvent> | null {
  if (
    payload === undefined ||
    Array.isArray(payload) ||
    payload instanceof ArrayBuffer ||
    ArrayBuffer.isView(payload)
  ) {
    return null;
  }
  const candidate = payload.channel;
  return candidate instanceof Channel ? candidate : null;
}

/**
 * Real Tauri-JS IPC interception below the generated bindings. Product
 * wrappers still call generated `commands`; only the native endpoint is fake.
 */
export class TauriMock {
  readonly calls: TauriCall[] = [];

  private readonly handlers = new Map<string, TauriCommandHandler>();
  private stream: Channel<ShellEvent> | null = null;
  private nextSequence = 0;

  constructor(handlers: Record<string, TauriCommandHandler>) {
    Object.entries(handlers).forEach(([command, handler]) => {
      this.handlers.set(command, handler);
    });
  }

  handle = (command: string, payload?: InvokeArgs): unknown => {
    this.calls.push({ command, payload });

    if (command === "subscribe") {
      this.stream = channelFrom(payload);
      if (this.stream === null) {
        return Promise.reject(new Error("subscribe did not receive a Tauri channel"));
      }
      this.nextSequence = 0;
      return null;
    }

    const handler = this.handlers.get(command);
    if (handler === undefined) {
      return Promise.reject(new Error(`Unhandled Tauri command in browser test: ${command}`));
    }
    return handler(payload);
  };

  setCommand(command: string, handler: TauriCommandHandler): void {
    this.handlers.set(command, handler);
  }

  acceptCommand(command: string, data: unknown = null): void {
    this.setCommand(command, () => data);
  }

  rejectCommand(command: string, error: CommandError): void {
    this.setCommand(command, () => Promise.reject(error));
  }

  callsFor(command: string): TauriCall[] {
    return this.calls.filter((call) => call.command === command);
  }

  emit(payload: StreamPayload_Deserialize): ShellEvent_Deserialize {
    return this.emitAt(this.nextSequence, payload);
  }

  emitAt(sequence: number, payload: StreamPayload_Deserialize): ShellEvent_Deserialize {
    if (this.stream === null) {
      throw new Error("subscribeStream must complete before emitting a shell event");
    }
    const event = { seq: sequence, payload } satisfies ShellEvent_Deserialize;
    this.nextSequence = sequence + 1;
    this.stream.onmessage(event);
    return event;
  }
}

export function installTauriMock(handlers: Record<string, TauriCommandHandler> = {}): TauriMock {
  const tauri = new TauriMock(handlers);
  mockIPC(tauri.handle);
  return tauri;
}
