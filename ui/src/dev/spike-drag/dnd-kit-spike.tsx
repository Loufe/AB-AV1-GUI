import {
  closestCenter,
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import { restrictToVerticalAxis } from "@dnd-kit/modifiers";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import { generateRows, moveRow, type Edge, type SpikeRow } from "./data";
import { ROW_HEIGHT, SpikeRowView } from "./row";

/**
 * Candidate A (#36 D6): dnd-kit legacy (6.3.1, frozen) + @tanstack/react-virtual.
 * Known seam: rows are positioned via `top` so dnd-kit owns `transform`, and
 * the active row is force-rendered when virtualization scrolls it out of range.
 * Constraint honesty gap: sortable previews every move; invalid drops are only
 * rejected on drop (moveRow returns null), never signalled during the drag.
 */
export function DndKitSpike() {
  const [rows, setRows] = useState<SpikeRow[]>(generateRows);
  const [activeId, setActiveId] = useState<string | null>(null);
  const ids = useMemo(() => rows.map((r) => r.id), [rows]);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  });

  function handleDragEnd({ active, over }: DragEndEvent) {
    setActiveId(null);
    if (!over || active.id === over.id) return;
    const sourceIndex = rows.findIndex((r) => r.id === active.id);
    const targetIndex = rows.findIndex((r) => r.id === over.id);
    // Sortable semantics are "insert at the target's index": dragging down
    // lands below the target, dragging up lands above it.
    const edge: Edge = sourceIndex < targetIndex ? "bottom" : "top";
    const next = moveRow(rows, String(active.id), String(over.id), edge);
    if (next) setRows(next);
  }

  const virtualItems = virtualizer.getVirtualItems();
  const activeIndex = activeId ? rows.findIndex((r) => r.id === activeId) : -1;
  const activeRow = activeIndex >= 0 ? rows[activeIndex] : null;
  const activeOutOfRange = activeIndex >= 0 && !virtualItems.some((v) => v.index === activeIndex);

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      modifiers={[restrictToVerticalAxis]}
      onDragStart={(e) => setActiveId(String(e.active.id))}
      onDragCancel={() => setActiveId(null)}
      onDragEnd={handleDragEnd}
    >
      <SortableContext items={ids} strategy={verticalListSortingStrategy}>
        <div ref={scrollRef} className="h-[480px] overflow-y-auto rounded-md border border-border">
          <div className="relative" style={{ height: virtualizer.getTotalSize() }}>
            {virtualItems.map((v) => (
              <SortableRow key={rows[v.index].id} row={rows[v.index]} top={v.start} />
            ))}
            {activeOutOfRange &&
              activeRow && (
                // Keep the dragged row mounted while scrolled out of the
                // virtual window, or dnd-kit loses its active node mid-drag.
                <SortableRow row={activeRow} top={activeIndex * ROW_HEIGHT} />
              )}
          </div>
        </div>
      </SortableContext>
      <DragOverlay>{activeRow ? <SpikeRowView row={activeRow} isOverlay /> : null}</DragOverlay>
    </DndContext>
  );
}

function SortableRow({ row, top }: { row: SpikeRow; top: number }) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: row.id });

  return (
    <SpikeRowView
      ref={setNodeRef}
      row={row}
      isDragSource={isDragging}
      style={{
        position: "absolute",
        top,
        left: 0,
        right: 0,
        // Inline (rather than @dnd-kit/utilities' CSS helper) to avoid a
        // direct import of a transitive package under pnpm's strict layout.
        transform: transform ? `translate3d(${transform.x}px, ${transform.y}px, 0)` : undefined,
        transition,
      }}
      handleRef={setActivatorNodeRef}
      // dnd-kit's listener map is typed Record<string, Function>; the actual
      // members are standard button handlers/ARIA attributes.
      handleProps={{ ...attributes, ...listeners } as React.ComponentPropsWithoutRef<"button">}
    />
  );
}
