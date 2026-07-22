import type { QueueItemId } from "@/lib/bindings";

export interface QueueSelectionState {
  selectedIds: ReadonlySet<QueueItemId>;
  anchorId: QueueItemId | null;
}

export type QueueSelectionMode = "replace" | "toggle" | "range";

export function emptyQueueSelection(): QueueSelectionState {
  return { selectedIds: new Set(), anchorId: null };
}

/** Apply click, Ctrl/Cmd-toggle, or Shift-range semantics over authoritative visible IDs. */
export function applyQueueSelection(
  current: QueueSelectionState,
  targetId: QueueItemId,
  visibleIds: readonly QueueItemId[],
  mode: QueueSelectionMode,
): QueueSelectionState {
  const targetIndex = visibleIds.indexOf(targetId);
  if (targetIndex < 0) return current;

  if (mode === "replace") {
    return { selectedIds: new Set([targetId]), anchorId: targetId };
  }

  if (mode === "toggle") {
    const selectedIds = new Set(current.selectedIds);
    if (selectedIds.has(targetId)) selectedIds.delete(targetId);
    else selectedIds.add(targetId);
    return { selectedIds, anchorId: selectedIds.size === 0 ? null : targetId };
  }

  const anchorIndex = current.anchorId === null ? -1 : visibleIds.indexOf(current.anchorId);
  if (anchorIndex < 0) {
    return { selectedIds: new Set([targetId]), anchorId: targetId };
  }
  const start = Math.min(anchorIndex, targetIndex);
  const end = Math.max(anchorIndex, targetIndex);
  return {
    selectedIds: new Set(visibleIds.slice(start, end + 1)),
    anchorId: current.anchorId,
  };
}

/** Drop IDs removed by an authoritative snapshot while retaining every survivor. */
export function pruneQueueSelection(
  current: QueueSelectionState,
  visibleIds: readonly QueueItemId[],
): QueueSelectionState {
  const visible = new Set(visibleIds);
  const survivors = visibleIds.filter((id) => current.selectedIds.has(id));
  const anchorId =
    current.anchorId !== null && visible.has(current.anchorId)
      ? current.anchorId
      : (survivors[0] ?? null);
  if (
    survivors.length === current.selectedIds.size &&
    anchorId === current.anchorId &&
    survivors.every((id) => current.selectedIds.has(id))
  ) {
    return current;
  }
  return { selectedIds: new Set(survivors), anchorId };
}
