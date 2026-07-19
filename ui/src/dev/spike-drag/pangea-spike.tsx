import { DragDropContext, Draggable, Droppable, type DropResult } from "@hello-pangea/dnd";
import { useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import { generateRows, moveRow, type Edge, type SpikeRow } from "./data";
import { ROW_HEIGHT, SpikeRowView } from "./row";

/**
 * Candidate C (#36 D6): @hello-pangea/dnd (maintained react-beautiful-dnd
 * fork, React 19) + @tanstack/react-virtual. The animated row-shifting the
 * dnd-kit candidate imitates, from its origin, with upstream alive. Virtual
 * mode requires renderClone (the dragged original unmounts) and a manual
 * placeholder allowance in the container height. Same honesty gap as
 * dnd-kit: invalid moves preview live and are only rejected on drop.
 */
export function PangeaSpike() {
  const [rows, setRows] = useState<SpikeRow[]>(generateRows);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8, // virtual mode requires overscan >= 1
  });

  function onDragEnd({ source, destination }: DropResult) {
    if (!destination || destination.index === source.index) return;
    // Destination semantics are "insert at index" (like arrayMove): dragging
    // down lands below the target row, dragging up lands above it.
    const edge: Edge = source.index < destination.index ? "bottom" : "top";
    const next = moveRow(rows, rows[source.index].id, rows[destination.index].id, edge);
    if (next) setRows(next);
  }

  return (
    <DragDropContext onDragEnd={onDragEnd}>
      <Droppable
        droppableId="spike"
        mode="virtual"
        renderClone={(provided, _snapshot, rubric) => {
          const { style, ...dragProps } = provided.draggableProps;
          return (
            <SpikeRowView
              ref={provided.innerRef}
              row={rows[rubric.source.index]}
              isOverlay
              rootProps={dragProps}
              handleProps={provided.dragHandleProps ?? undefined}
              style={style}
            />
          );
        }}
      >
        {(droppableProvided, droppableSnapshot) => (
          <div
            ref={(el) => {
              scrollRef.current = el;
              droppableProvided.innerRef(el);
            }}
            {...droppableProvided.droppableProps}
            className="h-[480px] overflow-y-auto rounded-md border border-border"
          >
            <div
              className="relative"
              style={{
                // Virtual lists can't use the standard placeholder; grow the
                // canvas by one row while a drag is in flight instead.
                height:
                  virtualizer.getTotalSize() +
                  (droppableSnapshot.isUsingPlaceholder ? ROW_HEIGHT : 0),
              }}
            >
              {virtualizer.getVirtualItems().map((v) => {
                const row = rows[v.index];
                return (
                  <Draggable key={row.id} draggableId={row.id} index={v.index}>
                    {(provided, snapshot) => {
                      const { style: dragStyle, ...dragProps } = provided.draggableProps;
                      return (
                        <SpikeRowView
                          ref={provided.innerRef}
                          row={row}
                          isDragSource={snapshot.isDragging}
                          rootProps={dragProps}
                          handleProps={provided.dragHandleProps ?? undefined}
                          style={{
                            position: "absolute",
                            top: v.start,
                            left: 0,
                            right: 0,
                            // Last so the library's displacement transform
                            // (and fixed positioning while dragging) wins.
                            ...dragStyle,
                          }}
                        />
                      );
                    }}
                  </Draggable>
                );
              })}
            </div>
          </div>
        )}
      </Droppable>
    </DragDropContext>
  );
}
