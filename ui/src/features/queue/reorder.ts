import type { QueueItemId } from "@/lib/bindings";

import type { QueueRowData } from "./queue-status";

/**
 * Reorders `itemId` to sit before `beforeId` (or at the end when null),
 * mirroring the engine's QueueCommand::Move semantics: a missing source is a
 * no-op, a missing target appends. Returns a new array.
 */
export function moveRowBefore(
  rows: QueueRowData[],
  itemId: QueueItemId,
  beforeId: QueueItemId | null,
): QueueRowData[] {
  const source = rows.findIndex((row) => row.item.id === itemId);
  if (source < 0) return rows;
  const next = rows.slice();
  const [moved] = next.splice(source, 1);
  const destination =
    beforeId === null ? next.length : next.findIndex((row) => row.item.id === beforeId);
  next.splice(destination < 0 ? next.length : destination, 0, moved);
  return next;
}

/**
 * Translates a drop on `targetId` into Move semantics: dragging downward
 * lands after the target, upward lands before it. Returns the `before` id
 * (null = end of queue), or undefined when the drop is a no-op. Rows are
 * forwarded regardless of state; the engine reducer clamps a drop above the
 * active or a finished row to the first pending slot.
 */
export function dropToBeforeId(
  rows: QueueRowData[],
  sourceId: QueueItemId,
  targetId: QueueItemId,
): QueueItemId | null | undefined {
  const source = rows.findIndex((row) => row.item.id === sourceId);
  const target = rows.findIndex((row) => row.item.id === targetId);
  if (source < 0 || target < 0 || source === target) return undefined;
  if (source < target) {
    const after = rows[target + 1];
    return after === undefined ? null : after.item.id;
  }
  return targetId;
}
