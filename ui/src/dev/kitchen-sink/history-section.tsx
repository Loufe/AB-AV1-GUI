import { ArrowDown, CircleAlert, CircleCheck, CircleSlash, ScanSearch, Search } from "lucide-react";

import {
  Button,
  Input,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui";
import { formatCrf, formatFileSize, formatTime } from "@/lib/format/format";
import { cn } from "@/lib/utils";

import { Section, ThemePair } from "./theme-pair";

/**
 * History view static design pass — hardcoded rows through the real
 * formatters, judged on localhost, verdict to be recorded on #36.
 *
 * Design decisions under review:
 * - One flat, sortable list; the date column carries the default sort
 *   (newest first, arrow affordance in the header).
 * - Filter bar: search plus outcome pills — All / Converted / Analyzed /
 *   Skipped / Failed — no dropdown; counts ride the pills.
 * - Size story reads left to right as before → after → saved%, with the
 *   saved percentage as the strongest number on the row.
 * - Quality cell compresses the analysis facts to "VMAF · CRF"; the full
 *   provenance (preset, fallback path) lives in the tooltip.
 * - Non-file outcomes reuse the queue's tone/icon language exactly, so the
 *   two views never invent different vocabularies for the same result.
 */

const GIB = 1024 ** 3;

const COLS =
  "grid grid-cols-[minmax(0,1fr)_6.5rem_5.5rem_5.5rem_4.5rem_7rem_5.5rem_minmax(8rem,10rem)] items-center gap-x-2 px-2";

interface MockHistoryRow {
  name: string;
  date: string;
  beforeBytes: number | null;
  afterBytes: number | null;
  savedPercent: number | null;
  quality: { vmaf: number; crf: number } | null;
  elapsedSec: number | null;
  outcome: React.ReactNode;
}

function OutcomeText({
  tone,
  icon: Icon,
  tooltip,
  children,
}: {
  tone: "success" | "warning" | "destructive" | "muted" | "primary";
  icon?: React.ComponentType<{ className?: string }>;
  tooltip?: React.ReactNode;
  children: React.ReactNode;
}) {
  const toneClass = {
    success: "text-success",
    warning: "text-warning",
    destructive: "text-destructive",
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

const ROWS: MockHistoryRow[] = [
  {
    name: "s01e02.mkv",
    date: "Today 14:32",
    beforeBytes: 4.11 * GIB,
    afterBytes: 2.02 * GIB,
    savedPercent: 51,
    quality: { vmaf: 95.3, crf: 24.25 },
    elapsedSec: 4813,
    outcome: (
      <OutcomeText tone="success" icon={CircleCheck}>
        Converted
      </OutcomeText>
    ),
  },
  {
    name: "s01e01.mkv",
    date: "Today 13:05",
    beforeBytes: 3.21 * GIB,
    afterBytes: 1.34 * GIB,
    savedPercent: 58,
    quality: { vmaf: 95.2, crf: 24 },
    elapsedSec: 4320,
    outcome: (
      <OutcomeText tone="success" icon={CircleCheck}>
        Converted
      </OutcomeText>
    ),
  },
  {
    name: "movie-night.mkv",
    date: "Yesterday 22:41",
    beforeBytes: 8.6 * GIB,
    afterBytes: null,
    savedPercent: null,
    quality: { vmaf: 95, crf: 22.75 },
    elapsedSec: 118,
    outcome: (
      <OutcomeText
        tone="primary"
        icon={ScanSearch}
        tooltip="CRF cached — conversion will skip the search"
      >
        Analyzed
      </OutcomeText>
    ),
  },
  {
    name: "s01e05.avi",
    date: "Yesterday 21:14",
    beforeBytes: 0.72 * GIB,
    afterBytes: null,
    savedPercent: null,
    quality: null,
    elapsedSec: 640,
    outcome: (
      <OutcomeText
        tone="warning"
        icon={CircleSlash}
        tooltip="Best attempt saved 3% at the VMAF 90 floor"
      >
        Not worthwhile
      </OutcomeText>
    ),
  },
  {
    name: "s01e06.wmv",
    date: "Yesterday 20:58",
    beforeBytes: 1.38 * GIB,
    afterBytes: null,
    savedPercent: null,
    quality: null,
    elapsedSec: 4,
    outcome: (
      <OutcomeText
        tone="destructive"
        icon={CircleAlert}
        tooltip={<span className="font-mono">ffprobe: moov atom not found</span>}
      >
        Failed
      </OutcomeText>
    ),
  },
];

function HistoryRow({ row }: { row: MockHistoryRow }) {
  return (
    <div className={cn("border-b border-border/40 py-1 text-sm", COLS)}>
      <span className="truncate">{row.name}</span>
      <span className="text-muted-foreground">{row.date}</span>
      <span className="text-right text-muted-foreground tabular-nums">
        {row.beforeBytes !== null ? formatFileSize(row.beforeBytes) : "—"}
      </span>
      <span className="text-right tabular-nums">
        {row.afterBytes !== null ? formatFileSize(row.afterBytes) : "—"}
      </span>
      <span
        className={cn(
          "text-right font-medium tabular-nums",
          row.savedPercent !== null ? "text-success" : "text-muted-foreground/60",
        )}
      >
        {row.savedPercent !== null ? `−${row.savedPercent}%` : "—"}
      </span>
      {row.quality ? (
        <Tooltip>
          <TooltipTrigger
            render={
              <span
                tabIndex={0}
                className="cursor-help text-right text-muted-foreground tabular-nums"
              />
            }
          >
            {row.quality.vmaf} · CRF {formatCrf(row.quality.crf)}
          </TooltipTrigger>
          <TooltipContent>SVT-AV1 preset 6 · target VMAF 95 · no fallback</TooltipContent>
        </Tooltip>
      ) : (
        <span className="text-right text-muted-foreground/60 tabular-nums">—</span>
      )}
      <span className="text-right text-muted-foreground tabular-nums">
        {row.elapsedSec !== null ? formatTime(row.elapsedSec) : "—"}
      </span>
      {row.outcome}
    </div>
  );
}

function FilterPill({ label, active }: { label: string; active?: boolean }) {
  return (
    <Button size="sm" variant={active ? "secondary" : "ghost"} className="h-7">
      {label}
    </Button>
  );
}

function HistoryPanel() {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-2">
        <div className="relative w-64">
          <Search
            className="absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <Input placeholder="Search files…" className="h-7 pl-7" />
        </div>
        <div className="flex items-center gap-1">
          <FilterPill label="All · 128" active />
          <FilterPill label="Converted · 97" />
          <FilterPill label="Analyzed · 12" />
          <FilterPill label="Skipped · 14" />
          <FilterPill label="Failed · 5" />
        </div>
      </div>
      <div className="overflow-hidden rounded-md border border-border">
        <div className={cn("border-b border-border py-1 text-xs text-muted-foreground", COLS)}>
          <span>Name</span>
          <span className="flex items-center gap-0.5">
            Date <ArrowDown className="size-3" aria-hidden="true" />
          </span>
          <span className="text-right">Before</span>
          <span className="text-right">After</span>
          <span className="text-right">Saved</span>
          <span className="text-right">Quality</span>
          <span className="text-right">Took</span>
          <span>Outcome</span>
        </div>
        {ROWS.map((row) => (
          <HistoryRow key={row.name} row={row} />
        ))}
        <div className={cn("bg-surface py-1 text-sm font-medium", COLS)}>
          <span>128 files · lifetime</span>
          <span />
          <span className="text-right text-muted-foreground tabular-nums">
            {formatFileSize(412 * GIB)}
          </span>
          <span className="text-right tabular-nums">{formatFileSize(198 * GIB)}</span>
          <span className="text-right text-success tabular-nums">−52%</span>
          <span />
          <span />
          <span className="font-normal text-muted-foreground">
            saved {formatFileSize(214 * GIB)}
          </span>
        </div>
      </div>
    </div>
  );
}

export function HistorySection() {
  return (
    <Section title="History view (static pass)">
      <p className="text-sm text-muted-foreground">
        Flat sortable list, newest first (arrow on the sorted column). Search plus outcome pills
        with counts replace dropdown filters. The size story reads before → after → saved%, with
        saved% as the strongest number; quality compresses to VMAF · CRF with provenance in the
        tooltip. Outcome vocabulary matches the queue exactly.
      </p>
      <TooltipProvider>
        <ThemePair>
          <HistoryPanel />
        </ThemePair>
      </TooltipProvider>
    </Section>
  );
}
