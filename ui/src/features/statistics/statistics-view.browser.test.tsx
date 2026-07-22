import { page } from "vitest/browser";
import { describe, expect, it } from "vitest";

import { ViewActiveContext } from "@/components/layout/view-activity";
import type { StatisticsPayload } from "@/lib/bindings";
import { appStore } from "@/lib/store/app-store";
import { renderApp } from "@/test/browser/render";
import { installTauriMock, type TauriMock } from "@/test/browser/tauri";
import { statisticsPayload } from "@/test/fixtures/statistics";

import { StatisticsView } from "./statistics-view";
import { currentUtcOffsetMinutes } from "./use-statistics-request";

const GIB = 1024 ** 3;

function StatisticsHarness({ active }: { active: boolean }) {
  return (
    <ViewActiveContext value={active}>
      <StatisticsView />
    </ViewActiveContext>
  );
}

function setStatistics(statistics: StatisticsPayload | null): void {
  appStore.setState((state) => ({ ...state, statistics }));
}

function clearStatisticsForSnapshot(): void {
  appStore.setState((state) => ({
    ...state,
    statistics: null,
    snapshotGeneration: state.snapshotGeneration + 1,
  }));
}

function requestPayloads(tauri: TauriMock): unknown[] {
  return tauri.callsFor("request_statistics").map(({ payload }) => payload);
}

