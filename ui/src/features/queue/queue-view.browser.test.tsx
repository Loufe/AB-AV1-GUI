import { page } from "vitest/browser";
import { describe, expect, it } from "vitest";

import type {
  ConversionRun,
  DurableState_Deserialize,
  QueueItem,
  Settings,
  Telemetry,
  ToolsState,
} from "@/lib/bindings";
import { appStore } from "@/lib/store/app-store";
import { emptyDurableState } from "@/lib/store/fold";
import { progressStore } from "@/lib/store/progress-store";
import { renderApp } from "@/test/browser/render";
import { installTauriMock } from "@/test/browser/tauri";

import { QueueView } from "./queue-view";

function settings(): Settings {
  return {
    last_input_folder: "C:\\Videos",
    scan_extensions: ["mp4", "mkv", "avi", "wmv"],
    output: {
      default_mode: "suffix",
      suffix: "_av1",
      separate_folder: null,
      overwrite_existing: false,
    },
    hardware_decode: true,
    privacy: { anonymize_logs: false, anonymize_history: false },
    log_folder: null,
  };
}

function tools(): ToolsState {
  return {
    availability: {
      Available: {
        source: "System",
        revisions: { ab_av1: "1", ffmpeg: "2", encoder: "3" },
      },
    },
    activity: "Idle",
    update_available: false,
  };
}

function item(id: number, name: string, state: QueueItem["state"] = "Queued"): QueueItem {
  return {
    id,
    input: `C:\\Videos\\${name}`,
    operation: "Convert",
    intent: "ReuseIfFresh",
    output_target: { Suffix: { suffix: "_av1" } },
    overwrite: "FollowSettings",
    state,
  };
}

function durable(queue: QueueItem[]): DurableState_Deserialize {
  return { ...emptyDurableState(), queue };
}

function runningState(): DurableState_Deserialize {
  const running = item(9, "run.mkv", { Running: { claim_id: 9, run_id: 9 } });
  const run: ConversionRun = {
    spec: {
      item_id: 9,
      claim_id: 9,
      run_id: 9,
      input: running.input,
      content_key: "run-content",
      operation: "Convert",
      intent: "ReuseIfFresh",
      output_target: running.output_target,
    } as ConversionRun["spec"],
    analysis: {
      measurement: { crf: 24_250, score: 9_530 },
    } as ConversionRun["analysis"],
    output_content_key: null,
    outcome: null,
    started_at: 1,
    finished_at: null,
    phase_spans: [],
  };
  return {
    ...durable([running, item(10, "next.mkv")]),
    conversion_runs: { 9: run },
    records: {
      "run-content": {
        metadata: {
          codec: "H264",
          container: "Matroska",
          width: 1920,
          height: 1080,
          rotation_degrees: 0,
          duration_ms: 100_000,
          size_bytes: 1_000,
          audio: [{ codec: "Aac", channels: 2 }],
          subtitle_count: 0,
        },
        analyses: [],
        verdict: null,
        imported: null,
      },
    },
  };
}

function telemetry(sequence: number, basisPoints: number): Telemetry {
  return {
    run_id: 9,
    sequence,
    phase: "Analyzing",
    progress: { SearchBasisPoints: basisPoints },
    fps_centi: 245,
    eta_ms: 42_000,
  };
}

