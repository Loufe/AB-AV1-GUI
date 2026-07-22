import { FileVideo, GripVertical, RotateCcw } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui";
import { formatDurationMsCompact } from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";
import { useProgressStore } from "@/lib/store/progress-store";
import { cn } from "@/lib/utils";

import { basename, deriveRowStatus, outputTargetLabel } from "./queue-status";
import type { EstimateConfidence, QueueRowData } from "./queue-status";
import { StatusCell } from "./status-cell";

/** Shared grid template so header, rows, and footer stay aligned (D11). */
export const QUEUE_COLS =
  "grid grid-cols-[1.75rem_minmax(0,1fr)_8.5rem_5.5rem_5rem_7rem_5.5rem_minmax(10rem,12rem)] items-center gap-x-2 px-2";

const EM_DASH = "—";

const CONFIDENCE_CLASS: Record<EstimateConfidence, string> = {
  exact: "text-foreground",
  estimate: "text-muted-foreground",
  rough: "text-muted-foreground/60",
};

/** Estimated times explain their basis on demand — no tilde jargon (D11). */
const CONFIDENCE_TOOLTIP: Record<Exclude<EstimateConfidence, "exact">, string> = {
  estimate: "Based on similar files you've converted",
  rough: "Rough guess — no history for this codec yet",
};

function TimeCell({
  durationMs,
  confidence,
}: {
  durationMs: number | null;
  confidence: EstimateConfidence;
}) {
  const value =
    durationMs !== null && durationMs > 0 ? formatDurationMsCompact(durationMs) : EM_DASH;
  const className = cn("text-right tabular-nums", CONFIDENCE_CLASS[confidence]);
  if (value === EM_DASH || confidence === "exact") {
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

export function QueueRow({
  row,
  selected,
  onSelect,
  ref,
  handleRef,
  isDragSource = false,
  isOverlay = false,
  style,
}: {
  row: QueueRowData;
  selected: boolean;
  onSelect: () => void;
  ref?: React.Ref<HTMLDivElement>;
  /** Drag handle target; the grip is inert when omitted. */
  handleRef?: React.Ref<HTMLButtonElement>;
  /** Source row still in the list while its copy rides the overlay. */
  isDragSource?: boolean;
  /** Rendered inside the drag overlay. */
  isOverlay?: boolean;
  style?: React.CSSProperties;
}) {
  const active = row.status.kind === "working";
  const name = basename(row.item.input);
  const telemetry = useProgressStore((state) =>
    row.runId === null ? null : (state.telemetry[row.runId] ?? null),
  );
  const status =
    row.item.state !== "Queued" && "Running" in row.item.state
      ? deriveRowStatus(row.item.state, telemetry, row.mediaDurationMs, null)
      : row.status;
  const displayedTimeMs = status.kind === "working" ? (telemetry?.eta_ms ?? null) : row.timeMs;
  const displayedConfidence = status.kind === "working" ? "estimate" : row.timeConfidence;
  return (
    <div
      ref={ref}
      role="row"
      aria-selected={selected}
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect();
        }
      }}
      style={style}
      className={cn(
        "cursor-default border-b border-border/40 bg-background py-1 text-sm",
        QUEUE_COLS,
        active && "bg-primary/5",
        selected && "bg-accent",
        isDragSource && "opacity-40",
        isOverlay && "rounded-md border border-border bg-elevated shadow-lg",
      )}
    >
      {handleRef ? (
        <button
          ref={handleRef}
          type="button"
          aria-label={`Reorder ${name}`}
          className="cursor-grab justify-self-center rounded p-0.5 text-muted-foreground/50 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        >
          <GripVertical className="size-3.5" aria-hidden="true" />
        </button>
      ) : (
        <span aria-hidden="true" />
      )}
      <span className="flex min-w-0 items-center gap-1.5">
        <FileVideo className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate">{name}</span>
      </span>
      <span className="truncate text-muted-foreground">{row.streams ?? EM_DASH}</span>
      <span className="text-right tabular-nums">
        {row.sizeBytes !== null ? formatFileSize(row.sizeBytes) : EM_DASH}
      </span>
      <TimeCell durationMs={displayedTimeMs} confidence={displayedConfidence} />
      <span className="flex items-center gap-1.5">
        {row.item.operation}
        {row.item.intent === "Refresh" && (
          <Tooltip>
            <TooltipTrigger
              render={
                <span tabIndex={0} className="-m-1 flex size-4 items-center justify-center" />
              }
            >
              <RotateCcw className="size-3 text-primary" aria-hidden="true" />
            </TooltipTrigger>
            <TooltipContent>Fresh analysis forced for this attempt</TooltipContent>
          </Tooltip>
        )}
      </span>
      <span className="text-muted-foreground">
        {outputTargetLabel(row.item.operation, row.item.output_target)}
      </span>
      <StatusCell status={status} />
    </div>
  );
}
