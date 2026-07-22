import type { QueueItemId } from "@/lib/bindings";

import { deriveFolderRuns, type QueuePlannerRow } from "./queue-interaction-planner";

/** Mirror the fold: frozen rows retain relative order as a prefix, then pending order is appended. */
export function applyPendingOrderToRows<T extends QueuePlannerRow>(
  rows: readonly T[],
  pendingOrder: readonly QueueItemId[],
): T[] {
  const pendingById = new Map(
    rows.filter((row) => row.item.state === "Queued").map((row) => [row.item.id, row]),
  );
  const plannedRows = pendingOrder.map((plannedId) => {
    const planned = plannedId === undefined ? undefined : pendingById.get(plannedId);
    if (planned === undefined) throw new Error("test pending order is not a complete permutation");
    return planned;
  });
  return [...rows.filter((row) => row.item.state !== "Queued"), ...plannedRows];
}

export function fullParentRuns(rows: readonly QueuePlannerRow[]): string[] {
  return deriveFolderRuns(rows).map((run) => run.parent.key);
}