describe("QueueView", () => {
  it("projects mixed durable outcomes, including sparse recovery and diagnostics", async () => {
    const rows = [
      item(1, "queued.mkv"),
      item(2, "converted.mkv", {
        Finished: {
          Converted: {
            LiveEncode: {
              input_size: 2_000,
              output_size: 1_000,
              stream_sizes: { video: 900, audio: 90, subtitle: 0, other: 10 },
              encode_decode: "Software",
            },
          },
        },
      }),
      item(3, "recovered.mkv", { Finished: { Converted: "RecoveredAtStartup" } }),
      item(4, "skipped.mkv", {
        Finished: { Skipped: { reason: "AlreadyAv1Matroska" } },
      }),
      item(5, "failed.mkv", {
        Finished: {
          Failed: { kind: "EncodeRun", message: "encoder exited", diagnostic: "tail" },
        },
      }),
    ];
    await renderApp(<QueueView />, {
      appState: { durable: durable(rows), settings: settings(), tools: tools() },
    });

    await expect.element(page.getByRole("row", { name: /queued\.mkv/ })).toBeVisible();
    await expect.element(page.getByText("Done · saved 1000 B", { exact: true })).toBeVisible();
    await expect.element(page.getByRole("row", { name: /recovered\.mkv/ })).toBeVisible();
    await expect.element(page.getByText("Skipped · already AV1", { exact: true })).toBeVisible();
    await expect.element(page.getByText("Error · encoder exited", { exact: true })).toBeVisible();

    await page.getByRole("row", { name: /recovered\.mkv/ }).click();
    await expect.element(page.getByText("Selection · recovered.mkv")).toBeVisible();
    await expect.element(page.getByText("—", { exact: true }).first()).toBeVisible();
  });

  it("updates only live RunId facts and preserves row identity and selection", async () => {
    const state = runningState();
    await renderApp(<QueueView />, {
      appState: { durable: state, settings: settings(), session: "Running", tools: tools() },
      progressState: { telemetry: { 9: telemetry(1, 2_000) } },
    });

    const row = page.getByRole("row", { name: /run\.mkv/ });
    await row.click();
    const rowBefore = row.element();
    await expect.element(page.getByText("Selection · run.mkv")).toBeVisible();
    await expect.element(page.getByText("Analyzing… 20%", { exact: true })).toBeVisible();
    await expect
      .element(page.getByText("VMAF 95.3 · CRF 24.25 · 2.45 fps · ETA 42s"))
      .toBeVisible();

    progressStore.setState({ telemetry: { 9: telemetry(2, 7_800) } });

    await expect.element(page.getByText("Analyzing… 78%", { exact: true })).toBeVisible();
    await expect.element(page.getByText("Selection · run.mkv")).toBeVisible();
    expect(row.element()).toBe(rowBefore);
  });

  it("keeps stop-after-current and force-stop distinct without optimistic state", async () => {
    const tauri = installTauriMock({ stop_after_current: () => null, force_stop: () => null });
    await renderApp(<QueueView />, {
      appState: {
        durable: runningState(),
        settings: settings(),
        session: "Running",
        tools: tools(),
      },
    });

    await page.getByRole("button", { name: "Stop After File" }).click();
    await expect.poll(() => tauri.callsFor("stop_after_current").length).toBe(1);
    expect(appStore.getState().session).toBe("Running");

    appStore.setState((state) => ({ ...state, session: "StopAfterCurrent" }));
    await expect.element(page.getByRole("button", { name: "Stop After File" })).toBeDisabled();
    await page.getByRole("button", { name: "Force Stop" }).click();
    await expect.poll(() => tauri.callsFor("force_stop").length).toBe(1);
    expect(appStore.getState().session).toBe("StopAfterCurrent");
  });

  it("uses native picker intents and waits for durable deltas after accepted commands", async () => {
    const tauri = installTauriMock({
      pick_paths: (payload) =>
        payload !== undefined &&
        !Array.isArray(payload) &&
        "kind" in payload &&
        payload.kind === "Files"
          ? ["C:\\picked\\one.mkv"]
          : [],
      queue_add_paths: () => null,
      start: () => null,
    });
    const state = durable([item(1, "queued.mkv")]);
    await renderApp(<QueueView />, {
      appState: { durable: state, settings: settings(), tools: tools() },
    });

    await page.getByRole("button", { name: "Add Files" }).click();
    await expect.poll(() => tauri.callsFor("queue_add_paths").length).toBe(1);
    expect(tauri.callsFor("pick_paths")[0]?.payload).toMatchObject({
      kind: "Files",
      startingDirectory: "C:\\Videos",
    });
    expect(tauri.callsFor("queue_add_paths")[0]?.payload).toMatchObject({
      inputs: ["C:\\picked\\one.mkv"],
      operation: "Convert",
      intent: "ReuseIfFresh",
      outputTarget: { Suffix: { suffix: "_av1" } },
    });
    expect(appStore.getState().durable).toBe(state);

    await page.getByRole("button", { name: "Add Folders" }).click();
    await expect.poll(() => tauri.callsFor("pick_paths").length).toBe(2);
    expect(tauri.callsFor("queue_add_paths")).toHaveLength(1);

    await page.getByRole("button", { name: "Start Queue" }).click();
    await expect.poll(() => tauri.callsFor("start").length).toBe(1);
    expect(appStore.getState().session).toBe("Idle");
  });

  it("surfaces missing tools and degraded health", async () => {
    const missing: ToolsState = {
      availability: { Missing: { missing: ["Ffmpeg"], detail: "FFmpeg was not found" } },
      activity: "Idle",
      update_available: false,
    };
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([item(1, "queued.mkv")]),
        settings: settings(),
        tools: missing,
      },
    });

    await expect.element(page.getByText("FFmpeg was not found")).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Start Queue" })).toBeDisabled();

    appStore.setState((state) => ({
      ...state,
      tools: tools(),
      health: {
        ...state.health,
        degraded: {
          reason: "journal tail is corrupt",
          signature: { tail_len: 10, digest: "abc" },
        },
      },
    }));
    await expect
      .element(page.getByText("Queue changes are unavailable: journal tail is corrupt"))
      .toBeVisible();
  });

  it("surfaces command rejection and leaves authoritative state untouched", async () => {
    const tauri = installTauriMock();
    tauri.rejectCommand("start", { code: "rejected", message: "queue changed" });
    const state = durable([item(1, "queued.mkv")]);
    await renderApp(<QueueView />, {
      appState: { durable: state, settings: settings(), tools: tools() },
    });

    await page.getByRole("button", { name: "Start Queue" }).click();

    await expect
      .element(page.getByText("queue start failed (rejected): queue changed"))
      .toBeVisible();
    expect(appStore.getState().session).toBe("Idle");
    expect(appStore.getState().durable).toBe(state);
  });

  it("confirms remove, clear, and clear-completed before sending stable commands", async () => {
    const tauri = installTauriMock({
      queue_remove: () => null,
      queue_clear: () => null,
      queue_clear_completed: () => null,
    });
    const state = durable([
      item(11, "selected.mkv"),
      item(12, "done.mkv", { Finished: "Analyzed" }),
    ]);
    await renderApp(<QueueView />, {
      appState: { durable: state, settings: settings(), tools: tools() },
    });

    await page.getByRole("row", { name: /selected\.mkv/ }).click();
    await page.getByRole("button", { name: "Remove", exact: true }).click();
    const removeDialog = page.getByRole("alertdialog");
    await removeDialog.getByRole("button", { name: "Remove", exact: true }).click();
    await expect.poll(() => tauri.callsFor("queue_remove").length).toBe(1);
    expect(tauri.callsFor("queue_remove")[0]?.payload).toMatchObject({ itemId: 11 });

    await page.getByRole("button", { name: "Clear Completed", exact: true }).click();
    const completedDialog = page.getByRole("alertdialog");
    await completedDialog.getByRole("button", { name: "Clear Completed", exact: true }).click();
    await expect.poll(() => tauri.callsFor("queue_clear_completed").length).toBe(1);

    await page.getByRole("button", { name: "Clear", exact: true }).click();
    const clearDialog = page.getByRole("alertdialog");
    await clearDialog.getByRole("button", { name: "Clear", exact: true }).click();
    await expect.poll(() => tauri.callsFor("queue_clear").length).toBe(1);
    expect(appStore.getState().durable).toBe(state);
  });
});
