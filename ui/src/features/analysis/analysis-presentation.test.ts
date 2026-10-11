import { describe, expect, it } from "vitest";
import type { AnalysisRow } from "@/lib/bindings";
import { discoveredRow, predictedRow, scannedRow } from "@/test/fixtures/analysis";
import {
  exclusionText,
  metadataResolution,
  relativeFolder,
  rowPresentation,
  scanFailureText,
} from "./analysis-presentation";

describe("Analysis presentation", () => {
  it("keeps undiscovered metadata and pre-search predictions absent", () => {
    expect(rowPresentation(discoveredRow())).toMatchObject({
      label: "Discovered",
      metadata: null,
      prediction: null,
      measured: null,
    });
    expect(rowPresentation(scannedRow())).toMatchObject({
      label: "Scanned",
      prediction: null,
      measured: null,
    });
    expect(rowPresentation(predictedRow()).prediction?.predicted_size).toBe(8_000_000);
  });

  it("does not promote historical analyzed or converted standing into applicability", () => {
    const row = scannedRow(2, "old.mkv", {
      applicable: { Scanned: { reuse: "Inapplicable" } },
      historical: "Converted",
      analyze: "Eligible",
      convert: "Eligible",
    });
    expect(rowPresentation(row)).toMatchObject({
      label: "Scanned",
      prediction: null,
      measured: null,
    });
  });

  it("keeps stale refresh metadata explicit while withholding its old prediction", () => {
    const row = predictedRow();
    if (row.entry.File?.scan === "Discovered" || row.entry.File?.scan.Scanned === undefined)
      throw new Error("fixture");
    row.entry.File.scan.Scanned.refresh_failure = "ChangedDuringSampling";
    expect(rowPresentation(row)).toMatchObject({
      label: "Refresh failed",
      failed: true,
      prediction: null,
    });
    expect(rowPresentation(row).detail).toContain("previous observation");
  });

  it("preserves absent settled-output metadata and measured sizes", () => {
    const row: AnalysisRow = {
      ...discoveredRow(),
      entry: {
        File: {
          scan: {
            SettledOutput: {
              source_content_key: "source",
              output_content_key: "output",
              metadata: null,
              refresh_failure: null,
              status: {
                historical: "Converted",
                analyze: { Skip: { reason: "AlreadyAv1Matroska" } },
                convert: { Skip: { reason: "AlreadyAv1Matroska" } },
                applicable: {
                  Converted: {
                    summary: {
                      Remuxed: {
                        input_size: null,
                        output_size: null,
                        decided_at: 1,
                        source_run: 1,
                      },
                    },
                  },
                },
              },
            },
          },
        },
      },
    };
    expect(rowPresentation(row)).toMatchObject({
      label: "Remuxed",
      metadata: null,
      prediction: null,
    });
    expect(rowPresentation(row).measured?.Remuxed?.output_size).toBeNull();
  });

  it("formats Windows and POSIX folders with path boundaries and no operational path conversion", () => {
    expect(relativeFolder(discoveredRow(), [{ text: "/videos", lossy: false }])).toBe("series");
    expect(
      relativeFolder(
        {
          ...discoveredRow(),
          display_path: { text: "C:\\Videos\\series\\movie.mkv", lossy: true },
        },
        [{ text: "C:\\Videos", lossy: false }],
      ),
    ).toBe("series");
    expect(relativeFolder(discoveredRow(), [{ text: "/vid", lossy: false }])).toBe(
      "/videos/series",
    );
  });

  it("renders post-rotation dimensions and keeps unknown metadata absent", () => {
    const metadata = rowPresentation(scannedRow()).metadata;
    if (metadata === null) throw new Error("fixture");
    expect(metadataResolution(null)).toBe("—");
    for (const rotation of [90, -90, 270, -270]) {
      expect(metadataResolution({ ...metadata, rotation_degrees: rotation })).toBe("1080x1920");
    }
    for (const rotation of [0, 180, -180, 360]) {
      expect(metadataResolution({ ...metadata, rotation_degrees: rotation })).toBe("1920x1080");
    }
  });

  it("retains diagnostics and typed exclusion reasons", () => {
    expect(
      scanFailureText({ TimedOut: { diagnostic: { text: "fixture probe", truncated: true } } }),
    ).toContain("Diagnostic tail truncated");
    expect(exclusionText("ScanFailed")).toBe("Scan failures");
    expect(exclusionText({ Policy: { reason: "PathNotPersistable" } })).toBe("unsupported path");
    expect(
      exclusionText({ Policy: { reason: { LowResolution: { pixels: 100, minimum: 200 } } } }),
    ).toBe("below minimum resolution");
  });
});
