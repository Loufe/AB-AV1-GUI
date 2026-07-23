import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ListVideo } from "lucide-react";
import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { EmptyState } from "@/components/empty-state";
import type {
  Operation,
  OutputTarget,
  QueueItem,
  QueueItemEdit,
  QueueItemId,
  Settings,
  ToolsState,
} from "@/lib/bindings";
import {
  forceStop,
  openPath,
  queueAddPaths,
  queueClear,
  queueClearCompleted,
  queueEdit,
  queueRemoveMany,
  queueReorderPending,
  queueRetry,
  revealInFileManager,
  startQueue,
  stopAfterCurrent,
} from "@/lib/ipc";
import { pickPaths } from "@/lib/ipc/path-picker";
import { useAppStore } from "@/lib/store/app-store";
import { useProgressStore } from "@/lib/store/progress-store";

import {
  pendingQueueIds,
  planRegroupPending,
  planSelectedMove,
  type QueueReorderPlan,
  type SelectedMoveDestination,
} from "./queue-interaction-planner";
import { queueRows } from "./queue-projection";
import { applyQueueSelection, emptyQueueSelection, pruneQueueSelection } from "./queue-selection";
import type { QueueSelectionMode, QueueSelectionState } from "./queue-selection";
import { basename, type QueueRowData } from "./queue-status";
import type { QueueRowAction } from "./queue-row";
import { QueueTable } from "./queue-table";
import type { QueueReorderFocus } from "./queue-table";
import { QueueToolbar, type QueuePendingAction } from "./queue-toolbar";
import { SelectionCard } from "./selection-card";
import { CurrentProcessingCard } from "./now-processing-card";

type ReorderInteraction =
  | { kind: "idle" }
  | {
      kind: "dragging";
      snapshot: readonly QueueRowData[];
      selectedIds: ReadonlySet<QueueItemId>;
      focus: QueueReorderFocus;
      snapshotGeneration: number;
    }
  | {
      kind: "confirming";
      plan: Exclude<QueueReorderPlan, { kind: "noop" }>;
      focus: QueueReorderFocus;
      snapshotGeneration: number;
      baselinePending: readonly QueueItemId[];
    }
  | {
      kind: "submitting";
      plan: Exclude<QueueReorderPlan, { kind: "noop" }>;
      focus: QueueReorderFocus;
      acknowledged: boolean;
      rejection: string | null;
      superseded: string | null;
      baselinePending: readonly QueueItemId[];
      snapshotGeneration: number;
    };

function configuredOutputTarget(settings: Settings): OutputTarget {
  if (settings.output.default_mode === "replace") return "Replace";
  if (settings.output.default_mode === "suffix")
    return { Suffix: { suffix: settings.output.suffix } };
  if (settings.output.separate_folder === null)
    throw new Error("Choose a separate output folder in Settings before adding files.");
  return { SeparateFolder: { directory: settings.output.separate_folder, source_root: null } };
}

function isRemovable(item: QueueItem): boolean {
  return item.state === "Queued" || "Finished" in item.state;
}
function isClearableCompleted(item: QueueItem): boolean {
  if (item.state === "Queued" || !("Finished" in item.state)) return false;
  const outcome = item.state.Finished;
  return typeof outcome === "string" || outcome?.Failed === undefined;
}
function toolBlockReason(tools: ToolsState | null): string | null {
  if (tools === null) return "Checking media tools before the Queue can start.";
  if (tools.availability.Missing !== undefined) return tools.availability.Missing.detail;
  if (
    tools.activity === "Installing" ||
    (typeof tools.activity === "object" && tools.activity.Downloading !== undefined)
  )
    return "Media tools are being updated. The Queue can start when that finishes.";
  return null;
}
function sameIds(left: readonly QueueItemId[], right: readonly QueueItemId[]): boolean {
  return left.length === right.length && left.every((id, index) => id === right[index]);
}
function plannedRows(rows: readonly QueueRowData[], order: readonly QueueItemId[]): QueueRowData[] {
  const pending = new Map(
    rows.filter((row) => row.item.state === "Queued").map((row) => [row.item.id, row]),
  );
  return [
    ...rows.filter((row) => row.item.state !== "Queued"),
    ...order.flatMap((id) => {
      const row = pending.get(id);
      return row === undefined ? [] : [row];
    }),
  ];
}

