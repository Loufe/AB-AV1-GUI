import type { QueueItem, QueueItemId } from "@/lib/bindings";

import { extractParentPath, type ParentPath } from "./parent-path";

declare const folderRunIdBrand: unique symbol;
export type FolderRunId = string & { readonly [folderRunIdBrand]: "FolderRunId" };
export type QueuePresentationMode = "grouped" | "ungrouped";
export type SelectedMoveDestination = "up" | "down" | "top" | "bottom";
export type QueuePlannerItem = Readonly<Pick<QueueItem, "id" | "input" | "state">>;
export interface QueuePlannerRow {
  readonly item: QueuePlannerItem;
}

export interface FolderRun {
  id: FolderRunId;
  parent: ParentPath;
  firstItemId: QueueItemId;
  itemIds: readonly QueueItemId[];
  pendingIds: readonly QueueItemId[];
}

export type QueuePlanNoopReason =
  | "empty-selection"
  | "unknown-selection"
  | "frozen-selection"
  | "target-not-pending"
  | "target-selected"
  | "unknown-run"
  | "frozen-run"
  | "boundary"
  | "identity";

export type QueueReorderPlan =
  | {
      kind: "noop";
      pendingOrder: readonly QueueItemId[];
      reason: QueuePlanNoopReason;
    }
  | {
      kind: "legal";
      pendingOrder: readonly QueueItemId[];
      movedIds: readonly QueueItemId[];
    }
  | {
      kind: "cross-folder";
      /** Immutable attempted order retained across the confirmation dialog. */
      pendingOrder: readonly QueueItemId[];
      movedIds: readonly QueueItemId[];
    };

export function pendingQueueIds(rows: readonly QueuePlannerRow[]): QueueItemId[] {
  return rows.filter((row) => row.item.state === "Queued").map((row) => row.item.id);
}

export function folderRunId(parentKey: string, firstItemId: QueueItemId): FolderRunId {
  return JSON.stringify([parentKey, firstItemId]) as FolderRunId;
}

/** Derive presentation-only folder headers from contiguous authoritative rows. */
export function deriveFolderRuns(rows: readonly QueuePlannerRow[]): FolderRun[] {
  const runs: FolderRun[] = [];
  for (const row of rows) {
    const parent = extractParentPath(row.item.input);
    const previous = runs.at(-1);
    if (previous !== undefined && previous.parent.key === parent.key) {
      const itemIds = [...previous.itemIds, row.item.id];
      const pendingIds =
        row.item.state === "Queued" ? [...previous.pendingIds, row.item.id] : previous.pendingIds;
      runs[runs.length - 1] = { ...previous, itemIds, pendingIds };
      continue;
    }
    runs.push({
      id: folderRunId(parent.key, row.item.id),
      parent,
      firstItemId: row.item.id,
      itemIds: [row.item.id],
      pendingIds: row.item.state === "Queued" ? [row.item.id] : [],
    });
  }
  return runs;
}

function sameOrder(left: readonly QueueItemId[], right: readonly QueueItemId[]): boolean {
  return left.length === right.length && left.every((id, index) => id === right[index]);
}

function parentKeysById(rows: readonly QueuePlannerRow[]): Map<QueueItemId, string> {
  const keys = new Map<QueueItemId, string>();
  for (const row of rows) {
    keys.set(row.item.id, extractParentPath(row.item.input).key);
  }
  return keys;
}

function parentRunCounts(parentSequence: readonly string[]): Map<string, number> {
  const counts = new Map<string, number>();
  let previous: string | undefined;
  for (const parent of parentSequence) {
    if (parent !== previous) counts.set(parent, (counts.get(parent) ?? 0) + 1);
    previous = parent;
  }
  return counts;
}

