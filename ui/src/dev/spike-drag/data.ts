/**
 * Queue-drag spike (#36 D6): shared mock data and reorder logic so both
 * candidates differ only in their drag layer. The row shape is deliberately
 * NOT QueueItem-shaped — no hand-authored domain types ahead of bindings.
 */

export type Edge = "top" | "bottom";

export interface SpikeRow {
  id: string;
  kind: "folder" | "file";
  label: string;
  /** Folder id for files; null for folders. */
  parentId: string | null;
}

/** ~500 rows: 30 folders with 14–17 files each, deterministic. */
export function generateRows(): SpikeRow[] {
  const rows: SpikeRow[] = [];
  for (let f = 0; f < 30; f++) {
    const folderId = `folder-${f}`;
    rows.push({ id: folderId, kind: "folder", label: `Season ${f + 1}`, parentId: null });
    const fileCount = 14 + (f % 4);
    for (let i = 0; i < fileCount; i++) {
      rows.push({
        id: `file-${f}-${i}`,
        kind: "file",
        label: `episode-${String(f + 1).padStart(2, "0")}x${String(i + 1).padStart(2, "0")}.mkv`,
        parentId: folderId,
      });
    }
  }
  return rows;
}

function indexOfId(rows: SpikeRow[], id: string): number {
  return rows.findIndex((r) => r.id === id);
}

/** End of a folder's block (exclusive): the index after its last file. */
function blockEnd(rows: SpikeRow[], folderIndex: number): number {
  let end = folderIndex + 1;
  while (end < rows.length && rows[end].kind === "file") end++;
  return end;
}

/**
 * Move a row relative to a target row edge. Returns the new array, or null
 * when the drop is invalid (two-level constraints):
 * - files stay inside some folder (never above the first folder header);
 * - folders move as whole blocks, only to folder boundaries;
 * - a folder cannot drop into itself.
 */
export function moveRow(
  rows: SpikeRow[],
  sourceId: string,
  targetId: string,
  edge: Edge,
): SpikeRow[] | null {
  if (sourceId === targetId) return null;
  const sourceIndex = indexOfId(rows, sourceId);
  const targetIndex = indexOfId(rows, targetId);
  if (sourceIndex < 0 || targetIndex < 0) return null;
  const source = rows[sourceIndex];
  const target = rows[targetIndex];

  if (source.kind === "file") {
    const without = rows.filter((r) => r.id !== sourceId);
    const targetIdx = indexOfId(without, targetId);
    let insertAt: number;
    if (target.kind === "folder") {
      // Bottom edge of a folder header drops in as its first file; the top
      // edge means "end of the previous folder's block".
      if (edge === "bottom") {
        insertAt = targetIdx + 1;
      } else if (targetIdx === 0) {
        return null;
      } else {
        insertAt = targetIdx;
      }
    } else {
      insertAt = edge === "top" ? targetIdx : targetIdx + 1;
    }
    // Reparent to the nearest folder header above the insertion point.
    let parentId: string | null = null;
    for (let i = insertAt - 1; i >= 0; i--) {
      if (without[i].kind === "folder") {
        parentId = without[i].id;
        break;
      }
    }
    if (parentId === null) return null;
    const moved = { ...source, parentId };
    return [...without.slice(0, insertAt), moved, ...without.slice(insertAt)];
  }

  // Folder: move the whole block relative to the target's containing block.
  const sourceEnd = blockEnd(rows, sourceIndex);
  if (targetIndex >= sourceIndex && targetIndex < sourceEnd) return null;
  const block = rows.slice(sourceIndex, sourceEnd);
  const without = [...rows.slice(0, sourceIndex), ...rows.slice(sourceEnd)];

  const containerFolderId = target.kind === "folder" ? target.id : target.parentId;
  if (containerFolderId === null || containerFolderId === sourceId) return null;
  const containerIndex = indexOfId(without, containerFolderId);
  const containerEnd = blockEnd(without, containerIndex);
  const insertAt = edge === "top" ? containerIndex : containerEnd;
  return [...without.slice(0, insertAt), ...block, ...without.slice(insertAt)];
}

/**
 * One-step keyboard move (the pointer-free path): files step over the
 * adjacent row, folders step over the adjacent folder block. Returns null
 * when already at the boundary or the step is invalid.
 */
export function moveByStep(rows: SpikeRow[], id: string, dir: -1 | 1): SpikeRow[] | null {
  const index = indexOfId(rows, id);
  if (index < 0) return null;
  const row = rows[index];

  if (row.kind === "file") {
    const target = rows[index + dir];
    if (!target) return null;
    return moveRow(rows, id, target.id, dir === -1 ? "top" : "bottom");
  }

  if (dir === -1) {
    for (let i = index - 1; i >= 0; i--) {
      if (rows[i].kind === "folder") return moveRow(rows, id, rows[i].id, "top");
    }
    return null;
  }
  for (let i = blockEnd(rows, index); i < rows.length; i++) {
    if (rows[i].kind === "folder") return moveRow(rows, id, rows[i].id, "bottom");
  }
  return null;
}
