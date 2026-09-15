// Pure presentation of the engine's media tool availability. The engine's
// own summaries stay path-free (they travel in logs and rejection reasons);
// this module is where paths are shown deliberately, because the settings
// panel is the one place the user needs to see exactly which file failed.

import type {
  HostPlatform,
  LocatedTool,
  MediaTool,
  ProbeFailure,
  ToolAvailability,
  ToolCapability,
  ToolLocationFailure,
  ToolSource,
} from "@/lib/bindings";

function toolName(tool: MediaTool): string {
  return tool === "Ffmpeg" ? "ffmpeg" : "ffprobe";
}

function capabilityLabel(capability: ToolCapability): string {
  switch (capability) {
    case "FfprobeVersion":
      return "ffprobe version report";
    case "Svtav1Encoder":
      return "ffmpeg libsvtav1 encoder";
    case "VmafFilter":
      return "ffmpeg libvmaf filter";
  }
}

function sourceLabel(source: ToolSource): string {
  switch (source) {
    case "Environment":
      return "environment override";
    case "Settings":
      return "configured path";
    case "SearchPath":
      return "found on PATH";
  }
}

export function describeLocationFailure(failure: ToolLocationFailure): string {
  if (failure.EnvironmentPathIsNotAFile !== undefined) {
    const { tool, path } = failure.EnvironmentPathIsNotAFile;
    return `The ${toolName(tool)} environment override does not name a file: ${path}`;
  }
  if (failure.SettingsPathIsNotAFile !== undefined) {
    const { tool, path } = failure.SettingsPathIsNotAFile;
    return `The configured ${toolName(tool)} path does not name a file: ${path}`;
  }
  return `${toolName(failure.NotOnSearchPath.tool)} was not found on PATH.`;
}

export function describeProbeFailure(failure: ProbeFailure): string {
  if (failure.CouldNotRun !== undefined) {
    const { capability, detail } = failure.CouldNotRun;
    return `The ${capabilityLabel(capability)} check could not run: ${detail}`;
  }
  if (failure.TimedOut !== undefined) {
    return `The ${capabilityLabel(failure.TimedOut.capability)} check timed out.`;
  }
  if (failure.Unsupported !== undefined) {
    return `The located tools do not support the ${capabilityLabel(failure.Unsupported.capability)}.`;
  }
  return `ffprobe reported an unreadable version document: ${failure.InvalidVersionDocument.detail}`;
}

function describeLocated(tool: MediaTool, located: LocatedTool): string {
  return `${toolName(tool)} (${sourceLabel(located.source)}): ${located.path}`;
}

export type ToolsTone = "muted" | "success" | "warning" | "destructive";

interface ToolsStatus {
  tone: ToolsTone;
  headline: string;
  details: string[];
  /** Install guidance belongs next to this status. */
  needsInstall: boolean;
}

export function toolsStatus(tools: ToolAvailability | null): ToolsStatus {
  if (tools === null) {
    return {
      tone: "muted",
      headline: "Waiting for the media tool report.",
      details: [],
      needsInstall: false,
    };
  }
  if (tools.Missing !== undefined) {
    const { failures } = tools.Missing;
    if (failures.length === 0) {
      return {
        tone: "warning",
        headline: "Media tool discovery has not reported yet.",
        details: [],
        needsInstall: false,
      };
    }
    return {
      tone: "destructive",
      headline: "Media tools are missing.",
      details: failures.map(describeLocationFailure),
      needsInstall: true,
    };
  }
  const { tools: located, verification } = tools.Located;
  const paths = [
    describeLocated("Ffmpeg", located.ffmpeg),
    describeLocated("Ffprobe", located.ffprobe),
  ];
  if (verification === "Pending") {
    return {
      tone: "muted",
      headline: "Media tools located. They are checked when the Queue starts.",
      details: paths,
      needsInstall: false,
    };
  }
  if (verification.Verified !== undefined) {
    return {
      tone: "success",
      headline: `Media tools verified: FFmpeg ${verification.Verified.revisions.ffmpeg}.`,
      details: paths,
      needsInstall: false,
    };
  }
  return {
    tone: "destructive",
    headline: "Media tools failed verification.",
    details: [...paths, describeProbeFailure(verification.Failed)],
    needsInstall: true,
  };
}

/**
 * Why the Queue cannot start yet, or null when the engine would accept a
 * start. Mirrors the reducer: only missing tools block; a failed
 * verification is re-probed by the next session start.
 */
export function startBlockReason(tools: ToolAvailability | null): string | null {
  if (tools === null) return "Checking media tools before the Queue can start.";
  if (tools.Missing === undefined) return null;
  const { failures } = tools.Missing;
  if (failures.length === 0) return "Media tool discovery has not reported yet.";
  return `${failures.map(describeLocationFailure).join(" ")} Configure the media tools in Settings.`;
}

/** A failed verification worth showing beside an enabled Start button. */
export function verificationWarning(tools: ToolAvailability | null): string | null {
  if (tools?.Located === undefined) return null;
  const { verification } = tools.Located;
  if (verification === "Pending" || verification.Failed === undefined) return null;
  return `${describeProbeFailure(verification.Failed)} Starting the Queue checks the tools again.`;
}

/**
 * Same rule as the engine's Settings validation: a configured tool path is
 * absolute on either platform's spelling (POSIX root, drive letter, or UNC).
 */
export function isAbsolutePath(value: string): boolean {
  return /^(\/|[A-Za-z]:[\\/]|\\\\)/.test(value);
}

export function installGuidance(platform: HostPlatform | null): string[] {
  const requirement =
    "CRFty needs an FFmpeg build with the libsvtav1 encoder and the libvmaf filter; ffmpeg and ffprobe come together in every build.";
  switch (platform) {
    case "Windows":
      return [
        requirement,
        "Download a full build (the gyan.dev or BtbN releases include both), unpack it, then either add its bin folder to PATH or set the paths above to ffmpeg.exe and ffprobe.exe.",
      ];
    case "Linux":
      return [
        requirement,
        "Install FFmpeg from your distribution (Debian and Ubuntu: apt install ffmpeg; Fedora: FFmpeg from RPM Fusion; Arch: pacman -S ffmpeg), or unpack a BtbN static build and set the paths above.",
      ];
    case "Other":
    case null:
      return [
        requirement,
        "Install FFmpeg with your package manager (Homebrew: brew install ffmpeg) or download an official build, then add it to PATH or set the paths above.",
      ];
  }
}
