import { DndKitReactSpike } from "./dnd-kit-react-spike";

/**
 * Queue-drag spike (#36 D6) — decided 2026-07-19: @dnd-kit/react won over
 * pragmatic-drag-and-drop, dnd-kit legacy, and @hello-pangea/dnd (rubric in
 * the #36 verdict comment). This reference implementation survives until the
 * real queue view lands, then dies.
 */
export default function SpikeDrag() {
  return (
    <div className="flex flex-col gap-4 p-6">
      <div className="flex flex-col gap-1">
        <h1 className="text-2xl">Queue-drag spike</h1>
        <p className="text-sm text-muted-foreground">
          Winner reference: @dnd-kit/react, non-virtualized (content-visibility hedge). ~500 rows,
          two levels. Reorder files within and across folders, drag folders (files follow on drop),
          drag past the viewport edge to auto-scroll, or focus a handle and use Space, arrows,
          Space.
        </p>
      </div>
      <DndKitReactSpike />
    </div>
  );
}
