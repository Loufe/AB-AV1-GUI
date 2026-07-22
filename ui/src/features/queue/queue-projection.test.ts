import { describe, expect, it } from "vitest";

import type {
  AnalysisResult,
  ConversionRun,
  DurableState_Deserialize,
  FileRecord_Deserialize,
  ItemOutcome,
  OutputTransaction_Deserialize,
  QueueItem,
} from "@/lib/bindings";
import { emptyDurableState } from "@/lib/store/fold";

import { queueRows } from "./queue-projection";

function item(id: number, state: QueueItem["state"] = "Queued"): QueueItem {
  return {
    id,
    input: `C:\\video\\item-${id}.mkv`,
    operation: "Convert",
    intent: "ReuseIfFresh",
    output_target: "Replace",
    overwrite: "FollowSettings",
    state,
  };
}

function analysis(): AnalysisResult {
  return {
    requested_target: 95,
    successful_target: 95,
    fallback_floor: 90,
    fallback_step: 1,
    failed_attempts: [],
    measurement: {
      crf: 24_250,
      score: 9_530,
      predicted_size: 600,
      predicted_percent_basis_points: 6_000,
      predicted_duration_ms: 90_000,
      from_cache: false,
    },
    profile: {
      preset: 6,
      max_encoded_percent_basis_points: 95_00,
      samples: null,
      sample_duration_ms: 2_000,
      thorough: false,
      decode_mode: "Software",
      ab_av1_revision: "1",
      ffmpeg_revision: "2",
      encoder_revision: "3",
    },
  };
}

function run(queueItem: QueueItem, id = 10): ConversionRun {
  return {
    spec: {
      item_id: queueItem.id,
      claim_id: id,
      run_id: id,
      input: queueItem.input,
      content_key: "content-a",
      operation: queueItem.operation,
      intent: queueItem.intent,
      output_target: queueItem.output_target,
      execution: {
        requested_target: 95,
        fallback_floor: 90,
        fallback_step: 1,
        overwrite_existing: false,
        decode_preference: "SoftwareOnly",
        profile: analysis().profile,
      },
      action: { Encode: { selected_analysis: analysis() } },
    },
    analysis: analysis(),
    output_content_key: null,
    outcome: null,
    started_at: 1,
    finished_at: null,
    phase_spans: [],
  };
}

function record(): FileRecord_Deserialize {
  return {
    metadata: {
      codec: "H264",
      container: "Matroska",
      width: 1920,
      height: 1080,
      rotation_degrees: 0,
      duration_ms: 120_000,
      size_bytes: 1_000,
      audio: [{ codec: "Aac", channels: 2 }],
      subtitle_count: 0,
    },
    analyses: [],
    verdict: null,
    imported: null,
  };
}

describe("queueRows", () => {
  it("preserves durable order and QueueItemId while joining run metadata", () => {
    const first = item(41, { Running: { claim_id: 10, run_id: 10 } });
    const second = item(7);
    const state: DurableState_Deserialize = {
      ...emptyDurableState(),
      queue: [first, second],
      records: { "content-a": record() },
      conversion_runs: { 10: run(first) },
    };

    const rows = queueRows(state);

    expect(rows.map((row) => row.item.id)).toEqual([41, 7]);
    expect(rows[0]).toMatchObject({
      runId: 10,
      streams: "H264 / AAC",
      sizeBytes: 1_000,
      mediaDurationMs: 120_000,
      timeMs: null,
      crf: 24_250,
      vmaf: 9_530,
      status: { kind: "working", phase: "Preparing", percent: null },
    });
    expect(rows[1]).toMatchObject({ runId: null, streams: null, status: { kind: "queued" } });
  });

  it("keeps a reserved RunId even before the run record exists", () => {
    const reserved = item(5, { Reserved: { claim_id: 22, run_id: 22 } });
    const state = { ...emptyDurableState(), queue: [reserved] };

    expect(queueRows(state)[0]).toMatchObject({
      runId: 22,
      crf: null,
      status: { kind: "starting" },
    });
  });

  it("shows live completion sizes and exact durable phase time", () => {
    const outcome: ItemOutcome = {
      Converted: {
        LiveEncode: {
          input_size: 1_000,
          output_size: 600,
          stream_sizes: { video: 500, audio: 90, subtitle: 0, other: 10 },
          encode_decode: "Software",
        },
      },
    };
    const completed = item(2, { Finished: outcome });
    const completedRun = {
      ...run(completed),
      outcome,
      finished_at: 2,
      phase_spans: [
        { phase: "Analyzing" as const, duration: 1_250 },
        { phase: "Encoding" as const, duration: 8_750 },
      ],
    };
    const state = {
      ...emptyDurableState(),
      queue: [completed],
      records: { "content-a": record() },
      conversion_runs: { 10: completedRun },
    };

    expect(queueRows(state)[0]).toMatchObject({
      sizeBytes: 1_000,
      timeMs: 10_000,
      status: {
        kind: "done",
        outcome: "Converted",
        sizeDeltaBytes: 400,
        recovered: false,
      },
    });
  });

  it("uses settled transaction sizes but keeps recovered facts sparse", () => {
    const outcome: ItemOutcome = { Converted: "RecoveredAtStartup" };
    const recovered = item(3, { Finished: outcome });
    const recoveredRun = {
      ...run(recovered, 12),
      outcome,
      finished_at: 2,
      phase_spans: [],
    };
    const transaction = {
      run_id: 12,
      input_identity: { size: 1_200 },
      state: { Committed: { final_identity: { destructive: { size: 700 } } } },
    } as OutputTransaction_Deserialize;
    const state = {
      ...emptyDurableState(),
      queue: [recovered],
      conversion_runs: { 12: recoveredRun },
      outputs: { 12: transaction },
    };

    expect(queueRows(state)[0]).toMatchObject({
      sizeBytes: 1_200,
      timeMs: null,
      status: {
        kind: "done",
        outcome: "Converted",
        sizeDeltaBytes: 500,
        recovered: true,
      },
    });
  });
});
