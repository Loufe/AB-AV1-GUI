import { describe, expect, it } from "vitest";

import type { QueueItem, QueueItemId, QueueItemState } from "@/lib/bindings";

import {
  deriveFolderRuns,
  folderRunId,
  pendingQueueIds,
  planFolderRunMove,
  planPendingBlockMove,
  planPendingFileMove,
  planRegroupPending,
  planSelectedMove,
  type QueueReorderPlan,
} from "./queue-interaction-planner";
import type { QueueRowData } from "./queue-status";

const id = (value: number) => value as QueueItemId;

function row(value: number, input: string, state: QueueItemState = "Queued"): QueueRowData {
  const item: QueueItem = {
    id: id(value),
    input,
    operation: "Convert",
    intent: "ReuseIfFresh",
    output_target: "Replace",
    overwrite: "FollowSettings",
    state,
  };
  return {
    item,
    runId: null,
    streams: null,
    sizeBytes: null,
    mediaDurationMs: null,
    timeMs: null,
    timeConfidence: "exact",
    crf: null,
    vmaf: null,
    status: state === "Queued" ? { kind: "queued" } : { kind: "starting" },
  };
}

const ids = (...values: number[]) => new Set(values.map(id));
const order = (plan: QueueReorderPlan) => plan.pendingOrder;

describe("pending IDs and contiguous folder runs", () => {
  it("extracts only queued IDs in authoritative order", () => {
    const rows = [
      row(1, "/a/1.mkv"),
      row(8, "/a/active.mkv", { Running: { claim_id: 1, run_id: 1 } }),
      row(2, "/b/2.mkv"),
      row(9, "/b/done.mkv", { Finished: "Stopped" }),
    ];
    expect(pendingQueueIds(rows)).toEqual([id(1), id(2)]);
  });

  it("derives repeated same-parent runs with stable first-ID identities", () => {
    const rows = [
      row(1, "/shows/one/a.mkv"),
      row(2, "/shows/one/b.mkv"),
      row(3, "/shows/two/c.mkv"),
      row(4, "/shows/one/d.mkv"),
    ];
    const runs = deriveFolderRuns(rows);
    expect(runs.map((run) => run.parent.key)).toEqual(["/shows/one", "/shows/two", "/shows/one"]);
    expect(runs.map((run) => run.itemIds)).toEqual([[id(1), id(2)], [id(3)], [id(4)]]);
    expect(runs.map((run) => run.id)).toEqual([
      folderRunId("/shows/one", id(1)),
      folderRunId("/shows/two", id(3)),
      folderRunId("/shows/one", id(4)),
    ]);
    expect(runs[0]?.id).not.toBe(runs[2]?.id);
  });

  it("keeps frozen members in presentation runs but out of their pending block", () => {
    const rows = [
      row(1, "/shows/one/a.mkv"),
      row(8, "/shows/one/active.mkv", { Claimed: { claim_id: 1, run_id: 1 } }),
      row(2, "/shows/one/b.mkv"),
    ];
    expect(deriveFolderRuns(rows)[0]).toMatchObject({
      itemIds: [id(1), id(8), id(2)],
      pendingIds: [id(1), id(2)],
    });
  });

  it("does not collapse duplicate folder labels from distinct full parents", () => {
    const runs = deriveFolderRuns([
      row(1, "/library/a/Season 1/one.mkv"),
      row(2, "/library/b/Season 1/two.mkv"),
    ]);
    expect(runs.map((run) => run.parent.label)).toEqual(["Season 1", "Season 1"]);
    expect(runs[0]?.parent.key).not.toBe(runs[1]?.parent.key);
  });
});

describe("stable regrouping", () => {
  it("orders folders by first pending appearance and preserves within-folder order", () => {
    const rows = [
      row(1, "/a/1.mkv"),
      row(2, "/b/2.mkv"),
      row(8, "/frozen/8.mkv", { Finished: "Stopped" }),
      row(3, "/a/3.mkv"),
      row(4, "/c/4.mkv"),
      row(5, "/b/5.mkv"),
    ];
    const plan = planRegroupPending(rows);
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual([id(1), id(3), id(2), id(5), id(4)]);
    expect(order(plan)).not.toContain(id(8));
  });

  it("returns a typed identity no-op when already grouped", () => {
    expect(planRegroupPending([row(1, "/a/1"), row(2, "/a/2"), row(3, "/b/3")])).toEqual({
      kind: "noop",
      pendingOrder: [id(1), id(2), id(3)],
      reason: "identity",
    });
  });
});

