import {
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleSlash,
  FileVideo,
  Folder,
  GripVertical,
  Play,
} from "lucide-react";

import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Input,
  Label,
  Progress,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui";
import {
  formatCompactTime,
  formatCrf,
  formatFileSize,
  formatStreamDisplay,
  formatTime,
} from "@/lib/format/format";
import { cn } from "@/lib/utils";

import { Section } from "./theme-pair";

/**
 * Queue view static design pass (#36 D11) — hardcoded rows through the real
 * formatters; drag/selects are dressed, not wired. Judged on localhost, then
 * recorded on #36 like the D6/D7 verdicts.
 *
 * Design decisions under review:
 * - Column set carries over from the Python tab (Name, Format, Size, Time,
 *   Operation, Output, Status) at medium density.
 * - Status column per D11: colored text + at most one small icon, no chips;
 *   skip/error reasons inline on the row; the converting row gets an in-cell
 *   mini progress track.
 * - Operation cell per D11: one selector (chevron affordance) plus a small
 *   primary dot when a precise CRF is cached — replacing "Analyze+Convert".
 * - Estimate confidence per D11: the ~/~~ tildes are gone; values step down
 *   a muted-color ramp instead (exact / estimate / rough).
 * - Totals as a sticky-style footer row; toolbar shows the running state
 *   (Start disabled, stops live); properties panel shows an ANALYZE
 *   selection with output settings disabled (parity rule).
 */

const GIB = 1024 ** 3;

const CONFIDENCE_CLASS = {
  exact: "text-foreground",
  estimate: "text-muted-foreground",
  rough: "text-muted-foreground/60",
} as const;

type Confidence = keyof typeof CONFIDENCE_CLASS;

// Deliberately NOT QueueItem-shaped — mockup literals, no domain types
// ahead of the generated bindings (#33).
interface MockFileRow {
  name: string;
  streams: string;
  sizeBytes: number;
  timeSec: number;
  timeConfidence: Confidence;
  operation: "Convert" | "Analyze";
  preciseCrf: boolean;
  output: string;
  status: React.ReactNode;
  active?: boolean;
  selected?: boolean;
}

const COLS =
  "grid grid-cols-[1.75rem_minmax(0,1fr)_8.5rem_5.5rem_5rem_7rem_5.5rem_minmax(10rem,12rem)] items-center gap-x-2 px-2";

function StatusText({
  tone,
  icon: Icon,
  tooltip,
  children,
}: {
  tone: "success" | "warning" | "destructive" | "muted";
  icon?: React.ComponentType<{ className?: string }>;
  /** Reason/remediation detail (D11: reasons ride the item, not the logs). */
  tooltip?: React.ReactNode;
  children: React.ReactNode;
}) {
  const toneClass = {
    success: "text-success",
    warning: "text-warning",
    destructive: "text-destructive",
    muted: "text-muted-foreground",
  }[tone];
  if (!tooltip) {
    return (
      <span className={cn("flex min-w-0 items-center gap-1.5", toneClass)}>
        {Icon && <Icon className="size-3.5 shrink-0" aria-hidden="true" />}
        <span className="truncate">{children}</span>
      </span>
    );
  }
  // Dotted underline = "more here"; the trigger is focusable so the detail
  // is reachable by keyboard, not hover-only (D8).
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
        {Icon && <Icon className="size-3.5 shrink-0" aria-hidden="true" />}
        <span className="truncate underline decoration-current/40 decoration-dotted underline-offset-2">
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent variant="rich">{tooltip}</TooltipContent>
    </Tooltip>
  );
}

/**
 * Rich-tooltip skeleton: toned icon + title header, structured body, and the
 * remediation as a separated footer row — data as label/value, not prose.
 */
function ReasonTooltip({
  icon: Icon,
  toneClass,
  title,
  action,
  children,
}: {
  icon: React.ComponentType<{ className?: string }>;
  toneClass: string;
  title: string;
  action?: string;
  children?: React.ReactNode;
}) {
  return (
    <>
      <p className={cn("flex items-center gap-1.5 font-medium", toneClass)}>
        <Icon className="size-3.5 shrink-0" aria-hidden="true" />
        {title}
      </p>
      {children}
      {action && <p className="mt-2 border-t border-border pt-2 text-muted-foreground">{action}</p>}
    </>
  );
}

/** Estimated times explain their basis on demand — no tilde jargon (D11). */
const CONFIDENCE_TOOLTIP: Record<Exclude<Confidence, "exact">, string> = {
  estimate: "Based on similar files you've converted",
  rough: "Rough guess — no history for this codec yet",
};

