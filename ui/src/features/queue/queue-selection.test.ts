import { describe, expect, it } from "vitest";

import type { QueueItemId } from "@/lib/bindings";

import {
  applyQueueSelection,
  emptyQueueSelection,
  pruneQueueSelection,
  type QueueSelectionState,
} from "./queue-selection";

const id = (value: number) => value as QueueItemId;
const ORDER = [id(1), id(2), id(3), id(4), id(5)];
const selected = (state: QueueSelectionState) => [...state.selectedIds];

describe("applyQueueSelection", () => {
  it("replaces selection and establishes an anchor", () => {
    const state = applyQueueSelection(emptyQueueSelection(), id(3), ORDER, "replace");
    expect(selected(state)).toEqual([id(3)]);
    expect(state.anchorId).toBe(id(3));
  });

  it("Ctrl/Cmd toggles IDs and updates the stable anchor", () => {
    const one = applyQueueSelection(emptyQueueSelection(), id(2), ORDER, "toggle");
    const two = applyQueueSelection(one, id(4), ORDER, "toggle");
    expect(selected(two)).toEqual([id(2), id(4)]);
    expect(two.anchorId).toBe(id(4));

    const removed = applyQueueSelection(two, id(4), ORDER, "toggle");
    expect(selected(removed)).toEqual([id(2)]);
    expect(removed.anchorId).toBe(id(4));
    const empty = applyQueueSelection(removed, id(2), ORDER, "toggle");
    expect(selected(empty)).toEqual([]);
    expect(empty.anchorId).toBeNull();
  });

  it("Shift replaces the range while retaining its original anchor", () => {
    const anchored = applyQueueSelection(emptyQueueSelection(), id(2), ORDER, "replace");
    const forward = applyQueueSelection(anchored, id(5), ORDER, "range");
    expect(selected(forward)).toEqual([id(2), id(3), id(4), id(5)]);
    expect(forward.anchorId).toBe(id(2));

    const contracted = applyQueueSelection(forward, id(3), ORDER, "range");
    expect(selected(contracted)).toEqual([id(2), id(3)]);
    expect(contracted.anchorId).toBe(id(2));
  });

  it("falls back to a new anchor if range selection has no visible anchor", () => {
    const stale: QueueSelectionState = { selectedIds: new Set([id(9)]), anchorId: id(9) };
    const next = applyQueueSelection(stale, id(3), ORDER, "range");
    expect(selected(next)).toEqual([id(3)]);
    expect(next.anchorId).toBe(id(3));
  });

  it("ignores an ID outside the visible authoritative order", () => {
    const initial = emptyQueueSelection();
    const state = applyQueueSelection(initial, id(9), ORDER, "replace");
    expect(state).toBe(initial);
  });
});

describe("pruneQueueSelection", () => {
  it("retains surviving IDs and a surviving anchor", () => {
    const state: QueueSelectionState = {
      selectedIds: new Set([id(1), id(3), id(5)]),
      anchorId: id(3),
    };
    const pruned = pruneQueueSelection(state, [id(1), id(3), id(4)]);
    expect(selected(pruned)).toEqual([id(1), id(3)]);
    expect(pruned.anchorId).toBe(id(3));
  });

  it("chooses the first surviving authoritative ID when the anchor disappeared", () => {
    const state: QueueSelectionState = {
      selectedIds: new Set([id(5), id(2)]),
      anchorId: id(5),
    };
    const pruned = pruneQueueSelection(state, [id(1), id(2), id(3)]);
    expect(selected(pruned)).toEqual([id(2)]);
    expect(pruned.anchorId).toBe(id(2));
  });

  it("clears selection and anchor when no selected ID survives", () => {
    const state: QueueSelectionState = { selectedIds: new Set([id(8)]), anchorId: id(8) };
    expect(pruneQueueSelection(state, ORDER)).toEqual({ selectedIds: new Set(), anchorId: null });
  });

  it("retains object identity when no pruning is needed", () => {
    const state: QueueSelectionState = {
      selectedIds: new Set([id(2), id(4)]),
      anchorId: id(2),
    };
    expect(pruneQueueSelection(state, ORDER)).toBe(state);
  });
});
