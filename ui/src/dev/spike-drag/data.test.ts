import { describe, expect, it } from "vitest";

import { generateRows, moveByStep, moveRow, type SpikeRow } from "./data";

function ids(rows: SpikeRow[]): string[] {
  return rows.map((r) => r.id);
}

const SMALL: SpikeRow[] = [
  { id: "A", kind: "folder", label: "A", parentId: null },
  { id: "a1", kind: "file", label: "a1", parentId: "A" },
  { id: "a2", kind: "file", label: "a2", parentId: "A" },
  { id: "B", kind: "folder", label: "B", parentId: null },
  { id: "b1", kind: "file", label: "b1", parentId: "B" },
];

describe("generateRows", () => {
  it("produces ~500 two-level rows", () => {
    const rows = generateRows();
    expect(rows.length).toBeGreaterThanOrEqual(450);
    expect(rows.filter((r) => r.kind === "folder")).toHaveLength(30);
    expect(rows[0].kind).toBe("folder");
  });
});

describe("moveRow: files", () => {
  it("reorders within a folder", () => {
    const next = moveRow(SMALL, "a2", "a1", "top");
    expect(next && ids(next)).toEqual(["A", "a2", "a1", "B", "b1"]);
  });

  it("moves across folders and reparents", () => {
    const next = moveRow(SMALL, "a1", "b1", "bottom");
    expect(next && ids(next)).toEqual(["A", "a2", "B", "b1", "a1"]);
    expect(next?.find((r) => r.id === "a1")?.parentId).toBe("B");
  });

  it("drops onto a folder's bottom edge as its first file", () => {
    const next = moveRow(SMALL, "a1", "B", "bottom");
    expect(next && ids(next)).toEqual(["A", "a2", "B", "a1", "b1"]);
    expect(next?.find((r) => r.id === "a1")?.parentId).toBe("B");
  });

  it("rejects a file landing above the first folder", () => {
    expect(moveRow(SMALL, "b1", "A", "top")).toBeNull();
  });
});

describe("moveRow: folders", () => {
  it("moves a folder block above another folder", () => {
    const next = moveRow(SMALL, "B", "A", "top");
    expect(next && ids(next)).toEqual(["B", "b1", "A", "a1", "a2"]);
  });

  it("moves a folder block after another folder's block", () => {
    const next = moveRow(SMALL, "A", "b1", "bottom");
    expect(next && ids(next)).toEqual(["B", "b1", "A", "a1", "a2"]);
  });

  it("rejects dropping a folder into itself", () => {
    expect(moveRow(SMALL, "A", "a2", "top")).toBeNull();
  });
});

describe("moveByStep", () => {
  it("steps a file down within its folder", () => {
    const next = moveByStep(SMALL, "a1", 1);
    expect(next && ids(next)).toEqual(["A", "a2", "a1", "B", "b1"]);
  });

  it("steps a file down into the next folder", () => {
    const next = moveByStep(SMALL, "a2", 1);
    expect(next && ids(next)).toEqual(["A", "a1", "B", "a2", "b1"]);
    expect(next?.find((r) => r.id === "a2")?.parentId).toBe("B");
  });

  it("steps a folder block up over the previous folder", () => {
    const next = moveByStep(SMALL, "B", -1);
    expect(next && ids(next)).toEqual(["B", "b1", "A", "a1", "a2"]);
  });

  it("returns null at the boundaries", () => {
    expect(moveByStep(SMALL, "a1", -1)).toBeNull(); // above first folder
    expect(moveByStep(SMALL, "b1", 1)).toBeNull(); // last row
    expect(moveByStep(SMALL, "A", -1)).toBeNull(); // first folder up
    expect(moveByStep(SMALL, "B", 1)).toBeNull(); // last folder down
  });
});
