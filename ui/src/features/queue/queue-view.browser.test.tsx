import { page, userEvent } from "vitest/browser";
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

function folderItem(id: number, folder: string, name: string): QueueItem {
  return { ...item(id, name), input: `C:\\${folder}\\${name}` };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
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

    const recoveredRow = page.getByRole("row", { name: /recovered\.mkv/ });
    await recoveredRow.click();
    await expect.element(page.getByText("Selection · recovered.mkv")).toBeVisible();
    await expect.element(page.getByText("—", { exact: true }).first()).toBeVisible();
    await expect.element(recoveredRow).toHaveAttribute("aria-selected", "true");
    const cells = recoveredRow.element().querySelectorAll(':scope > [role="cell"]');
    expect(cells).toHaveLength(8);
    expect(cells[1]?.textContent).toContain("recovered.mkv");
    expect(cells[7]?.textContent).toContain("Done");
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
      .element(page.getByText("VMAF 95.3 · CRF 24.25 · 2.45 fps · ETA < 1m"))
      .toBeVisible();
    await expect
      .element(page.getByRole("progressbar", { name: "Analyzing progress" }))
      .toHaveAttribute("aria-valuenow", "20");
    await expect
      .element(page.getByRole("progressbar", { name: "Analyze progress" }))
      .toHaveAttribute("aria-valuenow", "20");

    progressStore.setState({ telemetry: { 9: telemetry(2, 7_800) } });

    await expect.element(page.getByText("Analyzing… 78%", { exact: true })).toBeVisible();
    await expect
      .element(page.getByRole("progressbar", { name: "Analyzing progress" }))
      .toHaveAttribute("aria-valuenow", "78");
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

    const preparing = page.getByRole("progressbar", { name: "Preparing progress" });
    await expect.element(preparing).toBeVisible();
    expect(preparing.element().hasAttribute("aria-valuenow")).toBe(false);

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
      .element(page.getByRole("alert"))
      .toHaveTextContent("queue start failed (rejected): queue changed");
    expect(appStore.getState().session).toBe("Idle");
    expect(appStore.getState().durable).toBe(state);
  });

  it("confirms remove, clear, and clear-completed before sending stable commands", async () => {
    const tauri = installTauriMock({
      queue_remove_many: () => null,
      queue_clear: () => null,
      queue_clear_completed: () => null,
    });
    const state = durable([
      item(11, "selected.mkv"),
      item(12, "done.mkv", { Finished: "Analyzed" }),
      item(13, "also-selected.mkv"),
    ]);
    await renderApp(<QueueView />, {
      appState: { durable: state, settings: settings(), tools: tools() },
    });

    await page.getByRole("row", { name: /Reorder selected\.mkv/ }).click();
    page
      .getByRole("row", { name: /Reorder also-selected\.mkv/ })
      .element()
      .dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    await page.getByRole("button", { name: "Remove 2", exact: true }).click();
    const removeDialog = page.getByRole("alertdialog");
    await expect.element(removeDialog).toHaveTextContent("Remove 2 selected items?");
    await removeDialog.getByRole("button", { name: "Remove 2", exact: true }).click();
    await expect.poll(() => tauri.callsFor("queue_remove_many").length).toBe(1);
    expect(tauri.callsFor("queue_remove_many")[0]?.payload).toMatchObject({ itemIds: [11, 13] });

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

  it("toggles grouping without mutation and explicitly regroups one exact permutation", async () => {
    const tauri = installTauriMock({ queue_reorder_pending: () => null });
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([
          folderItem(1, "A", "one.mkv"),
          folderItem(2, "B", "two.mkv"),
          folderItem(3, "A", "three.mkv"),
        ]),
        settings: settings(),
        tools: tools(),
      },
    });

    await page.getByRole("button", { name: "Group by folder" }).click();
    expect(tauri.callsFor("queue_reorder_pending")).toHaveLength(0);
    await page.getByRole("button", { name: "Group by folder" }).click();
    await page.getByRole("button", { name: "Regroup pending items" }).click();

    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);
    expect(tauri.callsFor("queue_reorder_pending")[0]?.payload).toMatchObject({
      pendingOrder: [1, 3, 2],
    });
  });

  it("moves folder runs and noncontiguous selections through click/tap alternatives", async () => {
    const tauri = installTauriMock({ queue_reorder_pending: () => null });
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([
          folderItem(1, "A", "one.mkv"),
          folderItem(2, "B", "two.mkv"),
          folderItem(3, "C", "three.mkv"),
        ]),
        settings: settings(),
        tools: tools(),
      },
    });

    await page.getByRole("button", { name: "Move B top" }).click();
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);
    expect(tauri.callsFor("queue_reorder_pending")[0]?.payload).toMatchObject({
      pendingOrder: [2, 1, 3],
    });

    appStore.setState((state) => ({
      ...state,
      durable: durable([
        folderItem(2, "B", "two.mkv"),
        folderItem(1, "A", "one.mkv"),
        folderItem(3, "C", "three.mkv"),
      ]),
    }));
    await expect
      .element(page.getByRole("status").filter({ hasText: "Moved B" }))
      .toBeInTheDocument();
    appStore.setState((state) => ({
      ...state,
      durable: durable([
        folderItem(1, "A", "one.mkv"),
        folderItem(2, "B", "two.mkv"),
        folderItem(3, "C", "three.mkv"),
      ]),
    }));
    await page.getByRole("row", { name: /one\.mkv/ }).click();
    page
      .getByRole("row", { name: /three\.mkv/ })
      .element()
      .dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    await expect.element(page.getByRole("button", { name: "Move selected to top" })).toBeEnabled();
    await page.getByRole("button", { name: "Move selected to top" }).click();
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(2);
    expect(tauri.callsFor("queue_reorder_pending")[1]?.payload).toMatchObject({
      pendingOrder: [1, 3, 2],
    });
  });

  it("supports keyboard sortable movement with human announcements", async () => {
    const tauri = installTauriMock({ queue_reorder_pending: () => null });
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([item(1, "one.mkv"), item(2, "two.mkv"), item(3, "three.mkv")]),
        settings: settings(),
        tools: tools(),
      },
    });
    await page.getByRole("button", { name: "Group by folder" }).click();
    page.getByRole("button", { name: "Reorder one.mkv" }).element().focus();
    await userEvent.keyboard(" ");
    await expect
      .element(page.getByRole("status").filter({ hasText: "one.mkv, position 1." }))
      .toBeInTheDocument();
    await userEvent.keyboard("{ArrowDown}");
    await userEvent.keyboard(" ");

    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);
    expect(tauri.callsFor("queue_reorder_pending")[0]?.payload).toMatchObject({
      pendingOrder: [2, 1, 3],
    });
  });

  it("retains immutable cross-folder plans across cancel and confirmation", async () => {
    const tauri = installTauriMock({ queue_reorder_pending: () => null });
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([
          folderItem(1, "A", "one.mkv"),
          folderItem(2, "A", "two.mkv"),
          folderItem(3, "B", "three.mkv"),
        ]),
        settings: settings(),
        tools: tools(),
      },
    });
    await page.getByRole("row", { name: /two\.mkv/ }).click();
    const moveDown = page.getByRole("button", { name: "Move selected down" });
    await moveDown.click();
    await expect.element(page.getByRole("alertdialog")).toBeVisible();
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect
      .poll(() => document.activeElement?.getAttribute("aria-label"))
      .toBe("Move selected down");
    expect(tauri.callsFor("queue_reorder_pending")).toHaveLength(0);

    await moveDown.click();
    await page.getByRole("button", { name: "Ungroup and move" }).click();
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);
    expect(tauri.callsFor("queue_reorder_pending")[0]?.payload).toMatchObject({
      pendingOrder: [1, 3, 2],
    });
    await expect
      .element(page.getByRole("button", { name: "Group by folder" }))
      .toHaveAttribute("aria-pressed", "false");
  });

  it("reconciles delta-before-ack and does not hide rows added during submission", async () => {
    const reorder = deferred<null>();
    const tauri = installTauriMock({ queue_reorder_pending: () => reorder.promise });
    const initial = [item(1, "one.mkv"), item(2, "two.mkv"), item(3, "three.mkv")];
    await renderApp(<QueueView />, {
      appState: { durable: durable(initial), settings: settings(), tools: tools() },
    });
    await page.getByRole("row", { name: /two\.mkv/ }).click();
    await page.getByRole("button", { name: "Move selected to top" }).click();
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);

    appStore.setState((state) => ({
      ...state,
      durable: durable([initial[1]!, initial[0]!, initial[2]!]),
    }));
    await expect
      .element(page.getByRole("status").filter({ hasText: "Submitting Queue order" }))
      .toBeInTheDocument();
    reorder.resolve(null);
    await expect
      .element(page.getByRole("status").filter({ hasText: "Moved two.mkv to position 1 of 3" }))
      .toBeInTheDocument();

    const second = deferred<null>();
    tauri.setCommand("queue_reorder_pending", () => second.promise);
    await page.getByRole("button", { name: "Move selected to bottom" }).click();
    appStore.setState((state) => ({
      ...state,
      durable: durable([initial[0]!, initial[2]!, initial[1]!, item(4, "added.mkv")]),
    }));
    await expect.element(page.getByRole("row", { name: /added\.mkv/ })).toBeVisible();
    await expect.element(page.getByRole("alert")).toHaveTextContent("latest order was restored");
    second.resolve(null);
  });

  it("uses atomic recovery and keeps Open failures local and operator-visible", async () => {
    const tauri = installTauriMock({ queue_retry: () => null });
    tauri.rejectCommand("open_path", { code: "io", message: "file is unavailable" });
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([item(7, "failed.mkv", { Finished: "Stopped" })]),
        settings: settings(),
        tools: tools(),
      },
    });
    await page.getByRole("row", { name: /failed\.mkv/ }).click();
    await page.getByRole("button", { name: "Convert anyway" }).click();
    await expect.poll(() => tauri.callsFor("queue_retry").length).toBe(1);
    expect(tauri.callsFor("queue_retry")[0]?.payload).toMatchObject({
      itemId: 7,
      patch: { operation: "Convert", intent: "Refresh" },
    });

    await page.getByRole("button", { name: "Open", exact: true }).click();
    await expect.element(page.getByRole("alert")).toHaveTextContent("file is unavailable");
    expect(appStore.getState().durable.queue[0]?.intent).toBe("ReuseIfFresh");
  });

  it("keeps durable actions disabled under degraded health while Open remains available", async () => {
    const tauri = installTauriMock({ open_path: () => null });
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([item(7, "failed.mkv", { Finished: "Stopped" })]),
        settings: settings(),
        tools: tools(),
        health: {
          unavailable: "journal unavailable",
          fatal: null,
          degraded: null,
          secondInstance: null,
        },
      },
    });
    await page.getByRole("row", { name: /failed\.mkv/ }).click();
    await expect.element(page.getByRole("button", { name: "Retry" })).toBeDisabled();
    await expect.element(page.getByRole("button", { name: "Convert anyway" })).toBeDisabled();
    await page.getByRole("button", { name: "Open", exact: true }).click();
    await expect.poll(() => tauri.callsFor("open_path").length).toBe(1);
  });

  it("renders the accepted 500-row nonvirtualized fixture", async () => {
    const rows = Array.from({ length: 500 }, (_, index) =>
      item(index + 1, `video-${index + 1}.mkv`),
    );
    await renderApp(<QueueView />, {
      appState: { durable: durable(rows), settings: settings(), tools: tools() },
    });
    await page.getByRole("button", { name: "Group by folder" }).click();
    expect(page.getByRole("row").elements()).toHaveLength(502);
    await expect.element(page.getByRole("row", { name: /video-500\.mkv/ })).toBeInTheDocument();
  });
});
