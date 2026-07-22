import { page, userEvent } from "vitest/browser";
import { describe, expect, it } from "vitest";

import type { DurableState_Deserialize, HistoryRow, HistoryStatus, Settings } from "@/lib/bindings";
import fixturesJson from "@/lib/projection/projection-fixtures.json";
import { emptyDurableState } from "@/lib/store/fold";
import { renderApp } from "@/test/browser/render";
import { installTauriMock } from "@/test/browser/tauri";

import { historyDisplayRows, type HistoryDisplayRow } from "./history-model";
import { HistoryTable } from "./history-table";
import { HistoryView } from "./history-view";

interface Scenario {
  name: string;
  state: DurableState_Deserialize;
  expected_rows: HistoryRow[];
}

const fixtures = fixturesJson as unknown as { scenarios: Scenario[] };

function settings(): Settings {
  return {
    last_input_folder: null,
    scan_extensions: ["mkv", "mp4"],
    output: {
      default_mode: "replace",
      suffix: "_av1",
      separate_folder: null,
      overwrite_existing: false,
    },
    hardware_decode: true,
    privacy: { anonymize_logs: true, anonymize_history: true },
    log_folder: null,
  };
}

function historyRow(
  name: string,
  status: HistoryStatus,
  overrides: Partial<HistoryRow> = {},
): HistoryRow {
  return {
    key: { kind: "Parked", value: `c:/anonymized/${name}` },
    status,
    source_run: null,
    happened_at: 1_000,
    codec: "Hevc",
    container: "Matroska",
    width: 1920,
    height: 1080,
    duration_ms: 600_000,
    audio: ["Aac"],
    input_size_bytes: 10_000_000,
    output_size_bytes: 4_000_000,
    encoding_time_ms: 240_000,
    vmaf: 9_512,
    crf: 24_000,
    ...overrides,
  };
}

function displayRows(rows: HistoryRow[]): HistoryDisplayRow[] {
  return historyDisplayRows(rows, emptyDurableState());
}

function statusMatrix(): HistoryDisplayRow[] {
  return displayRows([
    historyRow("converted.mkv", "Converted", { happened_at: 6_000 }),
    historyRow("remuxed.mkv", "Remuxed", { happened_at: 5_000 }),
    historyRow(
      "declined.mkv",
      { NotWorthwhile: { requested: 95, floor: 90 } },
      {
        happened_at: 4_000,
        output_size_bytes: null,
      },
    ),
    historyRow("analyzed.mkv", "Analyzed", {
      happened_at: 3_000,
      output_size_bytes: null,
      encoding_time_ms: null,
    }),
    historyRow(
      "failed.mkv",
      {
        Failed: { kind: "SearchRun", message: "anonymized search failure" },
      },
      {
        happened_at: 2_000,
        output_size_bytes: null,
        vmaf: null,
        crf: null,
      },
    ),
    historyRow("stopped.mkv", "Stopped", {
      happened_at: null,
      output_size_bytes: null,
      vmaf: null,
      crf: null,
    }),
    historyRow("grew.mkv", "Converted", {
      happened_at: 7_000,
      input_size_bytes: 4_000_000,
      output_size_bytes: 5_000_000,
    }),
  ]);
}