function restoreReorderFocus(focus: QueueReorderFocus): void {
  requestAnimationFrame(() => {
    const targets = Array.from(document.querySelectorAll<HTMLElement>("[data-queue-focus]"));
    const target =
      targets.find((element) => element.dataset.queueFocus === focus.key) ??
      targets.find((element) => element.dataset.queueFocus === focus.fallbackKey);
    target?.focus();
  });
}

export function QueueView() {
  const durable = useAppStore((state) => state.durable);
  const settings = useAppStore((state) => state.settings);
  const session = useAppStore((state) => state.session);
  const health = useAppStore((state) => state.health);
  const tools = useAppStore((state) => state.tools);
  const snapshotGeneration = useAppStore((state) => state.snapshotGeneration);
  const aggregates = useProgressStore((state) => state.aggregates);
  const authoritativeRows = useMemo(() => queueRows(durable), [durable]);
  const [selection, setSelection] = useState<QueueSelectionState>(emptyQueueSelection);
  const [grouped, setGrouped] = useState(true);
  const [interaction, setInteraction] = useState<ReorderInteraction>({ kind: "idle" });
  const [pendingAction, setPendingAction] = useState<QueuePendingAction>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("Queue ready");
  const [contextRemovalId, setContextRemovalId] = useState<QueueItemId | null>(null);

  useEffect(() => {
    setSelection((current) =>
      pruneQueueSelection(
        current,
        authoritativeRows.map((row) => row.item.id),
      ),
    );
  }, [authoritativeRows]);
  useEffect(() => {
    if (interaction.kind !== "confirming") return;
    if (interaction.snapshotGeneration === snapshotGeneration) return;
    const focus = interaction.focus;
    setInteraction({ kind: "idle" });
    setAnnouncement(`Queue snapshot changed; cancelled moving ${focus.label}`);
    restoreReorderFocus(focus);
  }, [interaction, snapshotGeneration]);
  useEffect(() => {
    if (interaction.kind !== "confirming") return;
    const authoritativePending = pendingQueueIds(authoritativeRows);
    if (sameIds(authoritativePending, interaction.baselinePending)) return;
    const message = `Queue changed before moving ${interaction.focus.label}; the latest order was restored.`;
    const focus = interaction.focus;
    setInteraction({ kind: "idle" });
    setActionError(message);
    toast.error(message);
    setAnnouncement(message);
    restoreReorderFocus(focus);
  }, [authoritativeRows, interaction]);
  useEffect(() => {
    if (interaction.kind !== "submitting") return;
    if (interaction.rejection !== null) {
      const focus = interaction.focus;
      if (interaction.superseded === null) {
        setActionError(interaction.rejection);
        toast.error(interaction.rejection);
        setAnnouncement(`Queue error: ${interaction.rejection}`);
      }
      setInteraction({ kind: "idle" });
      restoreReorderFocus(focus);
      return;
    }
    if (interaction.superseded !== null) {
      if (!interaction.acknowledged) return;
      const focus = interaction.focus;
      setInteraction({ kind: "idle" });
      restoreReorderFocus(focus);
      return;
    }
    const authoritativePending = pendingQueueIds(authoritativeRows);
    const expectedMembership = [...interaction.plan.pendingOrder].sort((a, b) => a - b);
    const actualMembership = [...authoritativePending].sort((a, b) => a - b);
    if (!sameIds(expectedMembership, actualMembership)) {
      const message = "Queue changed before the move completed; the latest order was restored.";
      setInteraction({ ...interaction, superseded: message });
      setActionError(message);
      toast.error(message);
      setAnnouncement(`Queue error: ${message}`);
      return;
    }
    if (
      interaction.snapshotGeneration !== snapshotGeneration &&
      !sameIds(authoritativePending, interaction.plan.pendingOrder)
    ) {
      const message = `Queue snapshot changed while moving ${interaction.focus.label}; the latest order was restored.`;
      setInteraction({ ...interaction, superseded: message });
      setActionError(message);
      toast.error(message);
      setAnnouncement(message);
      return;
    }
    if (!interaction.acknowledged) return;
    if (!sameIds(authoritativePending, interaction.plan.pendingOrder)) {
      if (sameIds(authoritativePending, interaction.baselinePending)) return;
      const message = `Queue changed while moving ${interaction.focus.label}; the latest order was restored.`;
      setInteraction({ ...interaction, superseded: message });
      setActionError(message);
      toast.error(message);
      setAnnouncement(message);
      return;
    }
    const focus = interaction.focus;
    const firstMoved = interaction.plan.movedIds[0];
    const position =
      firstMoved === undefined ? -1 : authoritativePending.findIndex((id) => id === firstMoved);
    setInteraction({ kind: "idle" });
    setAnnouncement(
      position < 0
        ? `Queue order updated for ${focus.label}`
        : `Moved ${focus.label} to position ${position + 1} of ${authoritativePending.length}`,
    );
    restoreReorderFocus(focus);
  }, [authoritativeRows, interaction, snapshotGeneration]);

  const authoritativePending = pendingQueueIds(authoritativeRows);
  const submittingMembershipMatches =
    interaction.kind === "submitting" &&
    sameIds(
      [...authoritativePending].sort((a, b) => a - b),
      [...interaction.plan.pendingOrder].sort((a, b) => a - b),
    );
  const renderedRows =
    interaction.kind === "dragging"
      ? [...interaction.snapshot]
      : interaction.kind === "submitting" &&
          interaction.superseded === null &&
          interaction.snapshotGeneration === snapshotGeneration &&
          submittingMembershipMatches
        ? plannedRows(authoritativeRows, interaction.plan.pendingOrder)
        : authoritativeRows;
  const selectedRows = authoritativeRows.filter((row) => selection.selectedIds.has(row.item.id));
  const selected = selectedRows.length === 1 ? (selectedRows[0] ?? null) : null;
  const active = authoritativeRows.find(
    (row) => row.item.state !== "Queued" && !("Finished" in row.item.state),
  );
  const healthReason = health.unavailable ?? health.fatal ?? health.degraded?.reason ?? null;
  const startBlock = toolBlockReason(tools);
  const mutationPending = pendingAction !== null || interaction.kind !== "idle";
  const selectedAllRemovable =
    selectedRows.length > 0 && selectedRows.every((row) => isRemovable(row.item));
  const selectedAllPending =
    selectedRows.length > 0 && selectedRows.every((row) => row.item.state === "Queued");

  const runAction = async (
    action: Exclude<QueuePendingAction, null>,
    operation: () => Promise<void>,
  ): Promise<boolean> => {
    setActionError(null);
    setPendingAction(action);
    try {
      await operation();
      return true;
    } catch (error: unknown) {
      console.error(`Queue action ${action} failed`, error);
      const message = error instanceof Error ? error.message : "The Queue action failed.";
      setActionError(message);
      toast.error(message);
      setAnnouncement(`Queue error: ${message}`);
      return false;
    } finally {
      setPendingAction(null);
    }
  };

  const submitPlan = (
    plan: Exclude<QueueReorderPlan, { kind: "noop" }>,
    focus: QueueReorderFocus,
    planSnapshotGeneration: number,
  ) => {
    setInteraction({
      kind: "submitting",
      plan,
      focus,
      acknowledged: false,
      rejection: null,
      superseded: null,
      baselinePending: pendingQueueIds(authoritativeRows),
      snapshotGeneration: planSnapshotGeneration,
    });
    setAnnouncement(`Submitting Queue order for ${focus.label}`);
    void queueReorderPending([...plan.pendingOrder])
      .then(() => {
        setInteraction((current) =>
          current.kind === "submitting" && current.plan === plan
            ? { ...current, acknowledged: true }
            : current,
        );
      })
      .catch((error: unknown) => {
        console.error("Queue reorder failed", error);
        const message =
          error instanceof Error ? error.message : "Queue order changed; try the move again.";
        setInteraction((current) =>
          current.kind === "submitting" && current.plan === plan
            ? { ...current, rejection: message }
            : current,
        );
      });
  };
  const handlePlan = (plan: QueueReorderPlan, focus: QueueReorderFocus) => {
    const planSnapshotGeneration =
      interaction.kind === "dragging" ? interaction.snapshotGeneration : snapshotGeneration;
    const baselinePending =
      interaction.kind === "dragging"
        ? pendingQueueIds(interaction.snapshot)
        : pendingQueueIds(authoritativeRows);
    if (
      interaction.kind === "dragging" &&
      !sameIds(pendingQueueIds(authoritativeRows), baselinePending)
    ) {
      const message = `Queue changed while moving ${focus.label}; the latest order was restored.`;
      setInteraction({ kind: "idle" });
      setActionError(message);
      toast.error(message);
      setAnnouncement(message);
      restoreReorderFocus(focus);
      return;
    }
    if (plan.kind === "noop") {
      setInteraction({ kind: "idle" });
      setAnnouncement(`Queue order unchanged for ${focus.label}`);
      restoreReorderFocus(focus);
      return;
    }
    if (plan.kind === "cross-folder") {
      setInteraction({
        kind: "confirming",
        plan,
        focus,
        snapshotGeneration: planSnapshotGeneration,
        baselinePending,
      });
      setAnnouncement(`Confirm ungrouping to move ${focus.label}`);
      return;
    }
    submitPlan(plan, focus, planSnapshotGeneration);
  };

  const addPaths = (kind: "Files" | "Folders", action: "add-files" | "add-folders") =>
    void runAction(action, async () => {
      if (settings === null) throw new Error("Settings have not loaded yet.");
      const paths = await pickPaths(kind, settings.last_input_folder);
      if (paths.length > 0)
        await queueAddPaths(paths, "Convert", "ReuseIfFresh", configuredOutputTarget(settings));
    });
  const editSelected = async (patch: QueueItemEdit): Promise<boolean> => {
    if (selected === null) return false;
    return runAction("edit", () => queueEdit(selected.item.id, patch));
  };
  const recoverSelected = (operation: Operation) => {
    if (selected === null) return;
    const patch: QueueItemEdit = {
      operation,
      intent: "Refresh",
      output_target: null,
      overwrite: null,
    };
    void runAction("retry", () => queueRetry(selected.item.id, patch));
  };
  const handleRowAction = (id: QueueItemId, action: QueueRowAction) => {
    const row = authoritativeRows.find((candidate) => candidate.item.id === id);
    if (row === undefined) return;
    if (action === "edit") {
      setSelection({ selectedIds: new Set([id]), anchorId: id });
      requestAnimationFrame(() =>
        document.querySelector<HTMLElement>('[aria-label="Operation"]')?.focus(),
      );
    } else if (action === "open") {
      void runAction("open", () => openPath(row.item.input));
    } else if (action === "reveal") {
      void runAction("reveal", () => revealInFileManager(row.item.input));
    } else if (action === "retry") {
      void runAction("retry", () => queueRetry(id, null));
    } else if (action === "convert-anyway" || action === "reanalyze") {
      const patch: QueueItemEdit = {
        operation: action === "convert-anyway" ? "Convert" : "Analyze",
        intent: "Refresh",
        output_target: null,
        overwrite: null,
      };
      void runAction("retry", () => queueRetry(id, patch));
    } else {
      setContextRemovalId(id);
    }
  };

  return (
    <div className="flex min-h-full flex-col gap-4 p-4 sm:p-6">
      <div>
        <h1 className="text-xl font-semibold">Queue</h1>
        <p className="text-sm text-muted-foreground">
          {session === "Idle" ? "Ready" : session.replaceAll(/([A-Z])/g, " $1").trim()} ·{" "}
          {aggregates.completed} completed
        </p>
      </div>
      <QueueToolbar
        session={session}
        mutationsDisabled={
          healthReason !== null || settings === null || interaction.kind !== "idle"
        }
        canRemove={selectedAllRemovable}
        selectedCount={selectedRows.length}
        grouped={grouped}
        canReorder={selectedAllPending && interaction.kind === "idle"}
        canClear={session === "Idle" && authoritativeRows.some((row) => isRemovable(row.item))}
        canClearCompleted={authoritativeRows.some((row) => isClearableCompleted(row.item))}
        canStart={
          session === "Idle" &&
          healthReason === null &&
          settings !== null &&
          startBlock === null &&
          authoritativeRows.some((row) => row.item.state === "Queued")
        }
        pendingAction={pendingAction}
        onAddFiles={() => addPaths("Files", "add-files")}
        onAddFolder={() => addPaths("Folders", "add-folders")}
        onRemove={() =>
          void runAction("remove", () => queueRemoveMany(selectedRows.map((row) => row.item.id)))
        }
        onGroupedChange={setGrouped}
        onRegroup={() =>
          handlePlan(planRegroupPending(authoritativeRows), {
            key: "toolbar:regroup",
            label: "pending items",
          })
        }
        onMoveSelected={(destination: SelectedMoveDestination) =>
          handlePlan(
            planSelectedMove(
              authoritativeRows,
              selection.selectedIds,
              destination,
              grouped ? "grouped" : "ungrouped",
            ),
            {
              key: `toolbar:move:${destination}`,
              label:
                selectedRows.length === 1
                  ? basename(selectedRows[0]?.item.input ?? "selected item")
                  : `${selectedRows.length} selected items`,
            },
          )
        }
        onClear={() => void runAction("clear", queueClear)}
        onClearCompleted={() => void runAction("clear-completed", queueClearCompleted)}
        onStart={() => void runAction("start", startQueue)}
        onStopAfterCurrent={() => void runAction("stop-after-current", stopAfterCurrent)}
        onForceStop={() => void runAction("force-stop", forceStop)}
      />
      {healthReason !== null && (
        <p
          role="alert"
          className="flex gap-2 rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          <AlertTriangle className="size-4" />
          Queue changes are unavailable: {healthReason}
        </p>
      )}
      {healthReason === null && startBlock !== null && (
        <p className="flex gap-2 rounded-md border border-warning/40 p-3 text-sm text-warning">
          <AlertTriangle className="size-4" aria-hidden="true" />
          {startBlock}
        </p>
      )}
      {actionError !== null && (
        <p role="alert" className="text-sm text-destructive">
          {actionError}
        </p>
      )}
      <p className="sr-only" role="status" aria-live="polite">
        {announcement}
      </p>
      {authoritativeRows.length === 0 ? (
        <div className="min-h-72 flex-1">
          <EmptyState
            icon={ListVideo}
            title="The queue is empty"
            description="Add files or folders to begin."
          />
        </div>
      ) : (
        <>
          {active !== undefined && <CurrentProcessingCard row={active} />}
          <QueueTable
            rows={renderedRows}
            grouped={grouped}
            selectedIds={
              interaction.kind === "dragging" ? interaction.selectedIds : selection.selectedIds
            }
            onSelect={(id, mode: QueueSelectionMode) => {
              if (interaction.kind !== "idle") return;
              setSelection((current) =>
                applyQueueSelection(
                  current,
                  id,
                  authoritativeRows.map((row) => row.item.id),
                  mode,
                ),
              );
            }}
            onPlan={handlePlan}
            onDragStart={(focus) =>
              setInteraction({
                kind: "dragging",
                snapshot: authoritativeRows,
                selectedIds: new Set(selection.selectedIds),
                focus,
                snapshotGeneration,
              })
            }
            onDragCancel={() => {
              const snapshotChanged =
                interaction.kind === "dragging" &&
                interaction.snapshotGeneration !== snapshotGeneration;
              if (interaction.kind === "dragging") restoreReorderFocus(interaction.focus);
              setInteraction({ kind: "idle" });
              setAnnouncement(
                snapshotChanged
                  ? `Queue snapshot changed; cancelled moving ${interaction.focus.label}`
                  : interaction.kind === "dragging"
                    ? `Cancelled moving ${interaction.focus.label}`
                    : "Move cancelled",
              );
            }}
            onDragNoop={() => {
              if (interaction.kind === "dragging") {
                restoreReorderFocus(interaction.focus);
                setAnnouncement(`Queue order unchanged for ${interaction.focus.label}`);
              } else {
                setAnnouncement("Queue order unchanged");
              }
              setInteraction({ kind: "idle" });
            }}
            cancelActiveDrag={
              interaction.kind === "dragging" &&
              interaction.snapshotGeneration !== snapshotGeneration
            }
            reorderEnabled={
              healthReason === null &&
              settings !== null &&
              pendingAction === null &&
              (interaction.kind === "idle" || interaction.kind === "dragging")
            }
            actionsDisabled={mutationPending}
            editingAllowed={session === "Idle" && healthReason === null && settings !== null}
            recoveryAllowed={session === "Idle" && healthReason === null && settings !== null}
            durableActionsAllowed={healthReason === null && settings !== null}
            onRowAction={handleRowAction}
          />
          {selectedRows.length > 1 && (
            <p className="text-sm text-muted-foreground">{selectedRows.length} items selected</p>
          )}
          {selected !== null && (
            <SelectionCard
              row={selected}
              editable={
                session === "Idle" &&
                healthReason === null &&
                settings !== null &&
                selected.item.state === "Queued"
              }
              busy={mutationPending}
              canRecover={session === "Idle" && healthReason === null && settings !== null}
              canRetry={healthReason === null && settings !== null}
              suffixDefault={settings?.output.suffix ?? null}
              separateFolderDefault={settings?.output.separate_folder ?? null}
              onEdit={editSelected}
              onRetry={() => void runAction("retry", () => queueRetry(selected.item.id, null))}
              onRecover={recoverSelected}
              onOpen={() => void runAction("open", () => openPath(selected.item.input))}
              onReveal={() =>
                void runAction("reveal", () => revealInFileManager(selected.item.input))
              }
            />
          )}
        </>
      )}
      <AlertDialog
        open={interaction.kind === "confirming"}
        onOpenChange={(open) => {
          if (!open && interaction.kind === "confirming") {
            const focus = interaction.focus;
            setInteraction({ kind: "idle" });
            setAnnouncement(`Cancelled moving ${focus.label}`);
            restoreReorderFocus(focus);
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Ungroup and move?</AlertDialogTitle>
            <AlertDialogDescription>
              This move would split a folder run. Ungroup the Queue and submit exactly the planned
              order?
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (interaction.kind === "confirming") {
                  setGrouped(false);
                  submitPlan(interaction.plan, interaction.focus, interaction.snapshotGeneration);
                }
              }}
            >
              Ungroup and move
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog
        open={contextRemovalId !== null}
        onOpenChange={(open) => {
          if (!open) {
            const focusId = contextRemovalId;
            setContextRemovalId(null);
            if (focusId !== null)
              requestAnimationFrame(() =>
                document.querySelector<HTMLElement>(`[data-queue-actions="${focusId}"]`)?.focus(),
              );
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove this Queue item?</AlertDialogTitle>
            <AlertDialogDescription>
              The file stays on disk. This Queue entry will be removed atomically.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (contextRemovalId !== null) {
                  const itemId = contextRemovalId;
                  setContextRemovalId(null);
                  void runAction("remove", () => queueRemoveMany([itemId]));
                }
              }}
            >
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
