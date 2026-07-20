import {
  ChevronDown,
  ChevronRight,
  CircleCheck,
  CircleSlash,
  FileVideo,
  Folder,
  FolderOpen,
  ScanSearch,
} from "lucide-react";

import { Button, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui";
import { formatCompactTime, formatFileSize } from "@/lib/format/format";
import { cn } from "@/lib/utils";

import { Section, ThemePair } from "./theme-pair";

/**
 * Analysis view static design pass — hardcoded rows through the real
 * formatters, judged on localhost, verdict to be recorded on #36 like
 * D6/D11. Mockup literals only: the scan engine features don't exist yet,
 * so no domain types ahead of the bindings.
 *
 * Design decisions under review:
 * - The four-level model reads as a status column, not a tree of jargon:
 *   "Not scanned" (discovered) → "Scanned" → "Analyzed · CRF n" →
 *   "Converted · saved X" — same tone/icon language as the queue's D11 pass.
 * - Savings column carries the headline number; it steps down the D11
 *   confidence ramp (exact after analyze, estimate after scan, blank before).
 * - Folder rows aggregate size/savings/time and summarize child states.
 * - Toolbar: current folder + Change, then the ladder left-to-right —
 *   Basic Scan, Add All: Analyze, Add All: Convert. Per-row adds come via
 *   context menu later; no checkbox forest.
 * - Files the pipeline would skip (below minimum resolution, already AV1)
 *   show as muted rows with the reason inline, so the folder totals and the
 *   queue never disagree about what's convertible.
 */

const GIB = 1024 ** 3;

const COLS =
  "grid grid-cols-[minmax(0,1fr)_8.5rem_5.5rem_6.5rem_5rem_minmax(11rem,13rem)] items-center gap-x-2 px-2";

type Ramp = "exact" | "estimate" | "rough";

const RAMP_CLASS: Record<Ramp, string> = {
  exact: "text-foreground",
  estimate: "text-muted-foreground",
  rough: "text-muted-foreground/60",
};

function RampValue({ value, ramp, tooltip }: { value: string; ramp: Ramp; tooltip?: string }) {
  const className = cn("text-right tabular-nums", RAMP_CLASS[ramp]);
  if (!tooltip || ramp === "exact") return <span className={className}>{value}</span>;
  return (
    <Tooltip>
      <TooltipTrigger render={<span tabIndex={0} className={cn(className, "cursor-help")} />}>
        {value}
      </TooltipTrigger>
      <TooltipContent>{tooltip}</TooltipContent>
    </Tooltip>
  );
}

function StatusText({
  tone,
  icon: Icon,
  tooltip,
  children,
}: {
  tone: "success" | "warning" | "muted" | "primary";
  icon?: React.ComponentType<{ className?: string }>;
  tooltip?: React.ReactNode;
  children: React.ReactNode;
}) {
  const toneClass = {
    success: "text-success",
    warning: "text-warning",
    muted: "text-muted-foreground",
    primary: "text-foreground",
  }[tone];
  const body = (
    <>
      {Icon && <Icon className="size-3.5 shrink-0" aria-hidden="true" />}
      <span
        className={cn(
          "truncate",
          tooltip && "underline decoration-current/40 decoration-dotted underline-offset-2",
        )}
      >
        {children}
      </span>
    </>
  );
  if (!tooltip) {
    return <span className={cn("flex min-w-0 items-center gap-1.5", toneClass)}>{body}</span>;
  }
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            tabIndex={0}
            className={cn("flex min-w-0 cursor-help items-center gap-1.5", toneClass)}
          />
        }
      >
        {body}
      </TooltipTrigger>
      <TooltipContent>{tooltip}</TooltipContent>
    </Tooltip>
  );
}

interface MockAnalysisRow {
  name: string;
  depth: 0 | 1;
  streams: string | null;
  sizeBytes: number | null;
  savings: { value: string; ramp: Ramp; tooltip?: string } | null;
  timeSec: number | null;
  timeRamp: Ramp;
  status: React.ReactNode;
  dimmed?: boolean;
}

