import { describe, expect, it } from "vitest";

import {
  formatAudioCodecs,
  formatBitrate,
  formatCompactTime,
  formatCrf,
  formatDate,
  formatEfficiency,
  formatFileSize,
  formatReductionPercent,
  formatResolution,
  formatStreamDisplay,
  formatTime,
  formatVmaf,
  type TimeConfidence,
} from "./format";
import fixtures from "./parity-fixtures.json";

describe("formatCompactTime (V2 parity)", () => {
  for (const c of fixtures.formatCompactTime) {
    it(`${c.seconds}s, ${c.confidence} → ${c.expected}`, () => {
      expect(formatCompactTime(c.seconds, c.confidence as TimeConfidence)).toBe(c.expected);
    });
  }
});

describe("formatEfficiency (V2 parity)", () => {
  for (const c of fixtures.formatEfficiency) {
    it(`${c.savingsBytes} B / ${c.timeSeconds}s → ${c.expected}`, () => {
      expect(formatEfficiency(c.savingsBytes, c.timeSeconds)).toBe(c.expected);
    });
  }
});

describe("formatTime (V2 parity)", () => {
  for (const c of fixtures.formatTime) {
    it(`${c.seconds}s → ${c.expected}`, () => {
      expect(formatTime(c.seconds)).toBe(c.expected);
    });
  }
});

describe("formatFileSize (V2 parity)", () => {
  for (const c of fixtures.formatFileSize) {
    it(`${c.sizeBytes} B → ${c.expected}`, () => {
      expect(formatFileSize(c.sizeBytes)).toBe(c.expected);
    });
  }
});

describe("formatCrf (V2 parity)", () => {
  for (const c of fixtures.formatCrf) {
    it(`${c.crf} → ${c.expected}`, () => {
      expect(formatCrf(c.crf)).toBe(c.expected);
    });
  }
});

describe("formatStreamDisplay (V2 parity)", () => {
  for (const c of fixtures.formatStreamDisplay) {
    it(`${c.videoCodec} + ${c.audioCodecs.length} audio → ${c.expected}`, () => {
      expect(formatStreamDisplay(c.videoCodec, c.audioCodecs)).toBe(c.expected);
    });
  }
});

describe("formatResolution (V2 parity)", () => {
  for (const c of fixtures.formatResolution) {
    it(`${c.width}x${c.height} → ${c.expected}`, () => {
      expect(formatResolution(c.width, c.height)).toBe(c.expected);
    });
  }
});

describe("formatBitrate (V2 parity)", () => {
  for (const c of fixtures.formatBitrate) {
    it(`${c.kbps} kbps → ${c.expected}`, () => {
      expect(formatBitrate(c.kbps)).toBe(c.expected);
    });
  }
});

describe("formatAudioCodecs (V2 parity)", () => {
  for (const c of fixtures.formatAudioCodecs) {
    it(`${c.audioCodecs.length} streams → ${c.expected}`, () => {
      expect(formatAudioCodecs(c.audioCodecs)).toBe(c.expected);
    });
  }
});

describe("formatReductionPercent (V2 parity)", () => {
  for (const c of fixtures.formatReductionPercent) {
    it(`${c.percent} → ${c.expected}`, () => {
      expect(formatReductionPercent(c.percent)).toBe(c.expected);
    });
  }
});

describe("formatVmaf (V2 parity)", () => {
  for (const c of fixtures.formatVmaf) {
    it(`${c.score} → ${c.expected}`, () => {
      expect(formatVmaf(c.score)).toBe(c.expected);
    });
  }
});

describe("formatDate", () => {
  // Local-time construction keeps the expectations timezone-independent.
  it("formats epoch milliseconds as YYYY-MM-DD", () => {
    expect(formatDate(new Date(2026, 6, 20).getTime())).toBe("2026-07-20");
  });

  it("pads single-digit month and day", () => {
    expect(formatDate(new Date(2025, 0, 5, 23, 59).getTime())).toBe("2025-01-05");
  });

  it("renders an em dash for non-finite input", () => {
    expect(formatDate(Number.NaN)).toBe("—");
    expect(formatDate(Number.POSITIVE_INFINITY)).toBe("—");
  });
});

describe("documented divergences from V2", () => {
  it('negative size renders an em dash (V2 returned "-")', () => {
    expect(formatFileSize(-1)).toBe("—");
  });

  it('negative bitrate renders an em dash (V2 formatted "-5 kbps")', () => {
    expect(formatBitrate(-5)).toBe("—");
  });

  it("exact decimal ties round away from zero (V2 rounded half-to-even)", () => {
    // 0.5 GiB over 2h is exactly 0.25 GB/h: V2's ".1f" gave "0.2".
    expect(formatEfficiency(536870912, 7200)).toBe("0.3 GB/h");
  });

  it("non-finite input renders placeholders instead of throwing", () => {
    expect(formatCompactTime(Number.NaN)).toBe("—");
    expect(formatTime(Number.NaN)).toBe("--:--:--");
    expect(formatFileSize(Number.NaN)).toBe("—");
  });
});
