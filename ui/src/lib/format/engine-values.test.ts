import { describe, expect, it } from "vitest";

import type { HistoryRow, StatisticsPayload } from "@/lib/bindings";

import {
  formatDurationMsClock,
  formatDurationMsCompact,
  formatEngineCrf,
  formatEngineVmafScore,
  formatStatisticsCrf,
  formatStatisticsInputThroughput,
  formatStatisticsReduction,
  formatStatisticsVmaf,
  formatUnixMillisDate,
  formatVmafTarget,
} from "./engine-values";

// Values mirror converted_with_live_evidence in projection-fixtures.json.
const HISTORY_WIRE_VALUES = {
  crf: 24_000,
  vmaf: 9_512,
  encoding_time_ms: 240_000,
  happened_at: 1_728_003_600_000,
} satisfies Pick<HistoryRow, "crf" | "vmaf" | "encoding_time_ms" | "happened_at">;

// Statistics spreads have already crossed the fixed-point boundary in Rust.
const STATISTICS_WIRE_VALUES = {
  crf: { average: 24, minimum: 24, maximum: 24, count: 1 },
  vmaf: { average: 95.12, minimum: 95.12, maximum: 95.12, count: 1 },
  reduction_percent: { average: -2.25, minimum: -2.25, maximum: -2.25, count: 1 },
  gigabytes_per_hour: 111.76,
} satisfies Pick<StatisticsPayload, "crf" | "vmaf" | "reduction_percent" | "gigabytes_per_hour">;

describe("serialized fixed-point values", () => {
  it("formats History CRF and VMAF after scaling each exactly once", () => {
    expect(formatEngineCrf(HISTORY_WIRE_VALUES.crf)).toBe("24");
    expect(formatEngineVmafScore(HISTORY_WIRE_VALUES.vmaf)).toBe("95.1");
  });

  it("preserves fractional CRF precision after scaling", () => {
    expect(formatEngineCrf(24_250)).toBe("24.25");
  });

  it("uses each human formatter's intentional unavailable placeholder", () => {
    for (const invalid of [null, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(formatEngineCrf(invalid)).toBe("?");
      expect(formatEngineVmafScore(invalid)).toBe("—");
    }
  });
});

describe("serialized millisecond values", () => {
  it("converts a duration from milliseconds, not an epoch instant", () => {
    expect(formatDurationMsCompact(HISTORY_WIRE_VALUES.encoding_time_ms)).toBe("4m");
    expect(formatDurationMsClock(HISTORY_WIRE_VALUES.encoding_time_ms)).toBe("4:00");
  });

  it("keeps UnixMillis as epoch milliseconds", () => {
    const instant = HISTORY_WIRE_VALUES.happened_at;
    const expected = new Date(instant);
    const month = String(expected.getMonth() + 1).padStart(2, "0");
    const day = String(expected.getDate()).padStart(2, "0");
    expect(formatUnixMillisDate(instant)).toBe(`${expected.getFullYear()}-${month}-${day}`);
  });

  it("renders unavailable or invalid durations and instants consistently", () => {
    for (const invalid of [null, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(formatDurationMsCompact(invalid)).toBe("—");
      expect(formatDurationMsClock(invalid)).toBe("—");
      expect(formatUnixMillisDate(invalid)).toBe("—");
    }
  });
});

describe("already-normalized engine values", () => {
  it("does not scale a VMAF target", () => {
    expect(formatVmafTarget(95)).toBe("95");
  });

  it("does not divide Statistics CRF or VMAF spreads again", () => {
    expect(formatStatisticsCrf(STATISTICS_WIRE_VALUES.crf.average)).toBe("24");
    expect(formatStatisticsVmaf(STATISTICS_WIRE_VALUES.vmaf.average)).toBe("95.1");
  });

  it("preserves the sign of a normalized negative reduction", () => {
    expect(formatStatisticsReduction(STATISTICS_WIRE_VALUES.reduction_percent.average)).toBe(
      "-2.3%",
    );
  });

  it("formats the authoritative input throughput directly", () => {
    expect(formatStatisticsInputThroughput(STATISTICS_WIRE_VALUES.gigabytes_per_hour)).toBe(
      "112 GiB/h",
    );
  });

  it("keeps unavailable and invalid normalized values intentional", () => {
    expect(formatVmafTarget(null)).toBe("—");
    expect(formatVmafTarget(-1)).toBe("—");
    expect(formatVmafTarget(Number.NaN)).toBe("—");
    expect(formatStatisticsCrf(Number.NaN)).toBe("?");
    expect(formatStatisticsVmaf(-1)).toBe("—");
    expect(formatStatisticsVmaf(Number.POSITIVE_INFINITY)).toBe("—");
    expect(formatStatisticsReduction(null)).toBe("—");
    expect(formatStatisticsInputThroughput(null)).toBe("—");
    expect(formatStatisticsInputThroughput(-1)).toBe("—");
    expect(formatStatisticsInputThroughput(Number.POSITIVE_INFINITY)).toBe("—");
  });
});
