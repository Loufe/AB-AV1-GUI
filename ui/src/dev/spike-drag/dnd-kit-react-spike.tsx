import { DragDropProvider, DragOverlay } from "@dnd-kit/react";
import { useSortable } from "@dnd-kit/react/sortable";
import { useState } from "react";

import { generateRows, moveRow, type Edge, type SpikeRow } from "./data";
import { ROW_HEIGHT, SpikeRowView } from "./row";

/**
 * Candidate D (#36 D6): @dnd-kit/react — the dnd-kit author's actively
 * developed rewrite (pre-1.0). Virtualized sorting is an open upstream issue
 * (dnd-kit#1720), so this candidate tests the sanctioned fallback instead:
 * all 500 rows rendered plainly with a content-visibility hedge. Keyboard
 * sensor, auto-scroll, and optimistic sort animation are built in; the drag
 * representation is an owned, always-mounted DragOverlay per the #33
 * contract (folders show their file count while dragged). Same honesty gap
 * as legacy: invalid moves preview during the drag and are only rejected on
 * drop.
 */
export function DndKitReactSpike() {
  const [rows, setRows] = useState<SpikeRow[]>(generateRows);

  return (
    <DragDropProvider
      onDragEnd={(event) => {
        if (event.canceled) return;
        const { source, target } = event.operation;
        if (!source || !target || source.id === target.id) return;
        const sourceIndex = rows.findIndex((r) => r.id === source.id);
        const targetIndex = rows.findIndex((r) => r.id === target.id);
        if (sourceIndex < 0 || targetIndex < 0) return;
        const edge: Edge = sourceIndex < targetIndex ? "bottom" : "top";
        const next = moveRow(rows, String(source.id), String(target.id), edge);
        if (next) setRows(next);
      }}
    >
      <div className="h-[480px] overflow-y-auto rounded-md border border-border">
        {rows.map((row, index) => (
          <SortableRow key={row.id} row={row} index={index} />
        ))}
      </div>
      <DragOverlay>
        {(source) => {
          const row = rows.find((r) => r.id === source.id);
          if (!row) return null;
          const fileCount =
            row.kind === "folder" ? rows.filter((r) => r.parentId === row.id).length : 0;
          return (
            <SpikeRowView row={row} isOverlay>
              {row.kind === "folder" && (
                <span className="ml-auto text-xs text-muted-foreground">{fileCount} files</span>
              )}
            </SpikeRowView>
          );
        }}
      </DragOverlay>
    </DragDropProvider>
  );
}

function SortableRow({ row, index }: { row: SpikeRow; index: number }) {
  const { ref, handleRef, isDragSource } = useSortable({ id: row.id, index });

  return (
    <SpikeRowView
      ref={ref}
      row={row}
      isDragSource={isDragSource}
      style={{
        // Non-virtualized fallback hedge: off-screen rows skip layout/paint.
        contentVisibility: "auto",
        containIntrinsicSize: `auto ${ROW_HEIGHT}px`,
      }}
      handleRef={handleRef}
    />
  );
}
