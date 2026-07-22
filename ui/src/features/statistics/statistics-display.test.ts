import { describe, expect, it } from "vitest";

import { statisticsPayload } from "@/test/fixtures/statistics";

import {
  codecRows,
  coverageMessage,
  cumulativeRows,
  formatEpochDay,
  formatSignedFileSize,
  hasStatisticsData,
  reductionRows,
  runOutcomeRows,
} from "./statistics-display";

const GIB = 1024 ** 3;

describe("Statistics display model", () => {
  it("does not treat remux-only, not-worthwhile-only, or run-only data as empty", () => {
    expect(hasStatisticsData(statisticsPayload())).toBe(false);
    expect(hasStatisticsData(statisticsPayload({ remuxed_files: 1 }))).toBe(true);
    expect(hasStatisticsData(statisticsPayload({ not_worthwhile_files: 1 }))).toBe(true);
    expect(hasStatisticsData(statisticsPayload({ runs: { failed: 1 } }))).toBe(true);
  });

  it("communicates partial conversion-size coverage without altering totals", () => {
    const partial = statisticsPayload({ converted_files: 5, sized_converted_files: 3 });
    expect(coverageMessage(partial)).toBe(
      "3 of 5 converted standings include both sizes; savings and reduction statistics cover only those files.",
    );
    expect(
      coverageMessage(statisticsPayload({ converted_files: 5, sized_converted_files: 5 })),
    ).toBeNull();
  });

  it("formats negative and positive savings without losing the sign", () => {
    expect(formatSignedFileSize(-GIB)).toBe("−1.00 GB");
    expect(formatSignedFileSize(GIB)).toBe("1.00 GB");
    expect(formatSignedFileSize(0)).toBe("0 B");
    expect(formatSignedFileSize(Number.NaN)).toBe("—");
  });

  it("keeps grew files outside the engine's non-negative bins", () => {
    const payload = statisticsPayload({
      reduction_bins: [1, 2, 0, 0, 0, 0, 0, 0, 0, 1],
      grew_count: 4,
    });
    const rows = reductionRows(payload.reduction_bins);
    expect(rows).toEqual([
      { label: "0–10%", files: 1 },
      { label: "10–20%", files: 2 },
      { label: "20–30%", files: 0 },
      { label: "30–40%", files: 0 },
      { label: "40–50%", files: 0 },
      { label: "50–60%", files: 0 },
      { label: "60–70%", files: 0 },
      { label: "70–80%", files: 0 },
      { label: "80–90%", files: 0 },
      { label: "90–100%", files: 1 },
    ]);
    expect(rows.reduce((total, row) => total + row.files, 0)).toBe(4);
    expect(payload.grew_count).toBe(4);
  });

  it("preserves backend codec order across ties and never invents Other", () => {
    const rows = codecRows([
      { codec: "H264", count: 4 },
      { codec: "Hevc", count: 4 },
      { codec: { Other: "MPEG-2" }, count: 1 },
    ]);
    expect(rows).toEqual([
      { label: "H.264", files: 4 },
      { label: "HEVC", files: 4 },
      { label: "MPEG-2", files: 1 },
    ]);
  });

  it("preserves daily order and downward cumulative movement", () => {
    const rows = cumulativeRows([
      { epoch_day: 20_000, cumulative_saved_bytes: 2 * GIB },
      { epoch_day: 20_001, cumulative_saved_bytes: -GIB },
      { epoch_day: 20_002, cumulative_saved_bytes: 3 * GIB },
    ]);
    expect(rows.map(({ date }) => date)).toEqual([
      formatEpochDay(20_000),
      formatEpochDay(20_001),
      formatEpochDay(20_002),
    ]);
    expect(rows.map(({ savedBytes }) => savedBytes)).toEqual([2 * GIB, -GIB, 3 * GIB]);
  });

  it("keeps every terminal run outcome distinct", () => {
    expect(
      runOutcomeRows({
        analyzed: 1,
        converted: 2,
        remuxed: 3,
        not_worthwhile: 4,
        stopped: 5,
        skipped: 6,
        failed: 7,
      }),
    ).toEqual([
      { label: "Analyzed", count: 1 },
      { label: "Converted", count: 2 },
      { label: "Remuxed", count: 3 },
      { label: "Not worthwhile", count: 4 },
      { label: "Stopped", count: 5 },
      { label: "Skipped", count: 6 },
      { label: "Failed", count: 7 },
    ]);
  });
});