describe("file and stable selected-block planning", () => {
  const rows = [
    row(1, "/a/1.mkv"),
    row(2, "/a/2.mkv"),
    row(3, "/b/3.mkv"),
    row(4, "/b/4.mkv"),
    row(5, "/c/5.mkv"),
  ];

  it("moves one file in ungrouped mode", () => {
    const plan = planPendingFileMove(rows, id(2), id(4), "ungrouped");
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual([id(1), id(3), id(2), id(4), id(5)]);
  });

  it("moves a noncontiguous selection as one stable block", () => {
    const plan = planPendingBlockMove(rows, ids(2, 4), id(1), "ungrouped");
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual([id(2), id(4), id(1), id(3), id(5)]);
    if (plan.kind !== "noop") expect(plan.movedIds).toEqual([id(2), id(4)]);
  });

  it("classifies same-parent movement that retains run counts as legal", () => {
    const plan = planPendingBlockMove(rows, ids(2), id(1), "grouped");
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual([id(2), id(1), id(3), id(4), id(5)]);
  });

  it("freezes an exact ID-based cross-folder confirmation plan", () => {
    const selectedIds = ids(2);
    const plan = planPendingBlockMove(rows, selectedIds, id(4), "grouped");
    expect(plan.kind).toBe("cross-folder");
    expect(order(plan)).toEqual([id(1), id(3), id(2), id(4), id(5)]);
    selectedIds.add(id(5));
    expect(order(plan)).toEqual([id(1), id(3), id(2), id(4), id(5)]);
  });

  it("allows the identical attempted move in ungrouped mode", () => {
    expect(planPendingBlockMove(rows, ids(2), id(4), "ungrouped").kind).toBe("legal");
  });

  it("allows a grouped move that heals an already repeated parent run", () => {
    const repeated = [row(1, "/a/1"), row(2, "/b/2"), row(3, "/a/3")];
    const plan = planPendingFileMove(repeated, id(2), null, "grouped");
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual([id(1), id(3), id(2)]);
  });

  it("allows a whole-parent block to move without fragmenting folders", () => {
    const plan = planPendingBlockMove(rows, ids(1, 2), null, "grouped");
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual([id(3), id(4), id(5), id(1), id(2)]);
  });

  it.each([
    [new Set<QueueItemId>(), id(1), "empty-selection"],
    [ids(99), id(1), "unknown-selection"],
    [ids(1), id(99), "target-not-pending"],
    [ids(1, 2), id(2), "target-selected"],
  ] as const)("returns typed no-op %s", (selectedIds, beforeId, reason) => {
    expect(planPendingBlockMove(rows, selectedIds, beforeId, "grouped")).toMatchObject({
      kind: "noop",
      reason,
    });
  });

  it("rejects a mixed frozen selection instead of silently filtering it", () => {
    const frozenRows = [
      row(1, "/a/1"),
      row(8, "/a/8", { Reserved: { claim_id: 1, run_id: 1 } }),
      row(2, "/b/2"),
    ];
    const plan = planPendingBlockMove(frozenRows, ids(1, 8), null, "ungrouped");
    expect(plan).toEqual({
      kind: "noop",
      pendingOrder: [id(1), id(2)],
      reason: "frozen-selection",
    });
  });

  it("rejects a frozen target and keeps it out of the pending permutation", () => {
    const frozenRows = [row(1, "/a/1"), row(8, "/a/8", { Finished: "Stopped" }), row(2, "/b/2")];
    expect(planPendingBlockMove(frozenRows, ids(2), id(8), "ungrouped")).toEqual({
      kind: "noop",
      pendingOrder: [id(1), id(2)],
      reason: "target-not-pending",
    });
  });
});

describe("folder-run movement", () => {
  it("moves only a run's pending IDs as a stable block", () => {
    const rows = [
      row(1, "/a/1"),
      row(8, "/a/frozen", { Running: { claim_id: 1, run_id: 1 } }),
      row(2, "/a/2"),
      row(3, "/b/3"),
      row(4, "/b/4"),
      row(5, "/a/5"),
    ];
    const runs = deriveFolderRuns(rows);
    const plan = planFolderRunMove(rows, runs[0]?.id ?? "", runs[2]?.id ?? null, "grouped");
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual([id(3), id(4), id(1), id(2), id(5)]);
    expect(order(plan)).not.toContain(id(8));
  });

  it("rejects unknown and all-frozen runs", () => {
    const rows = [row(8, "/a/frozen", { Finished: "Stopped" }), row(1, "/b/1")];
    const frozenRun = deriveFolderRuns(rows)[0];
    expect(planFolderRunMove(rows, "missing", null, "grouped")).toMatchObject({
      kind: "noop",
      reason: "unknown-run",
    });
    expect(planFolderRunMove(rows, frozenRun?.id ?? "", null, "grouped")).toMatchObject({
      kind: "noop",
      reason: "frozen-run",
    });
  });
});

describe("Up, Down, Top, and Bottom alternatives", () => {
  const rows = [1, 2, 3, 4, 5].map((value) => row(value, `/folder/${value}`));

  it.each([
    ["up", [2, 3, 1, 4, 5]],
    ["down", [1, 4, 2, 3, 5]],
    ["top", [2, 3, 1, 4, 5]],
    ["bottom", [1, 4, 5, 2, 3]],
  ] as const)("plans Move %s for one stable selected block", (destination, expected) => {
    const plan = planSelectedMove(rows, ids(2, 3), destination, "ungrouped");
    expect(plan.kind).toBe("legal");
    expect(order(plan)).toEqual(expected.map(id));
  });

  it("preserves relative order while consolidating a noncontiguous selection", () => {
    expect(order(planSelectedMove(rows, ids(2, 4), "down", "ungrouped"))).toEqual([
      id(1),
      id(3),
      id(2),
      id(4),
      id(5),
    ]);
  });

  it("returns boundary no-ops", () => {
    expect(planSelectedMove(rows, ids(1, 2), "up", "ungrouped")).toMatchObject({
      kind: "noop",
      reason: "boundary",
    });
    expect(planSelectedMove(rows, ids(4, 5), "down", "ungrouped")).toMatchObject({
      kind: "noop",
      reason: "boundary",
    });
  });

  it("uses the same grouped cross-folder classification as pointer movement", () => {
    const grouped = [row(1, "/a/1"), row(2, "/a/2"), row(3, "/b/3"), row(4, "/b/4")];
    expect(planSelectedMove(grouped, ids(2), "down", "grouped").kind).toBe("cross-folder");
  });
});