describe("production History states", () => {
  it("distinguishes loading-before-snapshot from genuinely empty History", async () => {
    const loading = await renderApp(<HistoryView />);
    await expect.element(page.getByText("Loading history…")).toBeVisible();
    await expect.element(page.getByText("No records yet")).not.toBeInTheDocument();
    await loading.unmount();

    await renderApp(<HistoryView />, { appState: { settings: settings() } });
    await expect.element(page.getByText("No records yet")).toBeVisible();
    await expect.element(page.getByText("Loading history…")).not.toBeInTheDocument();
  });

  it("renders the existing projection's adopted and parked sparse facts", async () => {
    const parkedFixture = fixtures.scenarios.find(
      (candidate) => candidate.name === "parked_imported_history",
    );
    if (parkedFixture === undefined) throw new Error("missing parked projection fixture");
    const parked = await renderApp(<HistoryView />, {
      appState: { settings: settings(), durable: parkedFixture.state },
    });

    await expect.element(page.getByRole("table")).toBeVisible();
    await expect.element(page.getByText("analyzed.mkv", { exact: true })).toBeVisible();
    await expect.element(page.getByText("converted.mkv", { exact: true })).toBeVisible();
    await expect.element(page.getByText("declined.mkv", { exact: true })).toBeVisible();
    await expect.element(page.getByText("scanned.mkv", { exact: true })).not.toBeInTheDocument();
    await expect.element(page.getByText("Unresolved import").first()).toBeVisible();
    await expect.element(page.getByText("VMAF 95.5 · CRF 30").first()).toBeVisible();
    await parked.unmount();

    const adoptedFixture = fixtures.scenarios.find(
      (candidate) => candidate.name === "adopted_imported_history",
    );
    if (adoptedFixture === undefined) throw new Error("missing adopted projection fixture");
    await renderApp(<HistoryView />, {
      appState: { settings: settings(), durable: adoptedFixture.state },
    });
    await expect.element(page.getByText("adopted.mkv", { exact: true })).toBeVisible();
    await expect.element(page.getByText("Imported", { exact: true })).toBeVisible();
    expect(document.body.textContent).not.toContain("cached");
    expect(document.body.textContent).not.toContain("skip the search");
  });

  it("renders a recovered row's absent facts without exposing its content key", async () => {
    const fixture = fixtures.scenarios.find(
      (candidate) => candidate.name === "converted_verdict_without_run",
    );
    if (fixture === undefined) throw new Error("missing sparse projection fixture");
    await renderApp(<HistoryView />, {
      appState: { settings: settings(), durable: fixture.state },
    });

    const unknown = page.getByText("Unknown file", { exact: true });
    await expect.element(unknown).toBeVisible();
    const row = unknown.element().closest("tr");
    expect(row?.textContent).not.toContain("adopted");
    expect(row?.textContent).toContain("Converted");
    expect(row?.textContent).toContain("—");
    expect(row?.querySelector("button[aria-label^='Actions for']")).toBeNull();
  });
});