const FILES: MockAnalysisRow[] = [
  {
    name: "s02e01.mkv",
    depth: 1,
    streams: "H264 / AAC",
    sizeBytes: 3.4 * GIB,
    savings: { value: formatFileSize(1.9 * GIB), ramp: "exact" },
    timeSec: 4500,
    timeRamp: "estimate",
    status: (
      <StatusText tone="primary" icon={ScanSearch} tooltip="VMAF 95 reachable at CRF 24.25">
        Analyzed · CRF 24.25
      </StatusText>
    ),
  },
  {
    name: "s02e02.mkv",
    depth: 1,
    streams: "H264 / AAC",
    sizeBytes: 3.1 * GIB,
    savings: {
      value: formatFileSize(1.6 * GIB),
      ramp: "estimate",
      tooltip: "Estimated from similar files you've converted",
    },
    timeSec: 4100,
    timeRamp: "estimate",
    status: <StatusText tone="muted">Scanned</StatusText>,
  },
  {
    name: "s02e03.mkv",
    depth: 1,
    streams: null,
    sizeBytes: null,
    savings: null,
    timeSec: null,
    timeRamp: "rough",
    status: <StatusText tone="muted">Not scanned</StatusText>,
  },
  {
    name: "s02e04.mkv",
    depth: 1,
    streams: "AV1 / OPUS",
    sizeBytes: 1.2 * GIB,
    savings: null,
    timeSec: null,
    timeRamp: "exact",
    dimmed: true,
    status: (
      <StatusText tone="muted" icon={CircleSlash} tooltip="Already AV1 in an MKV container">
        Already AV1
      </StatusText>
    ),
  },
  {
    name: "extras-360p.mp4",
    depth: 1,
    streams: "H264 / AAC",
    sizeBytes: 0.4 * GIB,
    savings: null,
    timeSec: null,
    timeRamp: "exact",
    dimmed: true,
    status: (
      <StatusText tone="muted" icon={CircleSlash} tooltip="640×360 is under the 1280×720 minimum">
        Below minimum resolution
      </StatusText>
    ),
  },
  {
    name: "s02e05.mkv",
    depth: 1,
    streams: "H264 / AC3",
    sizeBytes: 3.6 * GIB,
    savings: { value: formatFileSize(2.1 * GIB), ramp: "exact" },
    timeSec: 4980,
    timeRamp: "exact",
    status: (
      <StatusText tone="success" icon={CircleCheck} tooltip="VMAF 95.1 · CRF 23.75 · 1h 23m">
        Converted · saved {formatFileSize(2.1 * GIB)}
      </StatusText>
    ),
  },
];

function AnalysisRow({ row }: { row: MockAnalysisRow }) {
  return (
    <div className={cn("border-b border-border/40 py-1 text-sm", COLS, row.dimmed && "opacity-60")}>
      <span className={cn("flex min-w-0 items-center gap-1.5", row.depth === 1 && "pl-6")}>
        <FileVideo className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate">{row.name}</span>
      </span>
      <span className="truncate text-muted-foreground">{row.streams ?? "—"}</span>
      <span className="text-right tabular-nums">
        {row.sizeBytes !== null ? formatFileSize(row.sizeBytes) : "—"}
      </span>
      {row.savings ? (
        <RampValue
          value={row.savings.value}
          ramp={row.savings.ramp}
          tooltip={row.savings.tooltip}
        />
      ) : (
        <span className="text-right text-muted-foreground/60 tabular-nums">—</span>
      )}
      <RampValue
        value={row.timeSec !== null ? formatCompactTime(row.timeSec) : "—"}
        ramp={row.timeRamp}
        tooltip={row.timeSec !== null ? "Based on similar files you've converted" : undefined}
      />
      {row.status}
    </div>
  );
}

