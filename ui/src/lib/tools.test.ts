import { describe, expect, it } from "vitest";

import type { ToolAvailability, ToolVerification } from "@/lib/bindings";
import {
  describeLocationFailure,
  describeProbeFailure,
  installGuidance,
  isAbsolutePath,
  startBlockReason,
  toolsStatus,
  verificationWarning,
} from "@/lib/tools";

function located(verification: ToolVerification): ToolAvailability {
  return {
    Located: {
      tools: {
        ffmpeg: { source: "Settings", path: "/opt/ffmpeg/bin/ffmpeg" },
        ffprobe: { source: "SearchPath", path: "/usr/bin/ffprobe" },
      },
      verification,
    },
  };
}

describe("tool availability presentation", () => {
  it("describes location failures with their paths", () => {
    expect(
      describeLocationFailure({
        SettingsPathIsNotAFile: { tool: "Ffmpeg", path: "/nowhere/ffmpeg" },
      }),
    ).toBe("The configured ffmpeg path does not name a file: /nowhere/ffmpeg");
    expect(
      describeLocationFailure({
        EnvironmentPathIsNotAFile: { tool: "Ffprobe", path: "C:\\tools\\ffprobe.exe" },
      }),
    ).toBe("The ffprobe environment override does not name a file: C:\\tools\\ffprobe.exe");
    expect(describeLocationFailure({ NotOnSearchPath: { tool: "Ffprobe" } })).toBe(
      "ffprobe was not found on PATH.",
    );
  });

  it("describes every probe failure", () => {
    expect(describeProbeFailure({ TimedOut: { capability: "VmafFilter" } })).toBe(
      "The ffmpeg libvmaf filter check timed out.",
    );
    expect(
      describeProbeFailure({ Unsupported: { capability: "Svtav1Encoder", diagnostic: "x" } }),
    ).toBe("The located tools do not support the ffmpeg libsvtav1 encoder.");
    expect(
      describeProbeFailure({ CouldNotRun: { capability: "FfprobeVersion", detail: "spawn" } }),
    ).toBe("The ffprobe version report check could not run: spawn");
    expect(describeProbeFailure({ InvalidVersionDocument: { detail: "no version" } })).toBe(
      "ffprobe reported an unreadable version document: no version",
    );
  });

  it("blocks a start only while tools are unknown or missing", () => {
    expect(startBlockReason(null)).toBe("Checking media tools before the Queue can start.");
    expect(startBlockReason({ Missing: { failures: [] } })).toBe(
      "Media tool discovery has not reported yet.",
    );
    expect(
      startBlockReason({ Missing: { failures: [{ NotOnSearchPath: { tool: "Ffmpeg" } }] } }),
    ).toBe("ffmpeg was not found on PATH. Configure the media tools in Settings.");
    expect(startBlockReason(located("Pending"))).toBeNull();
    expect(
      startBlockReason(located({ Failed: { TimedOut: { capability: "VmafFilter" } } })),
    ).toBeNull();
  });

  it("warns about a failed verification without blocking", () => {
    expect(verificationWarning(null)).toBeNull();
    expect(verificationWarning(located("Pending"))).toBeNull();
    expect(
      verificationWarning(located({ Failed: { TimedOut: { capability: "VmafFilter" } } })),
    ).toBe("The ffmpeg libvmaf filter check timed out. Starting the Queue checks the tools again.");
  });

  it("summarizes each availability state with its paths", () => {
    expect(toolsStatus(null)).toMatchObject({ tone: "muted", needsInstall: false });
    expect(toolsStatus({ Missing: { failures: [] } })).toMatchObject({ tone: "warning" });
    expect(
      toolsStatus({ Missing: { failures: [{ NotOnSearchPath: { tool: "Ffmpeg" } }] } }),
    ).toEqual({
      tone: "destructive",
      headline: "Media tools are missing.",
      details: ["ffmpeg was not found on PATH."],
      needsInstall: true,
    });
    expect(toolsStatus(located("Pending"))).toEqual({
      tone: "muted",
      headline: "Media tools located. They are checked when the Queue starts.",
      details: [
        "ffmpeg (configured path): /opt/ffmpeg/bin/ffmpeg",
        "ffprobe (found on PATH): /usr/bin/ffprobe",
      ],
      needsInstall: false,
    });
    expect(
      toolsStatus(
        located({
          Verified: {
            revisions: { ab_av1: "a", ffmpeg: "8.1.2", encoder: "8.1.2" },
            hardware_decoders: [],
          },
        }),
      ),
    ).toMatchObject({ tone: "success", headline: "Media tools verified: FFmpeg 8.1.2." });
    expect(
      toolsStatus(
        located({ Failed: { Unsupported: { capability: "VmafFilter", diagnostic: "" } } }),
      ),
    ).toMatchObject({
      tone: "destructive",
      details: [
        "ffmpeg (configured path): /opt/ffmpeg/bin/ffmpeg",
        "ffprobe (found on PATH): /usr/bin/ffprobe",
        "The located tools do not support the ffmpeg libvmaf filter.",
      ],
      needsInstall: true,
    });
  });

  it("recognizes absolute paths in both platform spellings", () => {
    expect(isAbsolutePath("/usr/bin/ffmpeg")).toBe(true);
    expect(isAbsolutePath("C:\\ffmpeg\\bin\\ffmpeg.exe")).toBe(true);
    expect(isAbsolutePath("D:/ffmpeg/ffmpeg.exe")).toBe(true);
    expect(isAbsolutePath("\\\\server\\share\\ffmpeg.exe")).toBe(true);
    expect(isAbsolutePath("ffmpeg")).toBe(false);
    expect(isAbsolutePath("bin/ffmpeg")).toBe(false);
    expect(isAbsolutePath("")).toBe(false);
  });

  it("tailors install guidance to the shell platform", () => {
    expect(installGuidance("Windows").join(" ")).toContain("ffmpeg.exe");
    expect(installGuidance("Linux").join(" ")).toContain("apt install ffmpeg");
    expect(installGuidance("Other").join(" ")).toContain("brew install ffmpeg");
    expect(installGuidance(null)).toEqual(installGuidance("Other"));
  });
});
