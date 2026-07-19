import { autoScrollForElements } from "@atlaskit/pragmatic-drag-and-drop-auto-scroll/element";
import {
  attachClosestEdge,
  extractClosestEdge,
} from "@atlaskit/pragmatic-drag-and-drop-hitbox/closest-edge";
import * as liveRegion from "@atlaskit/pragmatic-drag-and-drop-live-region";
import { DropIndicator } from "@atlaskit/pragmatic-drag-and-drop-react-drop-indicator/box";
import { combine } from "@atlaskit/pragmatic-drag-and-drop/combine";
import {
  draggable,
  dropTargetForElements,
  monitorForElements,
} from "@atlaskit/pragmatic-drag-and-drop/element/adapter";
import { pointerOutsideOfPreview } from "@atlaskit/pragmatic-drag-and-drop/element/pointer-outside-of-preview";
import { setCustomNativeDragPreview } from "@atlaskit/pragmatic-drag-and-drop/element/set-custom-native-drag-preview";
import { ArrowDown, ArrowUp } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { useVirtualizer } from "@tanstack/react-virtual";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

import { generateRows, moveByStep, moveRow, type Edge, type SpikeRow } from "./data";
import { ROW_HEIGHT, SpikeRowView } from "./row";

/**
 * Candidate B (#36 D6): Atlassian pragmatic-drag-and-drop + @tanstack/react-virtual.
 * Native HTML5 drag with a custom rendered preview (#33 requires an owned
 * drag representation, not the OS screenshot). Keyboard path is pragmatic's
 * recommended alternative-control pattern: the handle opens a move menu and
 * outcomes are announced via the live-region package.
 */
export function PragmaticSpike() {
  const [rows, setRows] = useState<SpikeRow[]>(generateRows);
  const [dragging, setDragging] = useState(false);
  const rowsRef = useRef(rows);
  rowsRef.current = rows;

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  });

  // Live validity for the drop indicator: only show an edge the reorder
  // logic would actually accept (dnd-kit candidate cannot do this).
  const canDrop = useCallback(
    (sourceId: string, targetId: string, edge: Edge) =>
      moveRow(rowsRef.current, sourceId, targetId, edge) !== null,
    [],
  );

  const applyMove = useCallback((sourceId: string, targetId: string, edge: Edge) => {
    const current = rowsRef.current;
    const next = moveRow(current, sourceId, targetId, edge);
    if (!next) return;
    setRows(next);
    const source = current.find((r) => r.id === sourceId);
    const index = next.findIndex((r) => r.id === sourceId);
    liveRegion.announce(
      `Moved ${source?.label ?? sourceId} to position ${index + 1} of ${next.length}.`,
    );
  }, []);

  useEffect(() => {
    const scrollEl = scrollRef.current;
    if (!scrollEl) return;
    return combine(
      monitorForElements({
        canMonitor: ({ source }) => typeof source.data.spikeRowId === "string",
        onDragStart: () => setDragging(true),
        onDrop({ source, location }) {
          setDragging(false);
          const target = location.current.dropTargets[0];
          if (!target || typeof target.data.spikeRowId !== "string") return;
          const edge = extractClosestEdge(target.data);
          if (edge !== "top" && edge !== "bottom") return;
          applyMove(String(source.data.spikeRowId), target.data.spikeRowId, edge);
        },
      }),
      autoScrollForElements({
        element: scrollEl,
        canScroll: ({ source }) => typeof source.data.spikeRowId === "string",
      }),
      liveRegion.cleanup,
    );
  }, [applyMove]);

  const moveStep = useCallback((id: string, dir: -1 | 1) => {
    const next = moveByStep(rowsRef.current, id, dir);
    if (!next) {
      liveRegion.announce("Cannot move further.");
      return;
    }
    setRows(next);
    const index = next.findIndex((r) => r.id === id);
    liveRegion.announce(`Moved to position ${index + 1} of ${next.length}.`);
  }, []);

  return (
    <div
      ref={scrollRef}
      className="h-[480px] overflow-y-auto rounded-md border border-border"
      // Point the Atlassian drop indicator's design token at our primary.
      style={{ "--ds-border-selected": "var(--primary)" } as React.CSSProperties}
    >
      <div className="relative" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((v) => (
          <PragmaticRow
            key={rows[v.index].id}
            row={rows[v.index]}
            top={v.start}
            dragging={dragging}
            canDrop={canDrop}
            onMoveStep={moveStep}
          />
        ))}
      </div>
    </div>
  );
}