describe("History table interaction and semantics", () => {
  it("filters and searches exact standing statuses while retaining semantic sorting", async () => {
    await renderApp(<HistoryTable rows={statusMatrix()} />);

    await expect.element(page.getByRole("table")).toBeVisible();
    const dateHeader = page.getByRole("columnheader", { name: /Date/ });
    await expect.element(dateHeader).toHaveAttribute("aria-sort", "descending");
    const visibleNames = [...document.querySelectorAll<HTMLElement>("[data-history-row]")].map(
      (row) => row.textContent,
    );
    expect(visibleNames.at(0)).toContain("grew.mkv");
    expect(visibleNames.at(-1)).toContain("stopped.mkv");
    await expect.element(page.getByRole("button", { name: "All · 7" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Converted · 2" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Remuxed · 1" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Not Worthwhile · 1" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Analyzed · 1" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Failed · 1" })).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Stopped · 1" })).toBeVisible();
    expect(document.body.textContent).not.toContain("Skipped");

    await page.getByRole("textbox", { name: "Search History" }).fill("GREW.MKV");
    await expect.element(page.getByText("grew.mkv", { exact: true })).toBeVisible();
    await expect.element(page.getByText("converted.mkv", { exact: true })).not.toBeInTheDocument();
    await page.getByRole("textbox", { name: "Search History" }).fill("");

    await page.getByRole("button", { name: "Failed · 1" }).click();
    await expect.element(page.getByText("failed.mkv", { exact: true })).toBeVisible();
    await expect.element(page.getByText("grew.mkv", { exact: true })).not.toBeInTheDocument();
    await page.getByRole("button", { name: "Failed · 1" }).click();

    const dateSort = page.getByRole("button", { name: /Sort by Date/ });
    dateSort.element().focus();
    await userEvent.keyboard("{Enter}");
    await expect.element(dateHeader).toHaveAttribute("aria-sort", "ascending");
  });

  it("keeps keyboard selection attached to domain identity through sorting", async () => {
    await renderApp(<HistoryTable rows={statusMatrix()} />);
    const stoppedText = page.getByText("stopped.mkv", { exact: true });
    const stoppedRow = stoppedText.element().closest("tr");
    expect(stoppedRow).not.toBeNull();
    if (stoppedRow === null) return;

    stoppedRow.focus();
    await expect.element(stoppedRow).toHaveFocus();
    await userEvent.keyboard(" ");
    expect(stoppedRow.dataset.selected).toBe("true");

    page
      .getByRole("button", { name: /Sort by File/ })
      .element()
      .click();
    const selectedAfterSort = page
      .getByText("stopped.mkv", { exact: true })
      .element()
      .closest("tr");
    expect(selectedAfterSort?.dataset.selected).toBe("true");
    await expect.element(selectedAfterSort as HTMLTableRowElement).toHaveFocus();

    const convertedFilter = page.getByRole("button", { name: "Converted · 2" });
    convertedFilter.element().click();
    await expect.element(page.getByText("stopped.mkv", { exact: true })).not.toBeInTheDocument();
    await expect.element(convertedFilter).toHaveAttribute("aria-pressed", "true");
    convertedFilter.element().click();
    await expect.element(page.getByText("stopped.mkv", { exact: true })).toBeVisible();
    const selectedAfterFilter = page
      .getByText("stopped.mkv", { exact: true })
      .element()
      .closest("tr");
    expect(selectedAfterFilter?.dataset.selected).toBe("true");
    await expect.element(selectedAfterFilter as HTMLTableRowElement).toHaveFocus();
  });

  it("makes full paths and only meaningful parked actions keyboard reachable", async () => {
    const tauri = installTauriMock();
    tauri.acceptCommand("open_path");
    tauri.acceptCommand("reveal_in_file_manager");

    const [parkedBase, nativeBase] = displayRows([
      historyRow("parked.mkv", "Analyzed"),
      historyRow("native.mkv", "Converted"),
    ]);
    if (parkedBase === undefined || nativeBase === undefined) throw new Error("missing test rows");
    const native = { ...nativeBase, provenance: "native" as const };
    await renderApp(<HistoryTable rows={[parkedBase, native]} />);

    await expect.element(page.getByText("c:/anonymized/native.mkv", { exact: true })).toBeVisible();
    const nativeActions = page.getByRole("button", { name: "Actions for native.mkv" });
    nativeActions.element().focus();
    await userEvent.keyboard("{Enter}");
    await expect.element(page.getByRole("menuitem", { name: "Open file" })).toBeVisible();
    await page.getByRole("menuitem", { name: "Open file" }).click();
    await expect.poll(() => tauri.callsFor("open_path").length).toBe(1);
    expect(tauri.callsFor("open_path").at(0)?.payload).toMatchObject({
      path: "c:/anonymized/native.mkv",
    });

    await page.getByRole("button", { name: "Actions for parked.mkv" }).click();
    await expect.element(page.getByRole("menuitem", { name: "Open file" })).not.toBeInTheDocument();
    await page.getByRole("menuitem", { name: "Reveal in file manager" }).click();
    await expect.poll(() => tauri.callsFor("reveal_in_file_manager").length).toBe(1);
    expect(tauri.callsFor("reveal_in_file_manager").at(0)?.payload).toMatchObject({
      path: "c:/anonymized/parked.mkv",
    });
  });

  it("virtualizes large histories and restores selected identity after recycling", async () => {
    const rows = displayRows(
      Array.from({ length: 500 }, (_, index) =>
        historyRow(`virtual-${String(index).padStart(3, "0")}.mkv`, "Converted", {
          happened_at: index,
        }),
      ),
    );
    await renderApp(<HistoryTable rows={rows} />);
    const scroll = document.querySelector<HTMLElement>("[data-history-scroll]");
    expect(scroll).not.toBeNull();
    if (scroll === null) return;

    expect(document.querySelectorAll("[data-history-row]").length).toBeLessThan(100);
    const newest = page.getByText("virtual-499.mkv", { exact: true });
    const newestRow = newest.element().closest("tr");
    expect(newestRow).not.toBeNull();
    if (newestRow === null) return;
    newestRow.focus();
    await userEvent.keyboard(" ");
    expect(newestRow.dataset.selected).toBe("true");
    await expect.element(newestRow).toHaveFocus();

    scroll.scrollTop = scroll.scrollHeight;
    scroll.dispatchEvent(new Event("scroll"));
    await expect.element(page.getByText("virtual-000.mkv", { exact: true })).toBeVisible();
    await expect
      .element(page.getByText("virtual-499.mkv", { exact: true }))
      .not.toBeInTheDocument();
    expect(document.querySelectorAll("[data-history-row]").length).toBeLessThan(100);

    scroll.scrollTop = 0;
    scroll.dispatchEvent(new Event("scroll"));
    await expect.element(page.getByText("virtual-499.mkv", { exact: true })).toBeVisible();
    expect(
      page.getByText("virtual-499.mkv", { exact: true }).element().closest("tr")?.dataset.selected,
    ).toBe("true");
    const restoredRow = page.getByText("virtual-499.mkv", { exact: true }).element().closest("tr");
    await expect.element(restoredRow as HTMLTableRowElement).toHaveFocus();
  });
});
