import { describe, expect, it } from "vitest";

import type { QueueItemState, Telemetry } from "@/lib/bindings";

import { basename, deriveRowStatus, outputTargetLabel } from "./queue-status";

const RUNNING: QueueItemState = { Running: { claim_id: 1, run_id: 1 } };

function telemetry(progress: Telemetry["progress"], phase: Telemetry["phase"]): Telemetry {
  return { run_id: 1, sequence: 1, phase, progress };
}

describe("basename", () => {
  it("takes the last segment of unix paths", () => {
    expect(basename("/videos/season 1/e01.mkv")).toBe("e01.mkv");
  });
  it("takes the last segment of windows paths", () => {
    expect(basename("C:\\Videos\\e01.mkv")).toBe("e01.mkv");
  });
  it("tolerates trailing separators and bare names", () => {
    expect(basename("/videos/folder/")).toBe("folder");
    expect(basename("e01.mkv")).toBe("e01.mkv");
  });
});

describe("deriveRowStatus", () => {
  it("maps pre-run states", () => {
    expect(deriveRowStatus("Queued", null, null, null)).toEqual({ kind: "queued" });
    expect(deriveRowStatus({ Reserved: { claim_id: 1, run_id: 1 } }, null, null, null)).toEqual({
      kind: "starting",
    });
    expect(deriveRowStatus({ Claimed: { claim_id: 1, run_id: 1 } }, null, null, null)).toEqual({
      kind: "starting",
    });
  });

  it("shows the running phase without telemetry", () => {
    expect(deriveRowStatus(RUNNING, null, null, null)).toEqual({
      kind: "working",
      phase: "Preparing",
      percent: null,
    });
  });

  it("converts search basis points to a percent", () => {
    const status = deriveRowStatus(
      RUNNING,
      telemetry({ SearchBasisPoints: 6250 }, "Analyzing"),
      null,
      null,
    );
    expect(status).toEqual({ kind: "working", phase: "Analyzing", percent: 63 });
  });

  it("derives encode percent from position over duration", () => {
    const status = deriveRowStatus(
      RUNNING,
      telemetry({ OutputPositionMs: 30_000 }, "Encoding"),
      120_000,
      null,
    );
    expect(status).toEqual({ kind: "working", phase: "Encoding", percent: 25 });
  });

  it("leaves encode percent unknown without a duration", () => {
    const status = deriveRowStatus(
      RUNNING,
      telemetry({ OutputPositionMs: 30_000 }, "Encoding"),
      null,
      null,
    );
    expect(status).toEqual({ kind: "working", phase: "Encoding", percent: null });
  });

  it("maps finished outcomes", () => {
    expect(
      deriveRowStatus(
        {
          Finished: {
            Converted: {
              LiveEncode: {
                input_size: 2048,
                output_size: 1536,
                stream_sizes: { video: 1200, audio: 300, subtitle: 6, other: 30 },
                encode_decode: "Software",
              },
            },
          },
        },
        null,
        null,
        512,
      ),
    ).toEqual({
      kind: "done",
      outcome: "Converted",
      savedBytes: 512,
    });
    expect(
      deriveRowStatus({ Finished: { Remuxed: "RecoveredAtStartup" } }, null, null, null),
    ).toEqual({
      kind: "done",
      outcome: "Remuxed",
      savedBytes: null,
    });
    expect(deriveRowStatus({ Finished: "Analyzed" }, null, null, 512)).toEqual({
      kind: "done",
      outcome: "Analyzed",
      savedBytes: null,
    });
    expect(deriveRowStatus({ Finished: "Stopped" }, null, null, null)).toEqual({
      kind: "stopped",
    });
    expect(
      deriveRowStatus(
        { Finished: { Failed: { kind: "EncodeRun", message: "boom", diagnostic: "" } } },
        null,
        null,
        null,
      ),
    ).toEqual({ kind: "failed", message: "boom" });
  });

  it("maps skip reasons to readable text", () => {
    const skipped = deriveRowStatus(
      { Finished: { Skipped: { reason: "AlreadyAv1Matroska" } } },
      null,
      null,
      null,
    );
    expect(skipped.kind).toBe("skipped");
    if (skipped.kind === "skipped") expect(skipped.reason).toBe("already AV1");

    const lowRes = deriveRowStatus(
      { Finished: { Skipped: { reason: { LowResolution: { pixels: 307200, minimum: 921600 } } } } },
      null,
      null,
      null,
    );
    if (lowRes.kind === "skipped") expect(lowRes.reason).toBe("below minimum resolution");
  });

  it("summarizes the best not-worthwhile attempt", () => {
    const status = deriveRowStatus(
      {
        Finished: {
          NotWorthwhile: {
            attempts: [
              {
                target: 90,
                last_measurement: {
                  crf: 30,
                  score: 89.4,
                  predicted_size: 900,
                  predicted_percent_basis_points: 9700,
                  predicted_duration_ms: 1000,
                  from_cache: false,
                },
              },
            ],
          },
        },
      },
      null,
      null,
      null,
    );
    expect(status).toEqual({
      kind: "skipped",
      reason: "not worthwhile",
      detail: "Best attempt saved 3% at the VMAF 90 floor",
    });
  });
});

describe("outputTargetLabel", () => {
  it("hides output for analyze items", () => {
    expect(outputTargetLabel("Analyze", "Replace")).toBe("—");
  });
  it("labels each target kind", () => {
    expect(outputTargetLabel("Convert", "Replace")).toBe("Replace");
    expect(outputTargetLabel("Convert", { Suffix: { suffix: "_av1" } })).toBe("Suffix _av1");
    expect(
      outputTargetLabel("Convert", {
        SeparateFolder: { directory: "/out/converted", source_root: null },
      }),
    ).toBe("converted");
  });
});
