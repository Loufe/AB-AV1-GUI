import { describe, expect, it } from "vitest";

import type { AnalysisActivity, AnalysisGeneration_Deserialize, AnalysisRow } from "@/lib/bindings";
import { foldAnalysis, normalizeAnalysisSnapshot } from "@/lib/store/analysis-fold";
import { emptyAnalysisState } from "@/lib/store/analysis-store";

function row(id: number, name: string): AnalysisRow {
  return {
    id,
    parent: null,
    kind: "File",
    display_name: { text: name, lossy: false },
    display_path: { text: `/videos/${name}`, lossy: false },
    directory_failure: null,
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
