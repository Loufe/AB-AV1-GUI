import { describe, expect, it } from "vitest";

import type { QueueItem, QueueItemId } from "@/lib/bindings";

import type { QueueRowData } from "./queue-status";
import { dropToBeforeId, moveRowBefore } from "./reorder";

function row(id: number): QueueRowData {
  const item: QueueItem = {
    id: id as QueueItemId,
    input: `/videos/${id}.mkv`,
    operation: "Convert",
    output_target: "Replace",
    state: "Queued",
  };
  return {
    item,
    streams: null,
    sizeBytes: null,
    timeSec: null,
    timeConfidence: "exact",
    preciseCrf: false,
    status: { kind: "queued" },
  };
}

const ids = (rows: QueueRowData[]) => rows.map((r) => r.item.id);
const ROWS = [row(1), row(2), row(3), row(4)];

describe("moveRowBefore", () => {
  it("moves before a target", () => {
    expect(ids(moveRowBefore(ROWS, 4 as QueueItemId, 2 as QueueItemId))).toEqual([1, 4, 2, 3]);
  });
  it("moves to the end with a null target", () => {
    expect(ids(moveRowBefore(ROWS, 1 as QueueItemId, null))).toEqual([2, 3, 4, 1]);
  });
  it("appends when the target is missing (engine parity)", () => {
    expect(ids(moveRowBefore(ROWS, 1 as QueueItemId, 99 as QueueItemId))).toEqual([2, 3, 4, 1]);
  });
  it("ignores a missing source", () => {
    expect(ids(moveRowBefore(ROWS, 99 as QueueItemId, 2 as QueueItemId))).toEqual([1, 2, 3, 4]);
  });
});

describe("dropToBeforeId", () => {
  it("dropping downward lands after the target", () => {
    expect(dropToBeforeId(ROWS, 1 as QueueItemId, 3 as QueueItemId)).toBe(4);
  });
  it("dropping on the last row moves to the end", () => {
    expect(dropToBeforeId(ROWS, 1 as QueueItemId, 4 as QueueItemId)).toBeNull();
  });
  it("dropping upward lands before the target", () => {
    expect(dropToBeforeId(ROWS, 4 as QueueItemId, 2 as QueueItemId)).toBe(2);
  });
  it("is a no-op on self or unknown rows", () => {
    expect(dropToBeforeId(ROWS, 2 as QueueItemId, 2 as QueueItemId)).toBeUndefined();
    expect(dropToBeforeId(ROWS, 99 as QueueItemId, 2 as QueueItemId)).toBeUndefined();
  });
  it("forwards a drop above a frozen row; the engine clamps it to first pending", () => {
    const frozen = [row(1), row(2), row(3), row(4)];
    frozen[0].item.state = { Finished: "Stopped" };
    frozen[1].item.state = { Claimed: { claim_id: 1, run_id: 1 } };
    expect(dropToBeforeId(frozen, 4 as QueueItemId, 2 as QueueItemId)).toBe(2);
    expect(dropToBeforeId(frozen, 4 as QueueItemId, 1 as QueueItemId)).toBe(1);
  });
});