function TimeCell({ seconds, confidence }: { seconds: number; confidence: Confidence }) {
  const value = seconds > 0 ? formatCompactTime(seconds) : "—";
  const className = cn("text-right tabular-nums", CONFIDENCE_CLASS[confidence]);
  if (seconds <= 0 || confidence === "exact") {
    return <span className={className}>{value}</span>;
  }
  return (
    <Tooltip>
      <TooltipTrigger render={<span tabIndex={0} className={cn(className, "cursor-help")} />}>
        {value}
      </TooltipTrigger>
      <TooltipContent>{CONFIDENCE_TOOLTIP[confidence]}</TooltipContent>
    </Tooltip>
  );
}

/** Converting rows carry their progress in the status cell, not a dialog. */
function ConvertingStatus({ pct }: { pct: number }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 pr-3">
      <span className="text-foreground">Converting… {pct}%</span>
      <div className="h-0.5 w-full overflow-hidden rounded-full bg-muted">
        <div className="h-full rounded-full bg-primary" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

const SEASON_FILES: MockFileRow[] = [
  {
    name: "s01e01.mkv",
    streams: formatStreamDisplay("h264", ["aac"]),
    sizeBytes: 3.21 * GIB,
    timeSec: 4320,
    timeConfidence: "exact",
    operation: "Convert",
    preciseCrf: false,
    output: "Replace",
    status: (
      <StatusText
        tone="success"
        icon={CircleCheck}
        tooltip={
          <ReasonTooltip icon={CircleCheck} toneClass="text-success" title="Converted">
            <div className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
              <span className="text-muted-foreground">VMAF</span>
              <span className="tabular-nums">95.2</span>
              <span className="text-muted-foreground">CRF</span>
              <span className="tabular-nums">{formatCrf(24)}</span>
              <span className="text-muted-foreground">Time</span>
              <span className="tabular-nums">{formatCompactTime(4320)}</span>
              <span className="text-muted-foreground">Size</span>
              <span className="tabular-nums">
                {formatFileSize(3.21 * GIB)} → {formatFileSize(1.34 * GIB)} · −58%
              </span>
            </div>
          </ReasonTooltip>
        }
      >
        Done · saved {formatFileSize(1.87 * GIB)}
      </StatusText>
    ),
  },
  {
    name: "s01e02.mkv",
    streams: formatStreamDisplay("h264", ["aac", "ac3"]),
    sizeBytes: 4.11 * GIB,
    timeSec: 6780,
    timeConfidence: "exact",
    operation: "Convert",
    preciseCrf: true,
    output: "Replace",
    status: <ConvertingStatus pct={62} />,
    active: true,
  },
  {
    name: "s01e03.mkv",
    streams: formatStreamDisplay("h264", ["aac"]),
    sizeBytes: 2.87 * GIB,
    timeSec: 3480,
    timeConfidence: "estimate",
    operation: "Convert",
    preciseCrf: true,
    output: "Replace",
    status: <StatusText tone="muted">Queued</StatusText>,
  },
  {
    name: "s01e04.mkv",
    streams: formatStreamDisplay("hevc", ["ac3"]),
    sizeBytes: 5.63 * GIB,
    timeSec: 5040,
    timeConfidence: "rough",
    operation: "Analyze",
    preciseCrf: false,
    output: "—",
    status: <StatusText tone="muted">Queued</StatusText>,
    selected: true,
  },
  {
    name: "s01e05.avi",
    streams: formatStreamDisplay("mpeg4", ["mp3"]),
    sizeBytes: 0.72 * GIB,
    timeSec: 0,
    timeConfidence: "exact",
    operation: "Convert",
    preciseCrf: false,
    output: "Replace",
    status: (
      <StatusText
        tone="warning"
        icon={CircleSlash}
        tooltip={
          <ReasonTooltip
            icon={CircleSlash}
            toneClass="text-warning"
            title="Not worthwhile"
            action="Lower the VMAF floor in Settings to convert anyway."
          >
            <p className="mt-1 text-muted-foreground">
              No quality level down to the VMAF 90 floor saved meaningful space.
            </p>
          </ReasonTooltip>
        }
      >
        Skipped · not worthwhile
      </StatusText>
    ),
  },
  {
    name: "s01e06.wmv",
    streams: formatStreamDisplay("vc1", ["wmav2"]),
    sizeBytes: 1.38 * GIB,
    timeSec: 0,
    timeConfidence: "exact",
    operation: "Convert",
    preciseCrf: false,
    output: "Replace",
    status: (
      <StatusText
        tone="destructive"
        icon={CircleAlert}
        tooltip={
          <ReasonTooltip
            icon={CircleAlert}
            toneClass="text-destructive"
            title="Input unreadable"
            action="Remux to MKV and re-add. Full details in the log."
          >
            <p className="mt-1 text-muted-foreground">
              ffprobe could not parse this file — likely corrupt or truncated.
            </p>
          </ReasonTooltip>
        }
      >
        Error · input unreadable
      </StatusText>
    ),
  },
];

function FileRow({ row }: { row: MockFileRow }) {
  return (
    <div
      className={cn(
        "border-b border-border/40 py-1 text-sm",
        COLS,
        row.active && "bg-primary/5",
        row.selected && "bg-accent",
      )}
    >
      <GripVertical className="size-3.5 justify-self-center text-muted-foreground/50" />
      <span className="flex min-w-0 items-center gap-1.5 pl-5">
        <FileVideo className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate">{row.name}</span>
      </span>
      <span className="truncate text-muted-foreground">{row.streams}</span>
      <span className="text-right tabular-nums">{formatFileSize(row.sizeBytes)}</span>
      <TimeCell seconds={row.timeSec} confidence={row.timeConfidence} />
      <span className="flex items-center gap-1.5">
        {row.operation}
        {row.preciseCrf && (
          <Tooltip>
            <TooltipTrigger
              render={
                <span tabIndex={0} className="-m-1 flex size-4 items-center justify-center" />
              }
            >
              <span className="size-1.5 rounded-full bg-primary" />
            </TooltipTrigger>
            <TooltipContent>Precise CRF cached — skips the quality search</TooltipContent>
          </Tooltip>
        )}
        <ChevronDown className="size-3 text-muted-foreground" aria-hidden="true" />
      </span>
      <span className="text-muted-foreground">{row.output}</span>
      {row.status}
    </div>
  );
}

function FolderRow({
  name,
  expanded,
  fileCount,
  sizeBytes,
  timeSec,
  summary,
}: {
  name: string;
  expanded: boolean;
  fileCount: number;
  sizeBytes: number;
  timeSec: number;
  summary: string;
}) {
  const Chevron = expanded ? ChevronDown : ChevronRight;
  return (
    <div className={cn("border-b border-border/40 bg-surface py-1 text-sm font-medium", COLS)}>
      <GripVertical className="size-3.5 justify-self-center text-muted-foreground/50" />
      <span className="flex min-w-0 items-center gap-1.5">
        <Chevron className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <Folder className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate">{name}</span>
        <span className="font-normal text-muted-foreground">{fileCount} files</span>
      </span>
      <span />
      <span className="text-right tabular-nums">{formatFileSize(sizeBytes)}</span>
      <span className="text-right text-muted-foreground tabular-nums">
        {formatCompactTime(timeSec)}
      </span>
      <span />
      <span />
      <span className="truncate font-normal text-muted-foreground">{summary}</span>
    </div>
  );
}

function QueueToolbar() {
  return (
    <div className="flex items-center justify-between gap-2">
      <div className="flex items-center gap-1.5">
        <Button size="sm" variant="outline">
          + Add Folder
        </Button>
        <Button size="sm" variant="outline">
          + Add Files
        </Button>
        <Button size="sm" variant="ghost">
          Remove
        </Button>
        <Button size="sm" variant="ghost">
          Clear
        </Button>
        <Button size="sm" variant="ghost">
          Clear Completed
        </Button>
      </div>
      <div className="flex items-center gap-1.5">
        <Button size="sm" disabled>
          <Play data-icon="inline-start" aria-hidden="true" />
          Start Queue
        </Button>
        <Button size="sm" variant="outline">
          Stop After File
        </Button>
        <Button size="sm" variant="destructive">
          Force Stop
        </Button>
      </div>
    </div>
  );
}

function QueueTable() {
  return (
    <div className="overflow-hidden rounded-md border border-border">
      <div className={cn("border-b border-border py-1 text-xs text-muted-foreground", COLS)}>
        <span />
        <span>Name</span>
        <span>Format</span>
        <span className="text-right">Size</span>
        <span className="text-right">Time</span>
        <span>Operation</span>
        <span>Output</span>
        <span>Status</span>
      </div>
      <FolderRow
        name="Season 1"
        expanded
        fileCount={6}
        sizeBytes={SEASON_FILES.reduce((a, r) => a + r.sizeBytes, 0)}
        timeSec={SEASON_FILES.reduce((a, r) => a + r.timeSec, 0)}
        summary="1 done · 1 skipped · 1 failed"
      />
      {SEASON_FILES.map((row) => (
        <FileRow key={row.name} row={row} />
      ))}
      <FolderRow
        name="Movies"
        expanded={false}
        fileCount={2}
        sizeBytes={11.2 * GIB}
        timeSec={9060}
        summary=""
      />
      {/* Totals as a sticky-style footer (D11: the twin-Treeview hack dies). */}
      <div className={cn("bg-surface py-1 text-sm font-medium", COLS)}>
        <span />
        <span>Total · 8 items</span>
        <span />
        <span className="text-right tabular-nums">{formatFileSize(29.12 * GIB)}</span>
        <span className="text-right text-muted-foreground tabular-nums">
          {formatCompactTime(28860)}
        </span>
        <span />
        <span />
        <span className="font-normal text-muted-foreground">1 done · 1 skipped · 1 failed</span>
      </div>
    </div>
  );
}

function LabeledBar({ label, pct }: { label: string; pct: number }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-14 shrink-0 text-xs text-muted-foreground">{label}</span>
      <Progress value={pct} className="flex-1" />
      <span className="w-9 shrink-0 text-right text-xs tabular-nums">{pct}%</span>
    </div>
  );
}

