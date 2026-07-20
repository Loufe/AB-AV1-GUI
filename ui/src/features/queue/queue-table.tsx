import { DragDropProvider, DragOverlay } from "@dnd-kit/react";
import { useSortable } from "@dnd-kit/react/sortable";

import { TooltipProvider } from "@/components/ui";
import { formatCompactTime, formatFileSize } from "@/lib/format/format";
import { cn } from "@/lib/utils";

import type { QueueItemId } from "@/lib/bindings";

import type { QueueRowData } from "./queue-status";
import { QUEUE_COLS, QueueRow } from "./queue-row";
import { dropToBeforeId } from "./reorder";

function totalsSummary(rows: QueueRowData[]): string {
  let done = 0;
  let skipped = 0;
  let failed = 0;
  for (const row of rows) {
    if (row.status.kind === "done") done += 1;
    else if (row.status.kind === "skipped") skipped += 1;
    else if (row.status.kind === "failed") failed += 1;
  }
  const parts = [];
  if (done > 0) parts.push(`${done} done`);
  if (skipped > 0) parts.push(`${skipped} skipped`);
  if (failed > 0) parts.push(`${failed} failed`);
  return parts.join(" · ");
}

function SortableRow({
  row,
  index,
  selected,
  onSelect,
}: {
  row: QueueRowData;
  index: number;
  selected: boolean;
  onSelect: () => void;
}) {
  const { ref, handleRef, isDragSource } = useSortable({ id: row.item.id, index });
  return (
    <QueueRow
      ref={ref}
      handleRef={handleRef}
      isDragSource={isDragSource}
      row={row}
      selected={selected}
      onSelect={onSelect}
    />
  );
}

export function QueueTable({
  rows,
  selectedId,
  onSelect,
  onMove,
}: {
  rows: QueueRowData[];
  selectedId: QueueItemId | null;
  onSelect: (id: QueueItemId) => void;
  /** Reorder request in QueueCommand::Move terms (null before = end). */
  onMove?: (itemId: QueueItemId, beforeId: QueueItemId | null) => void;
}) {
  const totalSize = rows.reduce((sum, row) => sum + (row.sizeBytes ?? 0), 0);
  const totalTime = rows.reduce((sum, row) => sum + (row.timeSec ?? 0), 0);
  const table = (
    <TooltipProvider>
      <div className="overflow-hidden rounded-md border border-border">
        <div
          className={cn("border-b border-border py-1 text-xs text-muted-foreground", QUEUE_COLS)}
        >
          <span />
          <span>Name</span>
          <span>Input format</span>
          <span className="text-right">Size</span>
          <span className="text-right">Time</span>
          <span>Operation</span>
          <span>Output</span>
          <span>Status</span>
        </div>
        {rows.map((row, index) =>
          onMove ? (
            <SortableRow
              key={row.item.id}
              row={row}
              index={index}
              selected={row.item.id === selectedId}
              onSelect={() => onSelect(row.item.id)}
            />
          ) : (
            <QueueRow
              key={row.item.id}
              row={row}
              selected={row.item.id === selectedId}
              onSelect={() => onSelect(row.item.id)}
            />
          ),
        )}
        {/* Totals as a sticky-style footer (D11: the twin-Treeview hack dies). */}
        <div className={cn("bg-surface py-1 text-sm font-medium", QUEUE_COLS)}>
          <span />
          <span>
            Total · {rows.length} {rows.length === 1 ? "item" : "items"}
          </span>
          <span />
          <span className="text-right tabular-nums">
            {totalSize > 0 ? formatFileSize(totalSize) : "—"}
          </span>
          <span className="text-right text-muted-foreground tabular-nums">
            {totalTime > 0 ? formatCompactTime(totalTime) : "—"}
          </span>
          <span />
          <span />
          <span className="font-normal text-muted-foreground">{totalsSummary(rows)}</span>
        </div>
      </div>
    </TooltipProvider>
  );
  if (!onMove) return table;
  return (
    <DragDropProvider
      onDragEnd={(event) => {
        if (event.canceled) return;
        const { source, target } = event.operation;
        if (!source || !target) return;
        const beforeId = dropToBeforeId(
          rows,
          Number(source.id) as QueueItemId,
          Number(target.id) as QueueItemId,
        );
        if (beforeId !== undefined) onMove(Number(source.id) as QueueItemId, beforeId);
      }}
    >
      {table}
      {/* Owned, always-mounted drag representation (#33 contract). */}
      <DragOverlay>
        {(source) => {
          const row = rows.find((r) => r.item.id === Number(source.id));
          if (!row) return null;
          return <QueueRow row={row} selected={false} onSelect={() => undefined} isOverlay />;
        }}
      </DragOverlay>
    </DragDropProvider>
  );
}
