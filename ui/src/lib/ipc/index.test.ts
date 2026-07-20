import { beforeEach, describe, expect, it, vi } from "vitest";

import { commands, type CorruptionSignature } from "@/lib/bindings";

import { acknowledgeCorruption } from "./index";

vi.mock("@tauri-apps/api/core", () => ({ Channel: class {} }));
vi.mock("@/lib/bindings", () => ({
  commands: { acknowledgeCorruption: vi.fn() },
}));

const acknowledged = vi.mocked(commands.acknowledgeCorruption);

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
