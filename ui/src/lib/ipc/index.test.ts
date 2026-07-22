import { beforeEach, describe, expect, it, vi } from "vitest";

import { commands, type CorruptionSignature } from "@/lib/bindings";

import { acknowledgeCorruption, queueRemoveMany, queueRetry, startQueue } from "./index";
import { pickPaths } from "./path-picker";
import { importHistory } from "./settings";

vi.mock("@tauri-apps/api/core", () => ({ Channel: class {} }));
vi.mock("@/lib/bindings", () => ({
  commands: {
    acknowledgeCorruption: vi.fn(),
    importHistory: vi.fn(),
    pickPaths: vi.fn(),
    queueRemoveMany: vi.fn(),
    queueRetry: vi.fn(),
    start: vi.fn(),
  },
}));

const acknowledged = vi.mocked(commands.acknowledgeCorruption);
const imported = vi.mocked(commands.importHistory);
const picked = vi.mocked(commands.pickPaths);
const removed = vi.mocked(commands.queueRemoveMany);
const retried = vi.mocked(commands.queueRetry);
const started = vi.mocked(commands.start);

function signature(): CorruptionSignature {
  return { tail_len: 24, digest: "ab12cd34" };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("acknowledgeCorruption", () => {
  it("echoes the observed signature verbatim", async () => {
    acknowledged.mockResolvedValue({ status: "ok", data: null });
    const observed = signature();
    await acknowledgeCorruption(observed);
    expect(acknowledged).toHaveBeenCalledExactlyOnceWith(observed);
  });

  it("surfaces an engine rejection as an error", async () => {
    acknowledged.mockResolvedValue({
      status: "error",
      error: { code: "rejected", message: "corruption signature does not match" },
    });
    await expect(acknowledgeCorruption(signature())).rejects.toThrow(
      "corruption acknowledgement failed (rejected): corruption signature does not match",
    );
  });
});

describe("importHistory", () => {
  it("passes the path through and returns the summary", async () => {
    imported.mockResolvedValue({ status: "ok", data: { parked: 3, skipped: 1 } });
    const summary = await importHistory("C:\\exports\\history-v3.json");
    expect(imported).toHaveBeenCalledExactlyOnceWith("C:\\exports\\history-v3.json");
    expect(summary).toEqual({ parked: 3, skipped: 1 });
  });

  it("surfaces an engine rejection as an error", async () => {
    imported.mockResolvedValue({
      status: "error",
      error: { code: "import_failed", message: "unsupported import file version 2" },
    });
    await expect(importHistory("history.json")).rejects.toThrow(
      "history import failed (import_failed): unsupported import file version 2",
    );
  });
});

describe("Queue command wrappers", () => {
  it("passes picker intent and start directory through, including cancellation", async () => {
    picked.mockResolvedValue({ status: "ok", data: [] });

    await expect(pickPaths("Files", "C:\\Videos")).resolves.toEqual([]);
    expect(picked).toHaveBeenCalledExactlyOnceWith("Files", "C:\\Videos");
  });

  it("passes the complete QueueItemId set to atomic removal", async () => {
    removed.mockResolvedValue({ status: "ok", data: null });

    await queueRemoveMany([42, 7]);

    expect(removed).toHaveBeenCalledExactlyOnceWith([42, 7]);
  });

  it("distinguishes plain retry from an atomic patched retry", async () => {
    retried.mockResolvedValue({ status: "ok", data: null });
    const patch = {
      operation: "Convert" as const,
      intent: "Refresh" as const,
      output_target: null,
      overwrite: null,
    };

    await queueRetry(42, null);
    await queueRetry(7, patch);

    expect(retried).toHaveBeenNthCalledWith(1, 42, null);
    expect(retried).toHaveBeenNthCalledWith(2, 7, patch);
  });

  it("surfaces a start rejection without changing frontend state", async () => {
    started.mockResolvedValue({
      status: "error",
      error: { code: "rejected", message: "media tools are unavailable" },
    });

    await expect(startQueue()).rejects.toThrow(
      "queue start failed (rejected): media tools are unavailable",
    );
  });
});