function NowProcessingCard() {
  return (
    <Card size="sm" className="gap-2">
      <CardHeader className="gap-0.5">
        <CardTitle className="flex items-center gap-1.5 text-sm">
          <FileVideo className="size-4 text-muted-foreground" aria-hidden="true" />
          s01e02.mkv
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-1.5">
        <LabeledBar label="Analyze" pct={100} />
        <LabeledBar label="Encode" pct={62} />
        <p className="pt-1 text-xs text-muted-foreground">
          VMAF 95.3 · CRF 24.25 · preset 6 · output {formatFileSize(1.21 * GIB)} so far · elapsed{" "}
          {formatTime(2533)} · ETA {formatCompactTime(2280)}
        </p>
      </CardContent>
    </Card>
  );
}

function SelectionCard() {
  return (
    <Card size="sm" className="gap-2">
      <CardHeader className="gap-0.5">
        <CardTitle className="text-sm">Selection · s01e04.mkv</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <div className="flex items-center gap-2">
          <Label className="w-20 shrink-0 text-xs text-muted-foreground">Operation</Label>
          <Select defaultValue="analyze">
            <SelectTrigger size="sm" className="flex-1">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="analyze">Analyze</SelectItem>
              <SelectItem value="convert">Convert</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-2">
          <Label className="w-20 shrink-0 text-xs text-muted-foreground">Output</Label>
          <Select defaultValue="replace" disabled>
            <SelectTrigger size="sm" className="flex-1">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="replace">Replace original</SelectItem>
              <SelectItem value="suffix">Save with suffix</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-2">
          <Label className="w-20 shrink-0 text-xs text-muted-foreground">Suffix</Label>
          <Input defaultValue="_av1" disabled className="h-7 flex-1" />
        </div>
        <p className="text-xs text-muted-foreground">
          Analyze produces no output file — output settings are disabled.
        </p>
      </CardContent>
    </Card>
  );
}

function QueuePanel() {
  return (
    <div className="flex flex-col gap-3">
      <QueueToolbar />
      <QueueTable />
      <div className="grid grid-cols-[2fr_1fr] gap-3">
        <NowProcessingCard />
        <SelectionCard />
      </div>
    </div>
  );
}

export function QueueSection() {
  return (
    <Section title="Queue view (D11 static pass)">
      <p className="text-sm text-muted-foreground">
        Running state: s01e02 converting (highlighted row, in-cell progress), s01e04 selected
        (drives the properties card). Operation cells: chevron = in-cell selector, orange dot =
        precise CRF cached (replaces &quot;Analyze+Convert&quot;). Time column steps down the
        confidence ramp instead of ~/~~ tildes. Reasons ride the status cell; totals are the footer
        row. Dotted underline = tooltip with the full story — hover it or Tab to it (Done, Skipped,
        Error, estimated times, and the CRF dot all carry one).
      </p>
      <TooltipProvider>
        <div className="rounded-lg border border-border bg-background p-4">
          <QueuePanel />
        </div>
        <div className="dark rounded-lg border border-border bg-background p-4 text-foreground">
          <QueuePanel />
        </div>
      </TooltipProvider>
    </Section>
  );
}
