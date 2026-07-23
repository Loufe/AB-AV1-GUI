import { DragDropProvider, DragOverlay, useDragDropManager } from "@dnd-kit/react";
import { isSortableOperation, useSortable } from "@dnd-kit/react/sortable";
import {
  Accessibility,
  type DragEndEvent,
  type DragOverEvent,
  type DragStartEvent,
} from "@dnd-kit/dom";
import { useEffect } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";
import type { QueueItemId } from "@/lib/bindings";
import { formatDurationMsCompact } from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";
import { cn } from "@/lib/utils";

import { selectedBlockBeforeId } from "./queue-dnd-adapter";
import {
  deriveFolderRuns,
  planFolderRunMove,
  planPendingBlockMove,
  type FolderRun,
  type QueuePresentationMode,
  type QueueReorderPlan,
} from "./queue-interaction-planner";
import { basename, type QueueRowData } from "./queue-status";
import { QUEUE_COLS, QueueRow, type QueueRowAction, type QueueRowActions } from "./queue-row";

type DragData =
  | { kind: "item"; itemId: QueueItemId; label: string }
  | {
      kind: "folder";
      runId: FolderRun["id"];
      firstPendingId: QueueItemId;
      label: string;
    };

export interface QueueReorderFocus {
  key: string;
  label: string;
  fallbackKey?: string;
}

const itemFocus = (id: QueueItemId, label: string): QueueReorderFocus => ({
  key: `item:${id}`,
  label,
});
const folderHandleFocus = (
  id: FolderRun["id"],
  firstPendingId: QueueItemId | undefined,
  label: string,
): QueueReorderFocus => ({
  key: `folder:${id}:handle`,
  label,
  fallbackKey: firstPendingId === undefined ? undefined : `item:${firstPendingId}`,
});
const folderActionFocus = (
  id: FolderRun["id"],
  firstPendingId: QueueItemId,
  label: string,
  action: "up" | "down" | "top" | "bottom",
): QueueReorderFocus => ({
  key: `folder:${id}:${action}`,
  label,
  fallbackKey: `item:${firstPendingId}`,
});

const queueAccessibility = Accessibility.configure({
  screenReaderInstructions: {
    draggable:
      "Press Space or Enter to pick up. Use arrow keys to move. Press Space or Enter to drop, or Escape to cancel.",
  },
  announcements: {
    dragstart: ({ operation }: DragStartEvent) => {
      const data = operation.source?.data as DragData | undefined;
      return data === undefined ? undefined : `Picked up ${data.label}.`;
    },
    dragover: ({ operation }: DragOverEvent) => {
      if (!isSortableOperation(operation)) return undefined;
      const { source } = operation;
      if (source === null) return undefined;
      const data = source.data as DragData;
      return `${data.label}, position ${source.index + 1}.`;
    },
    dragend: ({ operation, canceled }: DragEndEvent) => {
      const data = operation.source?.data as DragData | undefined;
      if (data === undefined) return undefined;
      return canceled ? `Cancelled moving ${data.label}.` : `Dropped ${data.label}.`;
    },
  },
});

function selectionMode(
  event: React.MouseEvent | React.KeyboardEvent,
): "replace" | "toggle" | "range" {
  if (event.shiftKey) return "range";
  if (event.ctrlKey || event.metaKey) return "toggle";
  return "replace";
}

function totalsSummary(rows: readonly QueueRowData[]): string {
  const counts = { done: 0, skipped: 0, failed: 0 };
  for (const row of rows) {
    if (row.status.kind === "done") counts.done += 1;
    else if (row.status.kind === "skipped") counts.skipped += 1;
    else if (row.status.kind === "failed") counts.failed += 1;
  }
  return [
    counts.done && `${counts.done} done`,
    counts.skipped && `${counts.skipped} skipped`,
    counts.failed && `${counts.failed} failed`,
  ]
    .filter(Boolean)
    .join(" · ");
}

function SortableRow({
  row,
  index,
  group,
  selected,
  onSelect,
  actions,
}: {
  row: QueueRowData;
  index: number;
  group: string;
  selected: boolean;
  onSelect: (event: React.MouseEvent | React.KeyboardEvent) => void;
  actions: QueueRowActions;
}) {
  const { ref, handleRef, isDragSource } = useSortable<DragData>({
    id: `queue-item:${row.item.id}`,
    index,
    group,
    type: "queue-item",
    accept: "queue-item",
    data: { kind: "item", itemId: row.item.id, label: basename(row.item.input) },
  });
  return (
    <QueueRow
      ref={ref}
      handleRef={handleRef}
      isDragSource={isDragSource}
      row={row}
      selected={selected}
      onSelect={onSelect}
      actions={actions}
      focusKey={itemFocus(row.item.id, basename(row.item.input)).key}
    />
  );
}

