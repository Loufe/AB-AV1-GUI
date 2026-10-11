import { Activity, useState } from "react";
import { describe, expect, it } from "vitest";
import { page, userEvent } from "vitest/browser";

import App from "@/App";
import { ViewActiveContext } from "@/components/layout/view-activity";
import type {
  AnalysisGeneration_Deserialize,
  AnalysisRow,
  Operation,
  QueueItem,
} from "@/lib/bindings";
import { applyPayload } from "@/lib/store/connect";
import { appStore } from "@/lib/store/app-store";
import { renderApp } from "@/test/browser/render";
import { installTauriMock } from "@/test/browser/tauri";
import {
  analysisGeneration,
  analysisSettings,
  analysisTools,
  convertedRow,
  discoveredRow,
  predictedRow,
  scannedRow,
} from "@/test/fixtures/analysis";
import { AnalysisView } from "./analysis-view";

function publish(generation: AnalysisGeneration_Deserialize): void {
  applyPayload({ Ephemeral: { Analysis: { Reset: { snapshot: { current: generation } } } } });
}

function update(rows: AnalysisRow[], generation = 7): void {
  applyPayload({ Ephemeral: { Analysis: { RowsUpserted: { generation, rows } } } });
}

async function renderAnalysis(generation?: AnalysisGeneration_Deserialize) {
  const result = await renderApp(
    <ViewActiveContext value={true}>
      <AnalysisView />
    </ViewActiveContext>,
    {
      appState: { settings: analysisSettings(), tools: analysisTools() },
    },
  );
  if (generation !== undefined) publish(generation);
  return result;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((settle, fail) => {
    resolve = settle;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe("production Analysis workflow", () => {
  it("picks folders, scans, enqueues Analyze, starts Queue, inspects a search prediction, and enqueues Convert", async () => {
    const tauri = installTauriMock({
      app_info: () => ({ version: "fixture", platform: "Linux" }),
      pick_paths: () => ["/videos"],
      analysis_discover: () => {
        tauri.emit({
          Ephemeral: { Analysis: { Reset: { snapshot: { current: analysisGeneration() } } } },
        });
        return 7;
      },
      analysis_basic_scan: () => {
        tauri.emit({
          Ephemeral: { Analysis: { RowsUpserted: { generation: 7, rows: [scannedRow()] } } },
        });
        tauri.emit({
          Ephemeral: { Analysis: { ActivityChanged: { generation: 7, activity: "Ready" } } },
        });
        return null;
      },
      analysis_enqueue: (payload) => {
        const operation = (payload as { request: { operation: Operation } }).request.operation;
        const item: QueueItem = {
          id: operation === "Analyze" ? 1 : 2,
          input: "/videos/series/movie.mkv",
          operation,
          intent: "ReuseIfFresh",
          output_target: { Suffix: { suffix: "_av1" } },
          overwrite: "FollowSettings",
          state: "Queued",
        };
        tauri.emit({ Durable: { QueueAdded: { item } } });
        return { added: 1, excluded: [] };
      },
      start: () => null,
    });
    await renderApp(<App />, {
      appState: { settings: analysisSettings(), tools: analysisTools() },
    });
    await expect.poll(() => tauri.callsFor("subscribe").length).toBe(1);
    await page.getByRole("button", { name: "Analysis", exact: true }).click();
    await page.getByRole("button", { name: "Choose folders…" }).click();
    await expect
      .element(page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }))
      .toBeVisible();
    expect(tauri.callsFor("pick_paths")[0]?.payload).toEqual({
      kind: "Folders",
      startingDirectory: "/videos",
    });
    expect(tauri.callsFor("analysis_discover")[0]?.payload).toEqual({ roots: ["/videos"] });
    await page.getByRole("button", { name: "Basic Scan", exact: true }).click();
    expect(tauri.callsFor("analysis_basic_scan")[0]?.payload).toEqual({ generation: 7 });
    await expect.element(page.getByText("H.264 · MKV")).toBeVisible();
    await page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }).click();
    await page.getByRole("button", { name: "Analyze selected" }).click();
    await expect.element(page.getByText("1 added to Queue.")).toBeVisible();
    expect(tauri.callsFor("analysis_enqueue")[0]?.payload).toEqual({
      request: { generation: 7, operation: "Analyze", selection: { Selected: { row_ids: [2] } } },
    });
    await page.getByRole("button", { name: "Queue", exact: true }).click();
    await page.getByRole("button", { name: "Start Queue", exact: true }).click();
    await expect.poll(() => tauri.callsFor("start").length).toBe(1);
    tauri.emit({
      Ephemeral: { Analysis: { RowsUpserted: { generation: 7, rows: [predictedRow()] } } },
    });
    await page.getByRole("button", { name: "Analysis", exact: true }).click();
    await expect
      .element(page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }))
      .toBeChecked();
    await expect.element(page.getByText("Analyzed", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Inspect movie.mkv" }).click();
    await expect.element(page.getByText(/Applicable search prediction:/)).toBeVisible();
    await expect.element(page.getByText(/VMAF 96.5, CRF 28/)).toBeVisible();
    await page.getByRole("button", { name: "Convert selected" }).click();
    expect(tauri.callsFor("analysis_enqueue")[1]?.payload).toEqual({
      request: { generation: 7, operation: "Convert", selection: { Selected: { row_ids: [2] } } },
    });
    expect(appStore.getState().durable.queue.map((item) => item.operation)).toEqual([
      "Analyze",
      "Convert",
    ]);
  });

  it("preserves selection during incremental updates and shows bounded errors and per-reason queue exclusions", async () => {
    const failed: AnalysisRow = {
      ...discoveredRow(3, "failed.mkv"),
      entry: {
        File: {
          scan: {
            Failed: {
              failure: {
                TimedOut: { diagnostic: { text: "fixture probe tail", truncated: true } },
              },
            },
          },
        },
      },
    };
    const folder: AnalysisRow = {
      id: 4,
      parent: null,
      display_name: { text: "restricted", lossy: false },
      display_path: { text: "/videos/restricted", lossy: false },
      entry: { Folder: { failure: "PermissionDenied" } },
    };
    const tauri = installTauriMock({
      analysis_enqueue: () => ({
        added: 1,
        excluded: [
          ["ScanFailed", 1],
          [{ Policy: { reason: "AlreadyQueued" } }, 1],
        ],
      }),
    });
    await renderAnalysis(analysisGeneration([discoveredRow(), failed, folder], "Ready"));
    await page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }).click();
    update([scannedRow(), discoveredRow(5, "later.mkv")]);
    await expect
      .element(page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }))
      .toBeChecked();
    await page.getByRole("button", { name: "Inspect failed.mkv" }).click();
    await expect
      .element(page.getByRole("region", { name: "File details" }))
      .toHaveTextContent("fixture probe tail");
    await expect
      .element(page.getByRole("region", { name: "File details" }))
      .toHaveTextContent("Diagnostic tail truncated");
    await page.getByRole("button", { name: "Inspect restricted" }).click();
    await expect
      .element(page.getByRole("region", { name: "File details" }))
      .toHaveTextContent("Permission to read the folder was denied");
    await page.getByRole("button", { name: "Convert all" }).click();
    await expect.element(page.getByText("Scan failures: 1")).toBeVisible();
    await expect.element(page.getByText("already queued: 1")).toBeVisible();
    expect(tauri.callsFor("analysis_enqueue")[0]?.payload).toEqual({
      request: { generation: 7, operation: "Convert", selection: "AllFiles" },
    });
  });

  it("cancels a running scan using authoritative activity and prevents queue actions after cancellation", async () => {
    const tauri = installTauriMock({ analysis_cancel: () => null });
    await renderAnalysis(analysisGeneration([discoveredRow()], "BasicScanning"));
    await page.getByRole("button", { name: "Cancel scan" }).click();
    expect(tauri.callsFor("analysis_cancel")).toHaveLength(1);
    await expect.element(page.getByText(/Basic Scan in progress/)).toBeVisible();
    applyPayload({
      Ephemeral: { Analysis: { ActivityChanged: { generation: 7, activity: "Cancelled" } } },
    });
    await expect.element(page.getByText(/Scan cancelled/)).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Analyze all" })).toBeDisabled();
    await expect
      .element(page.getByRole("button", { name: "Basic Scan", exact: true }))
      .toBeDisabled();
    await expect.element(page.getByRole("button", { name: "Choose folders…" })).toBeEnabled();
  });

  it("invalidates selection, old command responses, and row focus when the generation changes", async () => {
    const response = deferred<{ added: number; excluded: [] }>();
    const tauri = installTauriMock({ analysis_enqueue: () => response.promise });
    await renderAnalysis(analysisGeneration());
    await page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }).click();
    await page.getByRole("button", { name: "Analyze selected" }).click();
    publish(analysisGeneration([discoveredRow(2, "replacement.mkv")], "Discovered", 8));
    response.resolve({ added: 99, excluded: [] });
    await expect.element(page.getByRole("button", { name: "Choose folders…" })).toHaveFocus();
    await expect
      .element(page.getByRole("checkbox", { name: "Select replacement.mkv", exact: true }))
      .not.toBeChecked();
    await expect.element(page.getByRole("button", { name: "Analyze selected" })).toBeDisabled();
    expect(document.body.textContent).not.toContain("99 added");
    await page.getByRole("button", { name: "Analyze all" }).click();
    expect(tauri.callsFor("analysis_enqueue")[1]?.payload).toEqual({
      request: { generation: 8, operation: "Analyze", selection: "AllFiles" },
    });
  });

  it("clears stale selection on reconnect even when a restarted engine reuses the generation ID", async () => {
    await renderAnalysis(analysisGeneration());
    await page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }).click();
    applyPayload({
      Snapshot: { durable: appStore.getState().durable, settings: analysisSettings() },
    });
    publish(analysisGeneration());
    await expect
      .element(page.getByRole("checkbox", { name: "Select movie.mkv", exact: true }))
      .not.toBeChecked();
    await expect.element(page.getByRole("button", { name: "Analyze selected" })).toBeDisabled();
    await expect.element(page.getByRole("button", { name: "Choose folders…" })).toHaveFocus();
  });

  it("keeps picker cancellation quiet and command rejection inspectable", async () => {
    const tauri = installTauriMock({ pick_paths: () => [] });
    await renderAnalysis();
    await page.getByRole("button", { name: "Choose folders…" }).click();
    expect(tauri.callsFor("analysis_discover")).toHaveLength(0);
    tauri.rejectCommand("pick_paths", { code: "picker_failed", message: "fixture picker error" });
    await page.getByRole("button", { name: "Choose folders…" }).click();
    await expect.element(page.getByRole("alert")).toHaveTextContent("fixture picker error");
    publish(analysisGeneration());
    tauri.rejectCommand("analysis_basic_scan", { code: "rejected", message: "fixture scan error" });
    await page.getByRole("button", { name: "Basic Scan", exact: true }).click();
    await expect.element(page.getByText(/Basic Scan failed .*fixture scan error/)).toBeVisible();
    tauri.rejectCommand("analysis_enqueue", {
      code: "durability_unknown",
      message: "fixture uncertain commit",
    });
    await page.getByRole("button", { name: "Convert all" }).click();
    await expect
      .element(page.getByText(/Queue action failed .*fixture uncertain commit/))
      .toBeVisible();
    expect(appStore.getState().durable.queue).toHaveLength(0);
  });

  it("returns focus to the invoking file after keyboard inspection and to Search if that file is filtered out", async () => {
    await renderAnalysis(analysisGeneration());
    const inspect = page.getByRole("button", { name: "Inspect movie.mkv" });
    (inspect.element() as HTMLElement).focus();
    await userEvent.keyboard("{Enter}");
    await expect
      .element(page.getByRole("heading", { name: "Details for movie.mkv" }))
      .toHaveFocus();
    await userEvent.tab();
    await expect.element(page.getByRole("button", { name: "Close details" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await expect.element(inspect).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    const search = page.getByRole("textbox", { name: "Search Analysis" });
    await search.fill("no-match");
    await page.getByRole("button", { name: "Close details" }).click();
    await expect.element(search).toHaveFocus();
  });

  it("discards picker errors on generation replacement and reconnect, including late rejection", async () => {
    const tauri = installTauriMock();
    tauri.rejectCommand("pick_paths", { code: "picker_failed", message: "fixture picker error" });
    await renderAnalysis(analysisGeneration());
    await page.getByRole("button", { name: "Choose folders…" }).click();
    await expect.element(page.getByRole("alert")).toHaveTextContent("fixture picker error");
    publish(analysisGeneration([discoveredRow()], "Discovered", 8));
    await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
    await page.getByRole("button", { name: "Choose folders…" }).click();
    await expect.element(page.getByRole("alert")).toHaveTextContent("fixture picker error");
    applyPayload({
      Snapshot: { durable: appStore.getState().durable, settings: analysisSettings() },
    });
    publish(analysisGeneration([discoveredRow()], "Discovered", 8));
    await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
    const response = deferred<string[]>();
    tauri.setCommand("pick_paths", () => response.promise);
    await page.getByRole("button", { name: "Choose folders…" }).click();
    publish(analysisGeneration([discoveredRow()], "Discovered", 9));
    response.reject({ code: "picker_failed", message: "fixture late picker error" });
    await expect.element(page.getByRole("button", { name: "Choose folders…" })).toBeEnabled();
    await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
    expect(tauri.callsFor("analysis_discover")).toHaveLength(0);
  });

  it("clears the previous enqueue summary before an uncertain new queue action", async () => {
    const tauri = installTauriMock({ analysis_enqueue: () => ({ added: 1, excluded: [] }) });
    await renderAnalysis(analysisGeneration());
    await page.getByRole("button", { name: "Analyze all" }).click();
    await expect.element(page.getByText("1 added to Queue.")).toBeVisible();
    tauri.rejectCommand("analysis_enqueue", {
      code: "durability_unknown",
      message: "fixture uncertain commit",
    });
    await page.getByRole("button", { name: "Convert all" }).click();
    await expect.element(page.getByRole("alert")).toHaveTextContent("fixture uncertain commit");
    await expect.element(page.getByText("1 added to Queue.")).not.toBeInTheDocument();
  });

  it("labels settled output metadata as the current file size and renders portrait dimensions", async () => {
    const portrait = scannedRow(3, "portrait.mkv");
    if (
      portrait.entry.File?.scan === "Discovered" ||
      portrait.entry.File?.scan.Scanned === undefined
    )
      throw new Error("fixture");
    portrait.entry.File.scan.Scanned.metadata.rotation_degrees = 90;
    await renderAnalysis(analysisGeneration([convertedRow(), portrait], "Ready"));
    await expect
      .element(page.getByRole("columnheader", { name: "Current file size" }))
      .toBeVisible();
    await expect.element(page.getByText("1080x1920", { exact: true })).toBeVisible();
    const converted = document.querySelector('[data-analysis-row="2"]');
    expect(converted?.querySelectorAll("td")[4]?.textContent).toBe("7.63 MB");
    expect(converted?.querySelectorAll("td")[5]?.textContent).toBe("7.63 MBMeasured output");
  });

  it("bounds rendering for 10,000 files, supports keyboard traversal, and retains selected hidden files during search", async () => {
    const tauri = installTauriMock({ analysis_enqueue: () => ({ added: 1, excluded: [] }) });
    await renderAnalysis(
      analysisGeneration(
        Array.from({ length: 10_000 }, (_, index) =>
          discoveredRow(index + 2, `video-${index}.mkv`),
        ),
      ),
    );
    await expect
      .poll(() => document.querySelectorAll("[data-analysis-row]").length)
      .toBeLessThan(40);
    await expect
      .element(page.getByRole("table", { name: "Analysis files" }))
      .toHaveAttribute("aria-rowcount", "10001");
    const first = page.getByRole("checkbox", { name: "Select video-0.mkv", exact: true });
    await first.click();
    await userEvent.keyboard("{End}");
    const last = page.getByRole("checkbox", { name: "Select video-9999.mkv", exact: true });
    await expect.element(last).toHaveFocus();
    await userEvent.keyboard(" ");
    await expect.element(last).toBeChecked();
    await userEvent.keyboard("{Home}");
    await expect.element(first).toHaveFocus();
    await page.getByRole("textbox", { name: "Search Analysis" }).fill("video-5000.mkv");
    await expect
      .element(page.getByRole("checkbox", { name: "Select video-5000.mkv", exact: true }))
      .toBeVisible();
    await page.getByRole("button", { name: "Analyze selected" }).click();
    expect(tauri.callsFor("analysis_enqueue")[0]?.payload).toEqual({
      request: {
        generation: 7,
        operation: "Analyze",
        selection: { Selected: { row_ids: [2, 10001] } },
      },
    });
    await page.getByRole("checkbox", { name: "Select visible files", exact: true }).click();
    await expect.element(page.getByText(/3 selected/)).toBeVisible();
    await page.getByRole("button", { name: "Clear selection" }).click();
    await expect.element(page.getByRole("button", { name: "Analyze selected" })).toBeDisabled();
  });

  it("retains selection, search, and scroll through view navigation without moving focus from another view", async () => {
    function RetainedView() {
      const [active, setActive] = useState(true);
      return (
        <div className="flex h-full min-h-0 flex-col">
          <button onClick={() => setActive(false)}>Show other view</button>
          <button onClick={() => setActive(true)}>Show Analysis view</button>
          <Activity mode={active ? "visible" : "hidden"}>
            <ViewActiveContext value={active}>
              <AnalysisView />
            </ViewActiveContext>
          </Activity>
        </div>
      );
    }
    await renderApp(<RetainedView />, {
      appState: { settings: analysisSettings(), tools: analysisTools() },
    });
    publish(
      analysisGeneration(
        Array.from({ length: 1000 }, (_, index) => discoveredRow(index + 2, `video-${index}.mkv`)),
      ),
    );
    await page.getByRole("checkbox", { name: "Select video-0.mkv", exact: true }).click();
    const scroll = document.querySelector<HTMLElement>("[data-analysis-scroll]");
    if (scroll === null) throw new Error("Analysis viewport missing");
    scroll.scrollTop = 320;
    await page.getByRole("button", { name: "Show other view" }).click();
    update([predictedRow(2, "video-0.mkv")]);
    await expect.element(page.getByRole("button", { name: "Show other view" })).toHaveFocus();
    await page.getByRole("button", { name: "Show Analysis view" }).click();
    expect(scroll.scrollTop).toBe(320);
    await page.getByRole("textbox", { name: "Search Analysis" }).fill("video-0.mkv");
    await expect
      .element(page.getByRole("checkbox", { name: "Select video-0.mkv", exact: true }))
      .toBeChecked();
    await page.getByRole("button", { name: "Show other view" }).click();
    await page.getByRole("button", { name: "Show Analysis view" }).click();
    await expect
      .element(page.getByRole("textbox", { name: "Search Analysis" }))
      .toHaveValue("video-0.mkv");
  });
});