/** Mirror the fold: frozen relative order becomes a prefix, followed by pending order. */
function fullParentSequence(
  rows: readonly QueuePlannerRow[],
  pendingOrder: readonly QueueItemId[],
): string[] {
  const parentKeys = parentKeysById(rows);
  const frozenParents = rows
    .filter((row) => row.item.state !== "Queued")
    .map((row) => parentKeys.get(row.item.id) ?? extractParentPath(row.item.input).key);
  const pendingParents = pendingOrder
    .map((id) => parentKeys.get(id))
    .filter((parent): parent is string => parent !== undefined);
  return [...frozenParents, ...pendingParents];
}

function increasesFolderFragmentation(
  rows: readonly QueuePlannerRow[],
  before: readonly QueueItemId[],
  after: readonly QueueItemId[],
): boolean {
  // Grouped presentation may already contain repeated runs of one parent.
  // A move is legal when it preserves or reduces that existing fragmentation;
  // demanding one global run here would silently turn presentation into regroup.
  const beforeCounts = parentRunCounts(fullParentSequence(rows, before));
  const afterCounts = parentRunCounts(fullParentSequence(rows, after));
  for (const [parent, count] of afterCounts) {
    if (count > (beforeCounts.get(parent) ?? 0)) return true;
  }
  return false;
}

function classifyMove(
  rows: readonly QueuePlannerRow[],
  before: readonly QueueItemId[],
  after: readonly QueueItemId[],
  movedIds: readonly QueueItemId[],
  mode: QueuePresentationMode,
): QueueReorderPlan {
  if (sameOrder(before, after)) return { kind: "noop", pendingOrder: before, reason: "identity" };
  if (mode === "grouped" && increasesFolderFragmentation(rows, before, after)) {
    return { kind: "cross-folder", pendingOrder: after, movedIds };
  }
  return { kind: "legal", pendingOrder: after, movedIds };
}

interface ValidSelection {
  pendingOrder: QueueItemId[];
  movedIds: QueueItemId[];
}

function validateSelection(
  rows: readonly QueuePlannerRow[],
  selectedIds: ReadonlySet<QueueItemId>,
): ValidSelection | QueueReorderPlan {
  const pendingOrder = pendingQueueIds(rows);
  if (selectedIds.size === 0) {
    return { kind: "noop", pendingOrder, reason: "empty-selection" };
  }
  const rowsById = new Map(rows.map((row) => [row.item.id, row]));
  for (const id of selectedIds) {
    const row = rowsById.get(id);
    if (row === undefined) return { kind: "noop", pendingOrder, reason: "unknown-selection" };
    if (row.item.state !== "Queued") {
      return { kind: "noop", pendingOrder, reason: "frozen-selection" };
    }
  }
  return { pendingOrder, movedIds: pendingOrder.filter((id) => selectedIds.has(id)) };
}

function moveBlockToIndex(
  rows: readonly QueuePlannerRow[],
  selection: ValidSelection,
  insertionIndex: number,
  mode: QueuePresentationMode,
): QueueReorderPlan {
  const moved = new Set(selection.movedIds);
  const remainder = selection.pendingOrder.filter((id) => !moved.has(id));
  const destination = Math.max(0, Math.min(insertionIndex, remainder.length));
  const after = [
    ...remainder.slice(0, destination),
    ...selection.movedIds,
    ...remainder.slice(destination),
  ];
  return classifyMove(rows, selection.pendingOrder, after, selection.movedIds, mode);
}

/** Plan a file or stable selected block before another pending ID, or at the end with null. */
export function planPendingBlockMove(
  rows: readonly QueuePlannerRow[],
  selectedIds: ReadonlySet<QueueItemId>,
  beforeId: QueueItemId | null,
  mode: QueuePresentationMode,
): QueueReorderPlan {
  const selection = validateSelection(rows, selectedIds);
  if ("kind" in selection) return selection;
  if (beforeId !== null && !selection.pendingOrder.includes(beforeId)) {
    return { kind: "noop", pendingOrder: selection.pendingOrder, reason: "target-not-pending" };
  }
  if (beforeId !== null && selectedIds.has(beforeId)) {
    return { kind: "noop", pendingOrder: selection.pendingOrder, reason: "target-selected" };
  }
  const moved = new Set(selection.movedIds);
  const remainder = selection.pendingOrder.filter((id) => !moved.has(id));
  const insertionIndex = beforeId === null ? remainder.length : remainder.indexOf(beforeId);
  return moveBlockToIndex(rows, selection, insertionIndex, mode);
}

