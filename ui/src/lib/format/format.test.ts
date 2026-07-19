import { describe, expect, it } from "vitest";

import {
  formatCompactTime,
  formatCrf,
  formatEfficiency,
  formatFileSize,
  formatStreamDisplay,
  formatTime,
  type TimeConfidence,
} from "./format";
import fixtures from "./parity-fixtures.json";

describe("formatCompactTime (Python parity)", () => {
  for (const c of fixtures.formatCompactTime) {
    it(`${c.seconds}s, ${c.confidence} → ${c.expected}`, () => {
      expect(formatCompactTime(c.seconds, c.confidence as TimeConfidence)).toBe(c.expected);
    });
  }
});

describe("formatEfficiency (Python parity)", () => {
  for (const c of fixtures.formatEfficiency) {
    it(`${c.savingsBytes} B / ${c.timeSeconds}s → ${c.expected}`, () => {
      expect(formatEfficiency(c.savingsBytes, c.timeSeconds)).toBe(c.expected);
    });
  }
});

describe("formatTime (Python parity)", () => {
  for (const c of fixtures.formatTime) {
    it(`${c.seconds}s → ${c.expected}`, () => {
      expect(formatTime(c.seconds)).toBe(c.expected);
    });
  }
});

describe("formatFileSize (Python parity)", () => {
  for (const c of fixtures.formatFileSize) {
    it(`${c.sizeBytes} B → ${c.expected}`, () => {
      expect(formatFileSize(c.sizeBytes)).toBe(c.expected);
    });
  }
});

describe("formatCrf (Python parity)", () => {
  for (const c of fixtures.formatCrf) {
    it(`${c.crf} → ${c.expected}`, () => {
      expect(formatCrf(c.crf)).toBe(c.expected);
    });
  }
});

describe("formatStreamDisplay (Python parity)", () => {
  for (const c of fixtures.formatStreamDisplay) {
    it(`${c.videoCodec} + ${c.audioCodecs.length} audio → ${c.expected}`, () => {
      expect(formatStreamDisplay(c.videoCodec, c.audioCodecs)).toBe(c.expected);
    });
  }
});

describe("documented divergences from Python", () => {
  it('negative size renders an em dash (Python returned "-")', () => {
    expect(formatFileSize(-1)).toBe("—");
  });

  it("exact decimal ties round away from zero (Python rounds half-to-even)", () => {
    // 0.5 GiB over 2h is exactly 0.25 GB/h: Python's ".1f" gives "0.2".
    expect(formatEfficiency(536870912, 7200)).toBe("0.3 GB/h");
  });

  it("non-finite input renders placeholders instead of throwing", () => {
    expect(formatCompactTime(Number.NaN)).toBe("—");
    expect(formatTime(Number.NaN)).toBe("--:--:--");
    expect(formatFileSize(Number.NaN)).toBe("—");
  });
});
