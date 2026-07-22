import {
  Ellipsis,
  ExternalLink,
  FilePenLine,
  FileVideo,
  FolderSearch,
  GripVertical,
  RefreshCw,
  RotateCcw,
  Trash2,
} from "lucide-react";

import {
  Button,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui";
import { formatDurationMsCompact } from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";
import { useProgressStore } from "@/lib/store/progress-store";
import { cn } from "@/lib/utils";

import { basename, deriveRowStatus, outputTargetLabel } from "./queue-status";
import type { EstimateConfidence, QueueRowData } from "./queue-status";
import { StatusCell } from "./status-cell";

export type QueueRowAction =
  | "edit"
  | "open"
  | "reveal"
  | "retry"
  | "convert-anyway"
  | "reanalyze"
  | "remove";

export interface QueueRowActions {
  disabled: boolean;
  editable: boolean;
  retryable: boolean;
  recoverable: boolean;
  removable: boolean;
  onAction: (action: QueueRowAction) => void;
}

function RowActions({
  name,
  itemId,
  actions,
}: {
  name: string;
  itemId: number;
  actions: QueueRowActions;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={`Actions for ${name}`}
            data-queue-actions={itemId}
            onClick={(event) => event.stopPropagation()}
          />
        }
      >
        <Ellipsis aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem
          disabled={actions.disabled || !actions.editable}
          onClick={() => actions.onAction("edit")}
        >
          <FilePenLine aria-hidden="true" /> Edit details
        </DropdownMenuItem>
        <DropdownMenuItem disabled={actions.disabled} onClick={() => actions.onAction("open")}>
          <ExternalLink aria-hidden="true" /> Open file
        </DropdownMenuItem>
        <DropdownMenuItem disabled={actions.disabled} onClick={() => actions.onAction("reveal")}>
          <FolderSearch aria-hidden="true" /> Reveal in file manager
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          disabled={actions.disabled || !actions.retryable}
          onClick={() => actions.onAction("retry")}
        >
          <RefreshCw aria-hidden="true" /> Retry
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={actions.disabled || !actions.recoverable}
          onClick={() => actions.onAction("convert-anyway")}
        >
          Convert anyway
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={actions.disabled || !actions.recoverable}
          onClick={() => actions.onAction("reanalyze")}
        >
          Re-analyze
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          variant="destructive"
          disabled={actions.disabled || !actions.removable}
          onClick={() => actions.onAction("remove")}
        >
          <Trash2 aria-hidden="true" /> Remove
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

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
  inert = false,
}: {
  durationMs: number | null;
  confidence: EstimateConfidence;
  inert?: boolean;
}) {
  const value =
    durationMs !== null && durationMs > 0 ? formatDurationMsCompact(durationMs) : EM_DASH;
  const className = cn("text-right tabular-nums", CONFIDENCE_CLASS[confidence]);
  if (inert || value === EM_DASH || confidence === "exact") {
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
  actions,
  focusKey,
  style,
}: {
  row: QueueRowData;
  selected: boolean;
  onSelect: (event: React.MouseEvent | React.KeyboardEvent) => void;
  ref?: React.Ref<HTMLDivElement>;
  /** Drag handle target; the grip is inert when omitted. */
  handleRef?: React.Ref<HTMLButtonElement>;
  /** Source row still in the list while its copy rides the overlay. */
  isDragSource?: boolean;
  /** Rendered inside the drag overlay. */
  isOverlay?: boolean;
  actions?: QueueRowActions;
  focusKey?: string;
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
      aria-hidden={isOverlay ? "true" : undefined}
      aria-selected={selected}
      tabIndex={isOverlay ? -1 : 0}
      onClick={(event) => {
        if (
          event.target === event.currentTarget ||
          event.currentTarget.contains(event.target as Node)
        )
          onSelect(event);
      }}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect(event);
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
        isOverlay && "pointer-events-none",
      )}
    >
      <div role="cell" className="flex justify-center">
        {handleRef ? (
          <button
            ref={handleRef}
            type="button"
            aria-label={`Reorder ${name}`}
            data-queue-handle={row.item.id}
            data-queue-focus={focusKey}
            onClick={(event) => event.stopPropagation()}
            className="flex size-6 cursor-grab items-center justify-center rounded text-muted-foreground/50 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          >
            <GripVertical className="size-3.5" aria-hidden="true" />
          </button>
        ) : (
          <span aria-hidden="true" />
        )}
      </div>
      <div role="cell" className="flex min-w-0 items-center gap-1.5">
        <FileVideo className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="truncate">{name}</span>
        {actions !== undefined && (
          <span className="ml-auto" onClick={(event) => event.stopPropagation()}>
            <RowActions name={name} itemId={row.item.id} actions={actions} />
          </span>
        )}
      </div>
      <div role="cell" className="truncate text-muted-foreground">
        {row.streams ?? EM_DASH}
      </div>
      <div role="cell" className="text-right tabular-nums">
        {row.sizeBytes !== null ? formatFileSize(row.sizeBytes) : EM_DASH}
      </div>
      <div role="cell">
        <TimeCell durationMs={displayedTimeMs} confidence={displayedConfidence} inert={isOverlay} />
      </div>
      <div role="cell" className="flex items-center gap-1.5">
        {row.item.operation}
        {row.item.intent === "Refresh" && !isOverlay && (
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
      </div>
      <div role="cell" className="text-muted-foreground">
        {outputTargetLabel(row.item.operation, row.item.output_target)}
      </div>
      <div role="cell" className="min-w-0">
        <StatusCell status={status} />
      </div>
    </div>
  );
}