function richPayload(offset = currentUtcOffsetMinutes()): StatisticsPayload {
  return statisticsPayload({
    utc_offset_minutes: offset,
    converted_files: 2,
    sized_converted_files: 1,
    remuxed_files: 1,
    not_worthwhile_files: 3,
    total_input_bytes: 3 * GIB,
    total_output_bytes: 4 * GIB,
    total_saved_bytes: -GIB,
    remux_saved_bytes: GIB / 2,
    total_time_ms: 3_600_000,
    gigabytes_per_hour: 3,
    reduction_bins: [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    grew_count: 1,
    codecs: [
      { codec: "H264", count: 1 },
      { codec: "Hevc", count: 1 },
    ],
    cumulative_savings: [
      { epoch_day: 20_000, cumulative_saved_bytes: GIB },
      { epoch_day: 20_001, cumulative_saved_bytes: -GIB },
    ],
    first_epoch_day: 20_000,
    last_epoch_day: 20_001,
    runs: { converted: 2, remuxed: 1, not_worthwhile: 1, failed: 1 },
  });
}

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

describe("StatisticsView request lifecycle", () => {
  it("keeps acknowledgement separate from the later sequenced response", async () => {
    const tauri = installTauriMock({ request_statistics: () => null });
    const expectedOffset = -new Date().getTimezoneOffset();
    await renderApp(<StatisticsHarness active />);

    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(1);
    expect(requestPayloads(tauri)).toEqual([{ utcOffsetMinutes: expectedOffset }]);
    await expect.element(page.getByText("Loading statistics")).toBeVisible();
    await expect.element(page.getByText("Converted standings")).not.toBeInTheDocument();

    setStatistics(richPayload());

    await expect.element(page.getByRole("heading", { name: "Statistics" })).toBeVisible();
    await expect.element(page.getByText("Loading statistics")).not.toBeInTheDocument();
    await expect.element(page.getByText("Converted standings").first()).toBeVisible();
  });

  it("retains valid data through refresh acknowledgement and a later response", async () => {
    const acknowledgement = deferred<null>();
    const tauri = installTauriMock({ request_statistics: () => acknowledgement.promise });
    const initial = richPayload();
    await renderApp(<StatisticsHarness active />, { appState: { statistics: initial } });

    await expect.element(page.getByText("Refreshing statistics…")).toBeVisible();
    await expect.element(page.getByText("Conversion net savings").first()).toBeVisible();
    acknowledgement.resolve(null);
    await expect.element(page.getByText("Refreshing statistics…")).toBeVisible();

    setStatistics({ ...initial, total_saved_bytes: 2 * GIB });

    await expect.element(page.getByText("Refreshing statistics…")).not.toBeInTheDocument();
    await expect.element(page.getByText("2.00 GB").first()).toBeVisible();
    expect(tauri.callsFor("request_statistics")).toHaveLength(1);
  });

  it("shows a refresh rejection without discarding the last valid payload", async () => {
    const tauri = installTauriMock();
    tauri.rejectCommand("request_statistics", {
      code: "engine_unavailable",
      message: "projection worker stopped",
    });
    await renderApp(<StatisticsHarness active />, {
      appState: { statistics: richPayload() },
    });

    await expect
      .element(page.getByRole("alert"))
      .toHaveTextContent(
        "statistics request failed (engine_unavailable): projection worker stopped Showing the last valid response.",
      );
    await expect.element(page.getByText("Conversion net savings").first()).toBeVisible();
  });

  it("requests on activation and focus regain only while active", async () => {
    const tauri = installTauriMock({ request_statistics: () => null });
    const rendered = await renderApp(<StatisticsHarness active={false} />);

    window.dispatchEvent(new Event("focus"));
    expect(tauri.callsFor("request_statistics")).toHaveLength(0);

    await rendered.rerender(<StatisticsHarness active />);
    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(1);
    setStatistics(richPayload());
    await expect.element(page.getByText("Converted standings").first()).toBeVisible();

    window.dispatchEvent(new Event("focus"));
    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(2);

    await rendered.rerender(<StatisticsHarness active={false} />);
    window.dispatchEvent(new Event("focus"));
    expect(tauri.callsFor("request_statistics")).toHaveLength(2);
  });

  it("hides a timezone-mismatched answer until a current response arrives", async () => {
    const tauri = installTauriMock({ request_statistics: () => null });
    const currentOffset = currentUtcOffsetMinutes();
    await renderApp(<StatisticsHarness active />, {
      appState: { statistics: richPayload(currentOffset + 60) },
    });

    await expect.element(page.getByText("Loading statistics")).toBeVisible();
    await expect.element(page.getByText("Conversion net savings")).not.toBeInTheDocument();
    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(1);
    expect(requestPayloads(tauri)).toEqual([{ utcOffsetMinutes: currentOffset }]);

    setStatistics(richPayload(currentOffset));
    await expect.element(page.getByText("Conversion net savings").first()).toBeVisible();
  });

  it("re-requests when a snapshot clears the non-replayed answer", async () => {
    const tauri = installTauriMock({ request_statistics: () => null });
    const initial = richPayload();
    await renderApp(<StatisticsHarness active />, { appState: { statistics: initial } });
    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(1);

    setStatistics({ ...initial });
    await expect.element(page.getByText("Refreshing statistics…")).not.toBeInTheDocument();
    clearStatisticsForSnapshot();

    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(2);
    await expect.element(page.getByText("Loading statistics")).toBeVisible();
  });

  it("re-requests after reconnect even when Statistics was already null", async () => {
    const tauri = installTauriMock({ request_statistics: () => null });
    await renderApp(<StatisticsHarness active />);
    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(1);

    clearStatisticsForSnapshot();

    await expect.poll(() => tauri.callsFor("request_statistics").length).toBe(2);
    await expect.element(page.getByText("Loading statistics")).toBeVisible();
  });
});

describe("StatisticsView payload presentation", () => {
  it("shows partial and negative conversion facts without fabricating absent spreads", async () => {
    installTauriMock({ request_statistics: () => null });
    await renderApp(<StatisticsHarness active />, { appState: { statistics: richPayload() } });

    await expect.element(page.getByText("−1.00 GB").first()).toBeVisible();
    await expect
      .element(
        page.getByText(
          "1 of 2 converted standings include both sizes; savings and reduction statistics cover only those files.",
        ),
      )
      .toBeVisible();
    await expect
      .element(page.getByText("1 converted outputs grew and are not included in these bins."))
      .toBeVisible();
    await expect.element(page.getByText("Average reduction")).not.toBeInTheDocument();
    await expect.element(page.getByText("Average VMAF")).not.toBeInTheDocument();
    await expect.element(page.getByText("Average CRF")).not.toBeInTheDocument();

    await page.getByText("View codec counts").click();
    await expect.element(page.getByText("H.264").last()).toBeVisible();
    await expect.element(page.getByText("HEVC").last()).toBeVisible();
    await expect.element(page.getByText("Other", { exact: true })).not.toBeInTheDocument();

    await page.getByText("View daily values").click();
    await expect.element(page.getByText("−1.00 GB").last()).toBeVisible();
  });

  it("presents already-normalized averages without fixed-point conversion", async () => {
    installTauriMock({ request_statistics: () => null });
    const payload = statisticsPayload({
      utc_offset_minutes: currentUtcOffsetMinutes(),
      converted_files: 1,
      sized_converted_files: 1,
      reduction_percent: { average: 42.5, minimum: 40, maximum: 45, count: 1 },
      vmaf: { average: 95.1, minimum: 95.1, maximum: 95.1, count: 1 },
      crf: { average: 24, minimum: 24, maximum: 24, count: 1 },
    });
    await renderApp(<StatisticsHarness active />, { appState: { statistics: payload } });

    await expect.element(page.getByText("Average reduction")).toBeVisible();
    await expect.element(page.getByText("42.5%").first()).toBeVisible();
    await expect.element(page.getByText("Average VMAF")).toBeVisible();
    await expect.element(page.getByText("95.1").first()).toBeVisible();
    await expect.element(page.getByText("Average CRF")).toBeVisible();
    await expect.element(page.getByText("24").first()).toBeVisible();
    await expect.element(page.getByText("0.0%", { exact: true })).not.toBeInTheDocument();
  });

  it("renders remux-only and not-worthwhile-only answers as real data", async () => {
    installTauriMock({ request_statistics: () => null });
    const remuxOnly = statisticsPayload({
      utc_offset_minutes: currentUtcOffsetMinutes(),
      remuxed_files: 2,
      remux_saved_bytes: GIB,
    });
    const rendered = await renderApp(<StatisticsHarness active />, {
      appState: { statistics: remuxOnly },
    });

    await expect.element(page.getByText("Outcomes and coverage")).toBeVisible();
    await expect.element(page.getByText("No statistics yet")).not.toBeInTheDocument();
    await expect.element(page.getByText("Remux savings")).toBeVisible();

    const notWorthwhileOnly = statisticsPayload({
      utc_offset_minutes: currentUtcOffsetMinutes(),
      not_worthwhile_files: 4,
    });
    setStatistics(notWorthwhileOnly);
    await rendered.rerender(<StatisticsHarness active />);

    await expect.element(page.getByText("Outcomes and coverage")).toBeVisible();
    await expect.element(page.getByText("No statistics yet")).not.toBeInTheDocument();
    await expect.element(page.getByText("Not worthwhile").first()).toBeVisible();
  });
});
