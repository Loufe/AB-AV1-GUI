import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  CorruptionReport,
  QueueItem,
  Settings,
  StatisticsPayload,
  StreamPayload_Deserialize,
  Telemetry,
} from "@/lib/bindings";

import { appStore, initialAppState } from "./app-store";
import { applyPayload, hasSequenceGap } from "./connect";
import { progressStore } from "./progress-store";

vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));
vi.mock("@/lib/ipc", () => ({ subscribeStream: vi.fn() }));

function settings(): Settings {
  return {
    last_input_folder: null,
    scan_extensions: ["mp4", "mkv", "avi", "wmv"],
    output: {
      default_mode: "replace",
      suffix: "_av1",
      separate_folder: null,
      overwrite_existing: false,
    },
    hardware_decode: true,
    privacy: {
      anonymize_logs: false,
      anonymize_history: false,
    },
    log_folder: null,
  };
}

function queueItem(id: number): QueueItem {
  return {
    id,
    input: `videos/input-${id}.mp4`,
    operation: "Convert",
    intent: "ReuseIfFresh",
    output_target: "Replace",
    state: "Queued",
  };
}

function telemetry(runId: number): Telemetry {
  return { run_id: runId, sequence: 1, phase: "Encoding", progress: "Phase" };
}

function statisticsPayload(): StatisticsPayload {
  return {
    utc_offset_minutes: 60,
    converted_files: 1,
    sized_converted_files: 1,
    remuxed_files: 0,
    not_worthwhile_files: 0,
    total_input_bytes: 10_000,
    total_output_bytes: 4_000,
    total_saved_bytes: 6_000,
    remux_saved_bytes: 0,
    total_time_ms: 300_000,
    gigabytes_per_hour: 111.76,
    reduction_percent: { average: 60, minimum: 60, maximum: 60, count: 1 },
    vmaf: { average: 95.12, minimum: 95.12, maximum: 95.12, count: 1 },
    crf: { average: 24, minimum: 24, maximum: 24, count: 1 },
    reduction_bins: [0, 0, 0, 0, 0, 0, 1, 0, 0, 0],
    grew_count: 0,
    codecs: [{ codec: "Hevc", count: 1 }],
    cumulative_savings: [{ epoch_day: 20_000, cumulative_saved_bytes: 6_000 }],
    first_epoch_day: 20_000,
    last_epoch_day: 20_000,
    runs: {
      analyzed: 0,
      converted: 1,
      remuxed: 0,
      not_worthwhile: 0,
      stopped: 0,
      skipped: 0,
      failed: 0,
    },
  };
}

function corruptionReport(): CorruptionReport {
  return {
    reason: "journal is corrupt at byte 120: invalid journal record",
    signature: { tail_len: 24, digest: "ab12" },
  };
}

function snapshot(item: QueueItem): StreamPayload_Deserialize {
  return {
    Snapshot: {
      durable: {
        queue: [item],
        paths: {},
        records: {},
        outputs: {},
        conversion_runs: {},
        parked: {},
      },
      settings: settings(),
    },
  };
}

beforeEach(() => {
  appStore.setState(initialAppState(), true);
  progressStore.setState({ telemetry: {} }, true);
  vi.clearAllMocks();
});

