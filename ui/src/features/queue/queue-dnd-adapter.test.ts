import { describe, expect, it } from "vitest";

import type { QueueItemId } from "@/lib/bindings";

import { selectedBlockBeforeId } from "./queue-dnd-adapter";

const id = (value: number) => value as QueueItemId;

describe("selectedBlockBeforeId", () => {
  const pending = [1, 2, 3, 4, 5].map(id);

  it("translates an optimistic drag of a non-first selected member", () => {
    expect(selectedBlockBeforeId(pending, id(4), 3, 1, new Set([id(2), id(4)]))).toBe(id(3));
  });

  it("translates a noncontiguous selected block dragged toward the end", () => {
    expect(selectedBlockBeforeId(pending, id(2), 1, 4, new Set([id(2), id(4)]))).toBeNull();
  });

  it("uses the one-item destination for an unselected row", () => {
    expect(selectedBlockBeforeId(pending, id(2), 1, 3, new Set([id(2)]))).toBe(id(5));
  });

  it("rejects stale initial-index data", () => {
    expect(selectedBlockBeforeId(pending, id(2), 2, 3, new Set([id(2)]))).toBeUndefined();
  });
});