export function planPendingFileMove(
  rows: readonly QueuePlannerRow[],
  itemId: QueueItemId,
  beforeId: QueueItemId | null,
  mode: QueuePresentationMode,
): QueueReorderPlan {
  return planPendingBlockMove(rows, new Set([itemId]), beforeId, mode);
}

/** Plan click/tap Up, Down, Top, and Bottom alternatives for one stable selected block. */
export function planSelectedMove(
  rows: readonly QueuePlannerRow[],
  selectedIds: ReadonlySet<QueueItemId>,
  destination: SelectedMoveDestination,
  mode: QueuePresentationMode,
): QueueReorderPlan {
  const selection = validateSelection(rows, selectedIds);
  if ("kind" in selection) return selection;
  const firstIndex = selection.pendingOrder.findIndex((id) => selectedIds.has(id));
  const remainderLength = selection.pendingOrder.length - selection.movedIds.length;
  let insertionIndex: number;
  if (destination === "top") insertionIndex = 0;
  else if (destination === "bottom") insertionIndex = remainderLength;
  else if (destination === "up") insertionIndex = firstIndex - 1;
  else insertionIndex = firstIndex + 1;

  if (
    (destination === "up" && firstIndex === 0) ||
    (destination === "down" && firstIndex >= remainderLength)
  ) {
    return { kind: "noop", pendingOrder: selection.pendingOrder, reason: "boundary" };
  }
  return moveBlockToIndex(rows, selection, insertionIndex, mode);
}

/** Move the pending members of one contiguous presentation run as a stable block. */
export function planFolderRunMove(
  rows: readonly QueuePlannerRow[],
  runId: FolderRunId,
  beforeRunId: FolderRunId | null,
  mode: QueuePresentationMode,
): QueueReorderPlan {
  const pendingOrder = pendingQueueIds(rows);
  const runs = deriveFolderRuns(rows);
  const source = runs.find((run) => run.id === runId);
  if (source === undefined) return { kind: "noop", pendingOrder, reason: "unknown-run" };
  if (source.pendingIds.length === 0) {
    return { kind: "noop", pendingOrder, reason: "frozen-run" };
  }
  if (beforeRunId === runId) {
    return { kind: "noop", pendingOrder, reason: "identity" };
  }
  const target = beforeRunId === null ? null : runs.find((run) => run.id === beforeRunId);
  if (beforeRunId !== null && target === undefined) {
    return { kind: "noop", pendingOrder, reason: "unknown-run" };
  }
  const beforeId = target?.pendingIds[0] ?? null;
  if (target !== null && target !== undefined && beforeId === null) {
    return { kind: "noop", pendingOrder, reason: "target-not-pending" };
  }
  return planPendingBlockMove(rows, new Set(source.pendingIds), beforeId, mode);
}

/** Folder order follows first pending appearance; within-folder order is unchanged. */
export function planRegroupPending(rows: readonly QueuePlannerRow[]): QueueReorderPlan {
  const pendingOrder = pendingQueueIds(rows);
  const groups = new Map<string, QueueItemId[]>();
  for (const row of rows) {
    if (row.item.state !== "Queued") continue;
    const parent = extractParentPath(row.item.input).key;
    const group = groups.get(parent);
    if (group === undefined) groups.set(parent, [row.item.id]);
    else group.push(row.item.id);
  }
  const regrouped = [...groups.values()].flat();
  if (sameOrder(pendingOrder, regrouped)) {
    return { kind: "noop", pendingOrder, reason: "identity" };
  }
  return { kind: "legal", pendingOrder: regrouped, movedIds: pendingOrder };
}
