import type {
  AnalysisActivity,
  AnalysisDirectoryFailure,
  AnalysisDisplayText,
  AnalysisQueueExclusion,
  AnalysisRow,
  AnalysisScanFailure,
  ConversionSummary,
  RowEligibility,
  SearchPrediction,
  VideoMeta,
} from "@/lib/bindings";
import { formatResolution } from "@/lib/format/format";
import { skipReasonText } from "@/lib/format/skip-reason";

export function activityText(activity: AnalysisActivity): string {
  switch (activity) {
    case "Discovering":
      return "Discovering files…";
    case "Discovered":
      return "Discovery complete";
    case "BasicScanning":
      return "Basic Scan in progress…";
    case "Ready":
      return "Basic Scan complete";
    case "Cancelled":
      return "Scan cancelled. Choose a folder to start again.";
    default:
      return `Scan failed: ${activity.Failed.detail}`;
  }
}

export function scanFailureText(failure: AnalysisScanFailure): string {
  switch (failure) {
    case "Missing":
      return "The file is missing.";
    case "ChangedAfterProbe":
      return "The file changed after probing. Run Basic Scan again.";
    case "ChangedDuringSampling":
      return "The file changed during sampling. Run Basic Scan again.";
    default: {
      const info = failure.Unavailable ?? failure.InvalidOutput ?? failure.Supervision;
      const diagnostic =
        failure.TimedOut?.diagnostic ??
        failure.Rejected?.diagnostic ??
        failure.InvalidOutput?.diagnostic ??
        failure.Supervision?.diagnostic;
      const message =
        failure.TimedOut !== undefined
          ? "The media probe timed out."
          : failure.Rejected !== undefined
            ? "The media probe rejected this file."
            : (info?.detail ?? "Scan failed.");
      return [
        message,
        diagnostic?.text,
        diagnostic?.truncated ? "Diagnostic tail truncated." : null,
      ]
        .filter(Boolean)
        .join(" ");
    }
  }
}

function directoryFailureText(failure: AnalysisDirectoryFailure): string {
  switch (failure) {
    case "Missing":
      return "The folder is missing.";
    case "PermissionDenied":
      return "Permission to read the folder was denied.";
    case "NotDirectory":
      return "The selected path is not a folder.";
    case "TraversalRefused":
      return "Folder traversal was refused.";
    default:
      return failure.EntriesUnavailable !== undefined
        ? `${failure.EntriesUnavailable.count} folder entries unavailable: ${failure.EntriesUnavailable.detail}`
        : failure.Unavailable.detail;
  }
}

function eligibilityText(eligibility: RowEligibility): string {
  if (eligibility === "Eligible") return "Eligible";
  if (eligibility === "Remux") return "Remux";
  return skipReasonText(eligibility.Skip.reason).reason;
}

export function exclusionText(exclusion: AnalysisQueueExclusion): string {
  switch (exclusion) {
    case "Folder":
      return "Folders";
    case "DuplicateIdentity":
      return "Duplicate identities";
    case "ScanFailed":
      return "Scan failures";
    default:
      return skipReasonText(exclusion.Policy.reason).reason;
  }
}

/** These strings are presentation only; queue commands use generation/row IDs. */
export function relativeFolder(row: AnalysisRow, roots: readonly AnalysisDisplayText[]): string {
  const path = row.display_path.text.replaceAll("\\", "/");
  const root = roots
    .map(({ text }) => text.replaceAll("\\", "/").replace(/\/$/, ""))
    .sort((left, right) => right.length - left.length)
    .find((candidate) => path.startsWith(`${candidate}/`));
  const relative = root === undefined ? path : path.slice(root.length + 1);
  const folder = relative.slice(0, Math.max(0, relative.lastIndexOf("/"))) || ".";
  return roots.length > 1 && root !== undefined ? `${root} / ${folder}` : folder;
}

export function metadataResolution(metadata: VideoMeta | null): string {
  if (metadata === null) return formatResolution(null, null);
  const quarterTurn = ((metadata.rotation_degrees % 180) + 180) % 180 === 90;
  return quarterTurn
    ? formatResolution(metadata.height, metadata.width)
    : formatResolution(metadata.width, metadata.height);
}

export function rowPresentation(row: AnalysisRow): {
  label: string;
  detail: string | null;
  metadata: VideoMeta | null;
  prediction: SearchPrediction | null;
  measured: ConversionSummary | null;
  analyze: string | null;
  convert: string | null;
  failed: boolean;
} {
  const empty = {
    detail: null,
    metadata: null,
    prediction: null,
    measured: null,
    analyze: null,
    convert: null,
    failed: false,
  };
  if (row.entry.Folder !== undefined) {
    return {
      ...empty,
      label: "Folder unavailable",
      failed: true,
      detail:
        row.entry.Folder.failure === null ? null : directoryFailureText(row.entry.Folder.failure),
    };
  }
  const { scan } = row.entry.File;
  if (scan === "Discovered") return { ...empty, label: "Discovered" };
  if (scan.Failed !== undefined)
    return {
      ...empty,
      label: "Scan failed",
      failed: true,
      detail: scanFailureText(scan.Failed.failure),
    };
  const observed = scan.Scanned ?? scan.SettledOutput;
  if (observed.refresh_failure !== null) {
    return {
      ...empty,
      label: "Refresh failed",
      failed: true,
      metadata: observed.metadata,
      detail: `${scanFailureText(observed.refresh_failure)} Metadata is from the previous observation.`,
    };
  }
  const { applicable, analyze, convert } = observed.status;
  const common = {
    ...empty,
    metadata: observed.metadata,
    analyze: eligibilityText(analyze),
    convert: eligibilityText(convert),
  };
  if (applicable.Analyzed !== undefined)
    return { ...common, label: "Analyzed", prediction: applicable.Analyzed.prediction };
  if (applicable.Converted !== undefined)
    return {
      ...common,
      label: applicable.Converted.summary.Remuxed !== undefined ? "Remuxed" : "Converted",
      measured: applicable.Converted.summary,
    };
  const { reuse } = applicable.Scanned;
  return {
    ...common,
    label: "Scanned",
    detail:
      reuse === "Inapplicable"
        ? "Recorded searches do not match the current profile and targets."
        : typeof reuse === "object"
          ? "Media tools must be verified before a recorded search can be reused."
          : null,
  };
}