function FolderHeaderRow({
  name,
  expanded,
  fileCount,
  sizeBytes,
  savings,
  timeSec,
  summary,
}: {
  name: string;
  expanded: boolean;
  fileCount: number;
  sizeBytes: number;
  savings: string;
  timeSec: number;
  summary: string;
}) {
  const Chevron = expanded ? ChevronDown : ChevronRight;
  return (
    <div className={cn("border-b border-border/40 bg-surface py-1 text-sm font-medium", COLS)}>
      <span className="flex min-w-0 items-center gap-1.5">
        <Chevron className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <Folder className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate">{name}</span>
        <span className="font-normal text-muted-foreground">{fileCount} files</span>
      </span>
      <span />
      <span className="text-right tabular-nums">{formatFileSize(sizeBytes)}</span>
      <span className="text-right text-muted-foreground tabular-nums">{savings}</span>
      <span className="text-right text-muted-foreground tabular-nums">
        {formatCompactTime(timeSec)}
      </span>
      <span className="truncate font-normal text-muted-foreground">{summary}</span>
    </div>
  );
}

function AnalysisToolbar() {
  return (
    <div className="flex items-center justify-between gap-2">
      <div className="flex min-w-0 items-center gap-1.5">
        <FolderOpen className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate text-sm text-muted-foreground">D:\Videos\Series</span>
        <Button size="sm" variant="ghost">
          Change…
        </Button>
      </div>
      <div className="flex items-center gap-1.5">
        <Button size="sm" variant="outline">
          <ScanSearch data-icon="inline-start" aria-hidden="true" />
          Basic Scan
        </Button>
        <Button size="sm" variant="outline">
          Add All: Analyze
        </Button>
        <Button size="sm">Add All: Convert</Button>
      </div>
    </div>
  );
}

function AnalysisPanel() {
  return (
    <div className="flex flex-col gap-3">
      <AnalysisToolbar />
      <div className="overflow-hidden rounded-md border border-border">
        <div className={cn("border-b border-border py-1 text-xs text-muted-foreground", COLS)}>
          <span>Name</span>
          <span>Input format</span>
          <span className="text-right">Size</span>
          <span className="text-right">Est. savings</span>
          <span className="text-right">Est. time</span>
          <span>Status</span>
        </div>
        <FolderHeaderRow
          name="Season 2"
          expanded
          fileCount={6}
          sizeBytes={FILES.reduce((a, r) => a + (r.sizeBytes ?? 0), 0)}
          savings={formatFileSize(5.6 * GIB)}
          timeSec={13580}
          summary="1 converted · 2 ready · 1 unscanned · 2 skipped"
        />
        {FILES.map((row) => (
          <AnalysisRow key={row.name} row={row} />
        ))}
        <FolderHeaderRow
          name="Season 3"
          expanded={false}
          fileCount={8}
          sizeBytes={24.4 * GIB}
          savings={formatFileSize(12.8 * GIB)}
          timeSec={36000}
          summary=""
        />
        <div className={cn("bg-surface py-1 text-sm font-medium", COLS)}>
          <span>Total · 14 files</span>
          <span />
          <span className="text-right tabular-nums">{formatFileSize(36.1 * GIB)}</span>
          <span className="text-right tabular-nums">{formatFileSize(18.4 * GIB)}</span>
          <span className="text-right text-muted-foreground tabular-nums">
            {formatCompactTime(49580)}
          </span>
          <span className="font-normal text-muted-foreground">
            est. 51% smaller after conversion
          </span>
        </div>
      </div>
    </div>
  );
}

export function AnalysisSection() {
  return (
    <Section title="Analysis view (static pass)">
      <p className="text-sm text-muted-foreground">
        Level ladder as plain status text: Not scanned → Scanned → Analyzed · CRF → Converted; skips
        (already AV1, below minimum resolution) dim the row with the reason inline. Savings is the
        headline column and steps down the D11 confidence ramp — exact after analyze, muted estimate
        after a basic scan, blank before. Folder rows aggregate and summarize child states; the
        footer carries the whole-library pitch.
      </p>
      <TooltipProvider>
        <ThemePair>
          <AnalysisPanel />
        </ThemePair>
      </TooltipProvider>
    </Section>
  );
}
