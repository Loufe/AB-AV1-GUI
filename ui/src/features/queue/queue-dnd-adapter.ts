import type { QueueItemId } from "@/lib/bindings";

/**
 * Translate dnd-kit's optimistic one-item index into a destination for our
 * stable selected block. The optimistic list moves only the grabbed row, so
 * using its index directly for a multi-selection misplaces the block.
 */
export function selectedBlockBeforeId(
  pendingIds: readonly QueueItemId[],
  itemId: QueueItemId,
  initialIndex: number,
  index: number,
  movingIds: ReadonlySet<QueueItemId>,
): QueueItemId | null | undefined {
  if (pendingIds[initialIndex] !== itemId) return undefined;

  const oneItemOrder = [...pendingIds];
  oneItemOrder.splice(initialIndex, 1);
  const optimisticIndex = Math.max(0, Math.min(index, oneItemOrder.length));
  oneItemOrder.splice(optimisticIndex, 0, itemId);

  const draggedIndex = oneItemOrder.indexOf(itemId);
  const insertionIndex = oneItemOrder
    .slice(0, draggedIndex)
    .filter((id) => !movingIds.has(id)).length;
  const remainder = pendingIds.filter((id) => !movingIds.has(id));
  return remainder[insertionIndex] ?? null;
}