describe("applyPayload", () => {
  it("replaces durable state and settings from a snapshot and clears health and telemetry", () => {
    appStore.setState((state) => ({
      ...state,
      health: {
        degraded: corruptionReport(),
        unavailable: "stale",
        fatal: "stale",
        secondInstance: "stale",
      },
    }));
    progressStore.setState({ telemetry: { 9: telemetry(9) } });

    applyPayload(snapshot(queueItem(1)));

    const state = appStore.getState();
    expect(state.durable.queue).toEqual([queueItem(1)]);
    expect(state.settings).toEqual(settings());
    expect(state.health).toEqual({
      degraded: null,
      unavailable: null,
      fatal: null,
      secondInstance: null,
    });
    expect(progressStore.getState().telemetry).toEqual({});
  });

  it("folds durable deltas into the app store", () => {
    applyPayload({ Durable: { QueueAdded: { item: queueItem(1) } } });
    applyPayload({ Durable: { QueueAdded: { item: queueItem(2) } } });
    expect(appStore.getState().durable.queue.map((item) => item.id)).toEqual([1, 2]);
  });

  it("applies config deltas to settings", () => {
    applyPayload({ Config: { SettingsChanged: { settings: settings() } } });
    expect(appStore.getState().settings).toEqual(settings());
  });

  it("routes session changes to the app store", () => {
    applyPayload({ Ephemeral: { SessionChanged: "Running" } });
    expect(appStore.getState().session).toBe("Running");
    expect(progressStore.getState().telemetry).toEqual({});
  });

  it("routes telemetry to the progress store without touching the app store", () => {
    const before = appStore.getState();
    applyPayload({ Ephemeral: { Telemetry: telemetry(1) } });
    expect(progressStore.getState().telemetry).toEqual({ 1: telemetry(1) });
    applyPayload({ Ephemeral: { TelemetryCleared: { run_id: 1 } } });
    expect(progressStore.getState().telemetry).toEqual({});
    expect(appStore.getState()).toBe(before);
  });

  it("surfaces a worker crash as a toast, not state", async () => {
    const { toast } = await import("sonner");
    const before = appStore.getState();
    applyPayload({ Ephemeral: { WorkerCrashed: { message: "boom" } } });
    expect(toast.error).toHaveBeenCalledWith("Worker crashed: boom");
    expect(appStore.getState()).toBe(before);
  });

  it("logs command rejections without state changes", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const before = appStore.getState();
    applyPayload({ Ephemeral: { CommandRejected: { reason: "queue is running" } } });
    expect(warn).toHaveBeenCalledWith("command rejected by the engine", "queue is running");
    expect(appStore.getState()).toBe(before);
    warn.mockRestore();
  });

  it("records tool state until the next snapshot resets it", () => {
    const missing = {
      availability: { Missing: { missing: ["Ffmpeg" as const], detail: "ffmpeg not found" } },
      activity: "Idle" as const,
      update_available: false,
    };
    applyPayload({ Ephemeral: { ToolsChanged: missing } });
    expect(appStore.getState().tools).toEqual(missing);
    expect(progressStore.getState().telemetry).toEqual({});

    const available = {
      availability: {
        Available: {
          source: "System" as const,
          revisions: { ab_av1: "rev-a", ffmpeg: "rev-f", encoder: "rev-s" },
        },
      },
      activity: "Idle" as const,
      update_available: true,
    };
    applyPayload({ Ephemeral: { ToolsChanged: available } });
    expect(appStore.getState().tools).toEqual(available);

    // The shell replays ToolsChanged right after each snapshot, so the
    // snapshot itself resets to the unknown state rather than guessing.
    applyPayload(snapshot(queueItem(1)));
    expect(appStore.getState().tools).toBeNull();
  });

  it("stores the statistics answer until the next snapshot resets it", () => {
    applyPayload({ Ephemeral: { Statistics: statisticsPayload() } });
    expect(appStore.getState().statistics).toEqual(statisticsPayload());
    expect(progressStore.getState().telemetry).toEqual({});

    // Statistics are never replayed on subscribe; the snapshot resets the
    // slot and the view re-requests.
    applyPayload(snapshot(queueItem(1)));
    expect(appStore.getState().statistics).toBeNull();
  });

  it("records standing health until the next snapshot", () => {
    applyPayload({ Degraded: corruptionReport() });
    applyPayload({ EngineUnavailable: { reason: "no data directory" } });
    applyPayload({ EngineFatal: { message: "driver exited" } });
    applyPayload({ SecondInstance: { lock_path: "/data/crfty.lock" } });
    expect(appStore.getState().health).toEqual({
      degraded: corruptionReport(),
      unavailable: "no data directory",
      fatal: "driver exited",
      secondInstance: "/data/crfty.lock",
    });

    applyPayload(snapshot(queueItem(1)));
    expect(appStore.getState().health).toEqual({
      degraded: null,
      unavailable: null,
      fatal: null,
      secondInstance: null,
    });
  });

  it("stores the full corruption report and clears only it on recovery", () => {
    applyPayload({ Degraded: corruptionReport() });
    applyPayload({ EngineFatal: { message: "driver exited" } });
    expect(appStore.getState().health.degraded).toEqual(corruptionReport());

    applyPayload("Recovered");
    const health = appStore.getState().health;
    expect(health.degraded).toBeNull();
    // Recovery clears the corruption report and nothing else.
    expect(health.fatal).toBe("driver exited");
  });
});

describe("hasSequenceGap", () => {
  it("accepts a fresh connection's zero and contiguous sequences", () => {
    expect(hasSequenceGap(null, 0)).toBe(false);
    expect(hasSequenceGap(0, 1)).toBe(false);
    expect(hasSequenceGap(41, 42)).toBe(false);
    // A replayed subscription restarts numbering at zero.
    expect(hasSequenceGap(41, 0)).toBe(false);
  });

  it("flags anything else as a gap", () => {
    expect(hasSequenceGap(null, 3)).toBe(true);
    expect(hasSequenceGap(0, 2)).toBe(true);
    expect(hasSequenceGap(41, 43)).toBe(true);
    expect(hasSequenceGap(41, 41)).toBe(true);
  });
});