function FolderHeaderContent({
  run,
  handleRef,
  isDragSource = false,
  movableIndex,
  movableCount,
  onMove,
}: {
  run: FolderRun;
  handleRef?: React.Ref<HTMLButtonElement>;
  isDragSource?: boolean;
  movableIndex?: number;
  movableCount?: number;
  onMove?: (destination: "up" | "down" | "top" | "bottom") => void;
}) {
  return (
    <div
      role="row"
      className={cn(
        "flex h-8 items-center gap-2 border-b border-border bg-surface px-2 text-xs font-medium",
        isDragSource && "opacity-40",
      )}
    >
      <span role="cell">
        <button
          ref={handleRef}
          type="button"
          data-queue-folder-handle={run.id}
          data-queue-focus={folderHandleFocus(run.id, run.pendingIds[0], run.parent.label).key}
          aria-label={`Reorder folder ${run.parent.label}`}
          disabled={handleRef === undefined}
          className="flex size-6 cursor-grab items-center justify-center rounded text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-30"
        >
          ⋮⋮
        </button>
      </span>
      <span role="rowheader" className="truncate" title={run.parent.key}>
        {run.parent.label}
      </span>
      <span role="cell" className="text-muted-foreground">
        {run.itemIds.length} {run.itemIds.length === 1 ? "item" : "items"}
      </span>
      {onMove !== undefined && movableIndex !== undefined && movableCount !== undefined && (
        <div
          role="cell"
          className="ml-auto flex items-center"
          aria-label={`Move folder ${run.parent.label}`}
        >
          {(["top", "up", "down", "bottom"] as const).map((destination) => {
            const atTop = movableIndex === 0;
            const atBottom = movableIndex === movableCount - 1;
            const disabled = destination === "top" || destination === "up" ? atTop : atBottom;
            const label =
              destination === "top" ? "top" : destination === "bottom" ? "bottom" : destination;
            return (
              <button
                key={destination}
                type="button"
                aria-label={`Move ${run.parent.label} ${label}`}
                data-queue-focus={
                  folderActionFocus(run.id, run.pendingIds[0]!, run.parent.label, destination).key
                }
                disabled={disabled}
                onClick={() => onMove(destination)}
                className="flex size-6 items-center justify-center rounded text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-30"
              >
                {destination === "top"
                  ? "⇈"
                  : destination === "up"
                    ? "↑"
                    : destination === "down"
                      ? "↓"
                      : "⇊"}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

function SortableFolderHeader({
  run,
  index,
  count,
  onMove,
}: {
  run: FolderRun;
  index: number;
  count: number;
  onMove: (destination: "up" | "down" | "top" | "bottom") => void;
}) {
  const { ref, handleRef, isDragSource } = useSortable<DragData>({
    id: `queue-folder:${run.id}`,
    index,
    group: "folder-runs",
    type: "queue-folder-run",
    accept: "queue-folder-run",
    data: {
      kind: "folder",
      runId: run.id,
      firstPendingId: run.pendingIds[0]!,
      label: run.parent.label,
    },
  });
  return (
    <div ref={ref}>
      <FolderHeaderContent
        run={run}
        handleRef={handleRef}
        isDragSource={isDragSource}
        movableIndex={index}
        movableCount={count}
        onMove={onMove}
      />
    </div>
  );
}

function ActiveDragCancellation({ requested }: { requested: boolean }) {
  const manager = useDragDropManager();

  useEffect(() => {
    if (!requested || manager?.dragOperation.source == null) return;
    manager.actions.stop({ canceled: true });
  }, [manager, requested]);

  return null;
}

export function QueueTable({
  rows,
  grouped,
  selectedIds,
  onSelect,
  onPlan,
  onDragStart,
  onDragCancel,
  onDragNoop,
  cancelActiveDrag,
  reorderEnabled,
  actionsDisabled,
  editingAllowed,
  recoveryAllowed,
  durableActionsAllowed,
  onRowAction,
}: {
  rows: readonly QueueRowData[];
  grouped: boolean;
  selectedIds: ReadonlySet<QueueItemId>;
  onSelect: (id: QueueItemId, mode: "replace" | "toggle" | "range") => void;
  onPlan: (plan: QueueReorderPlan, focus: QueueReorderFocus) => void;
  onDragStart: (focus: QueueReorderFocus) => void;
  onDragCancel: () => void;
  onDragNoop: () => void;
  cancelActiveDrag: boolean;
  reorderEnabled: boolean;
  actionsDisabled: boolean;
  editingAllowed: boolean;
  recoveryAllowed: boolean;
  durableActionsAllowed: boolean;
  onRowAction: (id: QueueItemId, action: QueueRowAction) => void;
}) {
  const mode: QueuePresentationMode = grouped ? "grouped" : "ungrouped";
  const runs = deriveFolderRuns(rows);
  const movableRuns = runs.filter((run) => run.pendingIds.length > 0);
  const rowsById = new Map(rows.map((row) => [row.item.id, row]));
  const pending = rows.filter((row) => row.item.state === "Queued");
  const pendingIndex = new Map(pending.map((row, index) => [row.item.id, index]));
  const pendingIds = pending.map((row) => row.item.id);
  const totalSize = rows.reduce((sum, row) => sum + (row.sizeBytes ?? 0), 0);
  const totalTime = rows.reduce((sum, row) => sum + (row.timeMs ?? 0), 0);

  const moveFolderRun = (run: FolderRun, destination: "up" | "down" | "top" | "bottom") => {
    const sourceIndex = movableRuns.findIndex((candidate) => candidate.id === run.id);
    const others = movableRuns.filter((candidate) => candidate.id !== run.id);
    let before: FolderRun["id"] | null;
    if (destination === "top") before = others[0]?.id ?? null;
    else if (destination === "bottom") before = null;
    else if (destination === "up") before = others[sourceIndex - 1]?.id ?? run.id;
    else before = others[sourceIndex + 1]?.id ?? null;
    onPlan(
      planFolderRunMove(rows, run.id, before, mode),
      folderActionFocus(run.id, run.pendingIds[0]!, run.parent.label, destination),
    );
  };

  const renderedRows = grouped
    ? runs.flatMap((run) => [
        run.pendingIds.length > 0 && reorderEnabled ? (
          <SortableFolderHeader
            key={`header:${run.id}`}
            run={run}
            index={movableRuns.indexOf(run)}
            count={movableRuns.length}
            onMove={(destination) => moveFolderRun(run, destination)}
          />
        ) : (
          <FolderHeaderContent key={`header:${run.id}`} run={run} />
        ),
        ...run.itemIds.flatMap((itemId) => {
          const row = rowsById.get(itemId);
          if (row === undefined) return [];
          const index = row.item.state === "Queued" ? run.pendingIds.indexOf(itemId) : 0;
          const select = (event: React.MouseEvent | React.KeyboardEvent) =>
            onSelect(itemId, selectionMode(event));
          const finished = row.item.state !== "Queued" && "Finished" in row.item.state;
          const actions: QueueRowActions = {
            disabled: actionsDisabled,
            editable: editingAllowed && row.item.state === "Queued",
            retryable: durableActionsAllowed && finished,
            recoverable: recoveryAllowed && finished,
            removable: durableActionsAllowed && (row.item.state === "Queued" || finished),
            onAction: (action) => onRowAction(itemId, action),
          };
          return [
            row.item.state === "Queued" && reorderEnabled ? (
              <SortableRow
                key={itemId}
                row={row}
                index={pendingIndex.get(itemId) ?? index}
                group="queue-items"
                selected={selectedIds.has(itemId)}
                onSelect={select}
                actions={actions}
              />
            ) : (
              <QueueRow
                key={itemId}
                row={row}
                selected={selectedIds.has(itemId)}
                onSelect={select}
                actions={actions}
              />
            ),
          ];
        }),
      ])
    : rows.map((row) => {
        const select = (event: React.MouseEvent | React.KeyboardEvent) =>
          onSelect(row.item.id, selectionMode(event));
        const finished = row.item.state !== "Queued" && "Finished" in row.item.state;
        const actions: QueueRowActions = {
          disabled: actionsDisabled,
          editable: editingAllowed && row.item.state === "Queued",
          retryable: durableActionsAllowed && finished,
          recoverable: recoveryAllowed && finished,
          removable: durableActionsAllowed && (row.item.state === "Queued" || finished),
          onAction: (action) => onRowAction(row.item.id, action),
        };
        return row.item.state === "Queued" && reorderEnabled ? (
          <SortableRow
            key={row.item.id}
            row={row}
            index={pendingIndex.get(row.item.id) ?? 0}
            group="queue-items"
            selected={selectedIds.has(row.item.id)}
            onSelect={select}
            actions={actions}
          />
        ) : (
          <QueueRow
            key={row.item.id}
            row={row}
            selected={selectedIds.has(row.item.id)}
            onSelect={select}
            actions={actions}
          />
        );
      });

  const table = (
    <TooltipProvider>
      <div
        role="table"
        aria-label="Conversion queue"
        className="overflow-hidden rounded-md border border-border"
        style={{ contentVisibility: "auto", containIntrinsicSize: "auto 1200px" }}
      >
        <div
          role="row"
          className={cn("border-b border-border py-1 text-xs text-muted-foreground", QUEUE_COLS)}
        >
          <span role="columnheader" />
          <span role="columnheader">Name</span>
          <span role="columnheader">Input format</span>
          <span role="columnheader" className="text-right">
            Size
          </span>
          <span role="columnheader" className="text-right">
            Time
          </span>
          <span role="columnheader">Operation</span>
          <span role="columnheader">Output</span>
          <span role="columnheader">Status</span>
        </div>
        {renderedRows}
        <div role="row" className={cn("bg-surface py-1 text-sm font-medium", QUEUE_COLS)}>
          <span role="cell" />
          <span role="cell">
            Total · {rows.length} {rows.length === 1 ? "item" : "items"}
          </span>
          <span role="cell" />
          <span role="cell" className="text-right tabular-nums">
            {totalSize > 0 ? formatFileSize(totalSize) : "—"}
          </span>
          <span role="cell" className="text-right text-muted-foreground tabular-nums">
            {totalTime > 0 ? formatDurationMsCompact(totalTime) : "—"}
          </span>
          <span role="cell" />
          <span role="cell" />
          <span role="cell" className="font-normal text-muted-foreground">
            {totalsSummary(rows)}
          </span>
        </div>
      </div>
    </TooltipProvider>
  );

  return (
    <DragDropProvider
      plugins={(defaults) => [...defaults, queueAccessibility]}
      onDragStart={(event) => {
        const data = event.operation.source?.data as DragData | undefined;
        if (data === undefined) return;
        const focus =
          data.kind === "item"
            ? itemFocus(data.itemId, data.label)
            : folderHandleFocus(data.runId, data.firstPendingId, data.label);
        queueMicrotask(() => onDragStart(focus));
      }}
      onDragEnd={(event) => {
        const cancel = () => queueMicrotask(onDragCancel);
        if (event.canceled) {
          cancel();
          return;
        }
        if (!isSortableOperation(event.operation)) {
          cancel();
          return;
        }
        const { source } = event.operation;
        if (source === null) {
          cancel();
          return;
        }
        const data = source.data as DragData;
        if (source.initialIndex === source.index && source.initialGroup === source.group) {
          queueMicrotask(onDragNoop);
          return;
        }
        if (data.kind === "folder") {
          const others = movableRuns.filter((run) => run.id !== data.runId);
          const before = others[source.index]?.id ?? null;
          const plan = planFolderRunMove(rows, data.runId, before, mode);
          const focus = folderHandleFocus(data.runId, data.firstPendingId, data.label);
          queueMicrotask(() => onPlan(plan, focus));
          return;
        }
        const moving = selectedIds.has(data.itemId) ? selectedIds : new Set([data.itemId]);
        const beforeId = selectedBlockBeforeId(
          pendingIds,
          data.itemId,
          source.initialIndex,
          source.index,
          moving,
        );
        if (beforeId === undefined) {
          cancel();
          return;
        }
        const plan = planPendingBlockMove(rows, moving, beforeId, mode);
        const focus = itemFocus(data.itemId, data.label);
        queueMicrotask(() => onPlan(plan, focus));
      }}
    >
      <ActiveDragCancellation requested={cancelActiveDrag} />
      {table}
      <DragOverlay dropAnimation={null}>
        {(source) => {
          const data = source.data as DragData;
          if (data.kind === "folder") {
            const run = runs.find((candidate) => candidate.id === data.runId);
            return run === undefined ? null : (
              <div
                aria-hidden="true"
                className="rounded border bg-elevated px-3 py-2 text-sm shadow-lg"
              >
                {run.parent.label} · {run.pendingIds.length} pending
              </div>
            );
          }
          const row = rowsById.get(data.itemId);
          return row === undefined ? null : (
            <QueueRow row={row} selected={false} onSelect={() => undefined} isOverlay />
          );
        }}
      </DragOverlay>
    </DragDropProvider>
  );
}
