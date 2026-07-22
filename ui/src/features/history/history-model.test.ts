import { describe, expect, it } from "vitest";

import type { DurableState_Deserialize, HistoryRow, HistoryStatus } from "@/lib/bindings";
import fixturesJson from "@/lib/projection/projection-fixtures.json";
import { emptyDurableState } from "@/lib/store/fold";

import {
  audioSummary,
  compareHistoryDefault,
  historyDisplayRows,
  historyRowId,
  historyTotals,
  reductionPercent,
  statusPresentation,
} from "./history-model";

interface Scenario {
  name: string;
  state: DurableState_Deserialize;
  expected_rows: HistoryRow[];
}

const fixtures = fixturesJson as unknown as { scenarios: Scenario[] };

function scenario(name: string): Scenario {
  const found = fixtures.scenarios.find((candidate) => candidate.name === name);
  if (found === undefined) throw new Error(`missing projection fixture ${name}`);
  return found;
}

function historyRow(
  key: string,
  status: HistoryStatus = "Converted",
  overrides: Partial<HistoryRow> = {},
): HistoryRow {
  return {
    key: { kind: "Content", value: key },
    status,
    source_run: null,
    happened_at: 1_000,
    codec: null,
    container: null,
    width: null,
    height: null,
    duration_ms: null,
    audio: null,
    input_size_bytes: null,
    output_size_bytes: null,
    encoding_time_ms: null,
    vmaf: null,
    crf: null,
    ...overrides,
  };
}

describe("History display identity and provenance", () => {
  it("serializes the tagged key so equal values in different arms cannot collide", () => {
    expect(historyRowId({ kind: "Content", value: "same" })).not.toBe(
      historyRowId({ kind: "Parked", value: "same" }),
    );
  });

  it("resolves native run input before all fallback labels", () => {
    const fixture = scenario("converted_with_live_evidence");
    const [display] = historyDisplayRows(fixture.expected_rows, fixture.state);
    expect(display).toMatchObject({
      label: "videos/input-1.mkv",
      basename: "input-1.mkv",
      provenance: "native",
    });
    expect(display?.label).not.toContain("content-0001");
  });

  it("uses retained adopted provenance when no source run survives", () => {
    const fixture = scenario("adopted_imported_history");
    const [display] = historyDisplayRows(fixture.expected_rows, fixture.state);
    expect(display).toMatchObject({
      label: "c:/history/adopted.mkv",
      basename: "adopted.mkv",
      provenance: "adopted",
    });
  });

  it("uses unresolved parked paths and never invents scanned rows", () => {
    const fixture = scenario("parked_imported_history");
    const displayed = historyDisplayRows(fixture.expected_rows, fixture.state);
    expect(displayed).toHaveLength(3);
    expect(displayed.map((row) => row.basename)).toEqual([
      "analyzed.mkv",
      "converted.mkv",
      "declined.mkv",
    ]);
    expect(displayed.every((row) => row.provenance === "parked")).toBe(true);
    expect(displayed.some((row) => row.label.includes("scanned"))).toBe(false);
  });

  it("uses a neutral label when neither run nor import provenance exists", () => {
    const [display] = historyDisplayRows([historyRow("opaque-content-key")], emptyDurableState());
    expect(display).toMatchObject({ label: "Unknown file", path: null, provenance: "unknown" });
  });
});

describe("History standing vocabulary", () => {
  it("covers every projected status without introducing Skipped", () => {
    const statuses: HistoryStatus[] = [
      "Converted",
      "Remuxed",
      { NotWorthwhile: { requested: 95, floor: 90 } },
      "Analyzed",
      { Failed: { kind: "SearchRun", message: "anonymized failure" } },
      "Stopped",
    ];
    expect(statuses.map((status) => statusPresentation(status).label)).toEqual([
      "Converted",
      "Remuxed",
      "Not Worthwhile",
      "Analyzed",
      "Failed",
      "Stopped",
    ]);
  });

  it("describes Analyzed historically without promising reusable CRF", () => {
    const presentation = statusPresentation("Analyzed");
    expect(presentation.detail).toContain("Historical analysis result");
    expect(presentation.detail).not.toContain("skip");
    expect(presentation.detail).not.toContain("cached");
  });

  it("preserves failure and not-worthwhile facts", () => {
    expect(statusPresentation({ NotWorthwhile: { requested: 96, floor: 89 } }).detail).toContain(
      "96 through 89",
    );
    expect(
      statusPresentation({ Failed: { kind: "EncodeRun", message: "anonymized stderr" } }).detail,
    ).toBe("anonymized stderr");
  });
});

describe("History sparse values, sorting, and totals", () => {
  it("calculates reduction only from a valid complete size pair and keeps growth negative", () => {
    expect(
      reductionPercent(
        historyRow("smaller", "Converted", {
          input_size_bytes: 1_000,
          output_size_bytes: 400,
        }),
      ),
    ).toBe(60);
    expect(
      reductionPercent(
        historyRow("grew", "Converted", {
          input_size_bytes: 1_000,
          output_size_bytes: 1_250,
        }),
      ),
    ).toBe(-25);
    expect(
      reductionPercent(
        historyRow("zero", "Converted", {
          input_size_bytes: 0,
          output_size_bytes: 0,
        }),
      ),
    ).toBeNull();
    expect(reductionPercent(historyRow("sparse"))).toBeNull();
  });

  it("sorts newest first, null dates last, and ties by tagged key", () => {
    const state = emptyDurableState();
    const displayed = historyDisplayRows(
      [
        historyRow("b", "Stopped", { happened_at: 2_000 }),
        historyRow("null", "Stopped", { happened_at: null }),
        historyRow("a", "Stopped", { happened_at: 2_000 }),
        historyRow("older", "Stopped", { happened_at: 1_000 }),
      ],
      state,
    ).sort(compareHistoryDefault);
    expect(displayed.map((row) => row.row.key.value)).toEqual(["a", "b", "older", "null"]);
  });

  it("totals only complete size pairs from already-projected rows", () => {
    const displayed = historyDisplayRows(
      [
        historyRow("smaller", "Converted", {
          input_size_bytes: 1_000,
          output_size_bytes: 400,
        }),
        historyRow("grew", "Remuxed", {
          input_size_bytes: 1_000,
          output_size_bytes: 1_250,
        }),
        historyRow("sparse", "Converted", { input_size_bytes: 900 }),
      ],
      emptyDurableState(),
    );
    expect(historyTotals(displayed)).toEqual({
      records: 3,
      sizedRecords: 2,
      inputBytes: 2_000,
      outputBytes: 1_650,
      savedBytes: 350,
    });
  });

  it("distinguishes missing audio metadata from a probed file with no audio", () => {
    expect(audioSummary(null)).toBe("—");
    expect(audioSummary([])).toBe("No audio");
    expect(audioSummary(["Aac", { Other: "wmav2" }])).toBe("AAC, WMAV2");
  });
});
