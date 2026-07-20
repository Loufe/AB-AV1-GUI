import { ChevronDown, FileVideo, GripVertical } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui";
import { formatCompactTime, formatFileSize } from "@/lib/format/format";
import { cn } from "@/lib/utils";

import { basename, outputTargetLabel } from "./queue-status";
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
  seconds,
  confidence,
}: {
  seconds: number | null;
  confidence: EstimateConfidence;
}) {
  const value = seconds !== null && seconds > 0 ? formatCompactTime(seconds) : EM_DASH;
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
  return (
    <div
      ref={ref}
      role="row"
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
      <button
        ref={handleRef}
        type="button"
        aria-label={`Reorder ${name}`}
        className={cn(
          "justify-self-center rounded p-0.5 text-muted-foreground/50",
          handleRef
            ? "cursor-grab hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            : "cursor-default",
        )}
      >
        <GripVertical className="size-3.5" aria-hidden="true" />
      </button>
      <span className="flex min-w-0 items-center gap-1.5">
        <FileVideo className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate">{name}</span>
      </span>
      <span className="truncate text-muted-foreground">{row.streams ?? EM_DASH}</span>
      <span className="text-right tabular-nums">
        {row.sizeBytes !== null ? formatFileSize(row.sizeBytes) : EM_DASH}
      </span>
      <TimeCell seconds={row.timeSec} confidence={row.timeConfidence} />
      <span className="flex items-center gap-1.5">
        {row.item.operation}
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
      <span className="text-muted-foreground">
        {outputTargetLabel(row.item.operation, row.item.output_target)}
      </span>
      <StatusCell status={row.status} />
    </div>
  );
}