type RowState = { kind: "idle" } | { kind: "dragging" } | { kind: "over"; edge: Edge };

interface PragmaticRowProps {
  row: SpikeRow;
  top: number;
  /** True while any spike row is being dragged (suppresses the handle menu). */
  dragging: boolean;
  canDrop: (sourceId: string, targetId: string, edge: Edge) => boolean;
  onMoveStep: (id: string, dir: -1 | 1) => void;
}

function PragmaticRow({ row, top, dragging, canDrop, onMoveStep }: PragmaticRowProps) {
  const rowRef = useRef<HTMLDivElement | null>(null);
  const handleRef = useRef<HTMLButtonElement | null>(null);
  const [state, setState] = useState<RowState>({ kind: "idle" });

  useEffect(() => {
    const element = rowRef.current;
    const dragHandle = handleRef.current;
    if (!element || !dragHandle) return;
    return combine(
      draggable({
        element,
        dragHandle,
        getInitialData: () => ({ spikeRowId: row.id }),
        onGenerateDragPreview({ nativeSetDragImage }) {
          setCustomNativeDragPreview({
            nativeSetDragImage,
            getOffset: pointerOutsideOfPreview({ x: "12px", y: "8px" }),
            render({ container }) {
              const root = createRoot(container);
              root.render(<SpikeRowView row={row} isOverlay />);
              return () => root.unmount();
            },
          });
        },
        onDragStart: () => setState({ kind: "dragging" }),
        onDrop: () => setState({ kind: "idle" }),
      }),
      dropTargetForElements({
        element,
        canDrop: ({ source }) =>
          typeof source.data.spikeRowId === "string" && source.data.spikeRowId !== row.id,
        getData: ({ input, element: el }) =>
          attachClosestEdge(
            { spikeRowId: row.id },
            {
              input,
              element: el,
              allowedEdges: ["top", "bottom"],
            },
          ),
        onDrag({ self, source }) {
          const edge = extractClosestEdge(self.data);
          const sourceId = String(source.data.spikeRowId);
          if ((edge === "top" || edge === "bottom") && canDrop(sourceId, row.id, edge)) {
            setState((prev) =>
              prev.kind === "over" && prev.edge === edge ? prev : { kind: "over", edge },
            );
          } else {
            setState((prev) => (prev.kind === "idle" ? prev : { kind: "idle" }));
          }
        },
        onDragLeave: () => setState({ kind: "idle" }),
        onDrop: () => setState({ kind: "idle" }),
      }),
    );
  }, [row, canDrop]);

  return (
    <SpikeRowView
      ref={rowRef}
      row={row}
      isDragSource={state.kind === "dragging"}
      style={{ position: "absolute", top, left: 0, right: 0 }}
      handleRef={handleRef}
    >
      {state.kind === "over" && <DropIndicator edge={state.edge} gap="1px" />}
      {!dragging && <MoveMenu label={row.label} onMoveStep={(dir) => onMoveStep(row.id, dir)} />}
    </SpikeRowView>
  );
}

/**
 * Alternative-control keyboard path: pragmatic deliberately has no simulated
 * keyboard drag; the handle's context menu performs the same moves.
 */
function MoveMenu({ label, onMoveStep }: { label: string; onMoveStep: (dir: -1 | 1) => void }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={`Move ${label}`}
        className="ml-auto rounded px-1 text-xs text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      >
        Move
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => onMoveStep(-1)}>
          <ArrowUp aria-hidden="true" /> Move up
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => onMoveStep(1)}>
          <ArrowDown aria-hidden="true" /> Move down
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
