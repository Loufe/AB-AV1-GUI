import { describe, expect, it } from "vitest";

import type {
  AnalysisActivity,
  AnalysisDelta_Deserialize,
  AnalysisGeneration_Deserialize,
  AnalysisRow,
  AnalysisSnapshot_Deserialize,
} from "@/lib/bindings";
import { foldAnalysis, normalizeAnalysisSnapshot } from "@/lib/store/analysis-fold";
import fixturesJson from "@/lib/store/analysis-fixtures.json";
import { emptyAnalysisState } from "@/lib/store/analysis-store";

interface Scenario {
  name: string;
  deltas: AnalysisDelta_Deserialize[];
  expected: AnalysisSnapshot_Deserialize;
}

// resolveJsonModule infers wide literal types, so the generated file is cast
// to the binding types it was serialized from.
const fixtures = fixturesJson as unknown as { scenarios: Scenario[] };

function row(id: number, name: string): AnalysisRow {
  return {
    id,
    parent: null,
    entry: { File: { scan: "Discovered" } },
    display_name: { text: name, lossy: false },
    display_path: { text: `/videos/${name}`, lossy: false },
  };
}

function generation(
  id: number,
  rows: AnalysisRow[] = [],
  activity: AnalysisActivity = "Discovering",
): AnalysisGeneration_Deserialize {
  return {
    id,
    roots: [{ text: "/videos", lossy: false }],
    activity,
    rows,
  };
}

describe("foldAnalysis", () => {
  it("normalizes a complete reset and replaces the prior generation", () => {
    const state = normalizeAnalysisSnapshot({ current: generation(1, [row(1, "old.mkv")]) });

    const replaced = foldAnalysis(state, {
      Reset: { snapshot: { current: generation(2, [row(7, "new.mkv")], "Discovered") } },
    });

    expect(replaced).toEqual({
      current: {
        ...generation(2, [], "Discovered"),
        rows: { 7: row(7, "new.mkv") },
      },
    });
    expect(replaced.current?.rows[1]).toBeUndefined();
  });

  it("upserts a current batch by row id without mutating prior state", () => {
    const original = row(1, "before.mkv");
    const state = normalizeAnalysisSnapshot({ current: generation(4, [original]) });
    const replacement = row(1, "after.mkv");
    const inserted = row(2, "second.mkv");

    const next = foldAnalysis(state, {
      RowsUpserted: { generation: 4, rows: [replacement, inserted] },
    });

    expect(next.current?.rows).toEqual({ 1: replacement, 2: inserted });
    expect(state.current?.rows).toEqual({ 1: original });
  });

  it("ignores row and activity deltas from superseded generations", () => {
    const state = normalizeAnalysisSnapshot({ current: generation(9, [row(1, "current.mkv")]) });

    const afterRows = foldAnalysis(state, {
      RowsUpserted: { generation: 8, rows: [row(2, "stale.mkv")] },
    });
    const afterActivity = foldAnalysis(afterRows, {
      ActivityChanged: { generation: 8, activity: { Failed: { detail: "stale" } } },
    });

    expect(afterRows).toBe(state);
    expect(afterActivity).toBe(state);
  });

  it("updates current activity and ignores live deltas without a generation", () => {
    const empty = emptyAnalysisState();
    expect(
      foldAnalysis(empty, { RowsUpserted: { generation: 1, rows: [row(1, "ignored.mkv")] } }),
    ).toBe(empty);

    const state = normalizeAnalysisSnapshot({ current: generation(3) });
    const next = foldAnalysis(state, {
      ActivityChanged: { generation: 3, activity: "BasicScanning" },
    });
    expect(next.current?.activity).toBe("BasicScanning");
  });

  it("accepts an empty reset as the authoritative reconnect state", () => {
    const state = normalizeAnalysisSnapshot({ current: generation(1, [row(1, "old.mkv")]) });
    expect(foldAnalysis(state, { Reset: { snapshot: { current: null } } })).toEqual(
      emptyAnalysisState(),
    );
  });
});

describe("analysis fixtures", () => {
  it("replays every reducer-published scenario to the reducer's own snapshot", () => {
    expect(fixtures.scenarios.length).toBeGreaterThan(0);
    for (const scenario of fixtures.scenarios) {
      let state = emptyAnalysisState();
      for (const delta of scenario.deltas) {
        state = foldAnalysis(state, delta);
      }
      expect(state, scenario.name).toEqual(normalizeAnalysisSnapshot(scenario.expected));
    }
  });

  it("carries a status on every scanned row and none on discovered or failed rows", () => {
    for (const scenario of fixtures.scenarios) {
      const rows = scenario.expected.current?.rows ?? [];
      for (const row of rows) {
        if (!("File" in row.entry) || row.entry.File === undefined) continue;
        const scan = row.entry.File.scan;
        if (scan === "Discovered" || "Failed" in scan) continue;
        const status =
          "Scanned" in scan && scan.Scanned !== undefined
            ? scan.Scanned.status
            : "SettledOutput" in scan && scan.SettledOutput !== undefined
              ? scan.SettledOutput.status
              : undefined;
        expect(status?.historical, `${scenario.name}/${row.display_name.text}`).toBeDefined();
      }
    }
  });
});
