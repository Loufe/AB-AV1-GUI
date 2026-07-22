import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ListVideo } from "lucide-react";
import { toast } from "sonner";

import { EmptyState } from "@/components/empty-state";
import type { OutputTarget, QueueItem, QueueItemId, Settings, ToolsState } from "@/lib/bindings";
import {
  forceStop,
  queueAddPaths,
  queueClear,
  queueClearCompleted,
  queueRemove,
  startQueue,
  stopAfterCurrent,
} from "@/lib/ipc";
import { pickPaths } from "@/lib/ipc/path-picker";
import { useAppStore } from "@/lib/store/app-store";
import { useProgressStore } from "@/lib/store/progress-store";

import { CurrentProcessingCard } from "./now-processing-card";
import { queueRows } from "./queue-projection";
import { QueueTable } from "./queue-table";
import { QueueToolbar, type QueuePendingAction } from "./queue-toolbar";
import { SelectionCard } from "./selection-card";

function configuredOutputTarget(settings: Settings): OutputTarget {
  const output = settings.output;
  if (output.default_mode === "replace") return "Replace";
  if (output.default_mode === "suffix") return { Suffix: { suffix: output.suffix } };
  if (output.separate_folder === null) {
    throw new Error("Choose a separate output folder in Settings before adding files.");
  }
  return {
    SeparateFolder: {
      directory: output.separate_folder,
      source_root: null,
    },
  };
}

function isRemovable(item: QueueItem): boolean {
  return item.state === "Queued" || "Finished" in item.state;
}

function isClearable(item: QueueItem): boolean {
  return item.state === "Queued" || "Finished" in item.state;
}

function isClearableCompleted(item: QueueItem): boolean {
  if (item.state === "Queued" || !("Finished" in item.state)) return false;
  const outcome = item.state.Finished;
  if (outcome === undefined) return false;
  return typeof outcome === "string" || outcome.Failed === undefined;
}

function toolBlockReason(tools: ToolsState | null): string | null {
  if (tools === null) return "Checking media tools before the Queue can start.";
  if (tools.availability.Missing !== undefined) return tools.availability.Missing.detail;
  if (tools.activity === "Installing") {
    return "Media tools are being updated. The Queue can start when that finishes.";
  }
  if (typeof tools.activity === "object" && tools.activity.Downloading !== undefined) {
    return "Media tools are being updated. The Queue can start when that finishes.";
  }
  return null;
}

function queueSummary(
  completed: number,
  failed: number,
  skipped: number,
  stopped: number,
): string | null {
  const facts = [];
  if (completed > 0) facts.push(`${completed} completed`);
  if (failed > 0) facts.push(`${failed} failed`);
  if (skipped > 0) facts.push(`${skipped} skipped`);
  if (stopped > 0) facts.push(`${stopped} stopped`);
  return facts.length === 0 ? null : `This session: ${facts.join(" · ")}`;
}

export function QueueView() {
  const durable = useAppStore((state) => state.durable);
  const settings = useAppStore((state) => state.settings);
  const session = useAppStore((state) => state.session);
  const health = useAppStore((state) => state.health);
  const tools = useAppStore((state) => state.tools);
  const aggregates = useProgressStore((state) => state.aggregates);
  const rows = useMemo(() => queueRows(durable), [durable]);
  const [selectedId, setSelectedId] = useState<QueueItemId | null>(null);
  const [pendingAction, setPendingAction] = useState<QueuePendingAction>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    if (selectedId !== null && !rows.some((row) => row.item.id === selectedId)) {
      setSelectedId(null);
    }
  }, [rows, selectedId]);

  const selected = rows.find((row) => row.item.id === selectedId) ?? null;
  const active = rows.find((row) => {
    const state = row.item.state;
    return state !== "Queued" && !("Finished" in state);
  });
  const healthReason = health.unavailable ?? health.fatal ?? health.degraded?.reason ?? null;
  const startBlock = toolBlockReason(tools);
  const queueHasPending = rows.some((row) => row.item.state === "Queued");
  const canStart =
    session === "Idle" &&
    healthReason === null &&
    settings !== null &&
    startBlock === null &&
    queueHasPending;
  const canRemove = selected !== null && isRemovable(selected.item);
  const canClear = session === "Idle" && rows.some((row) => isClearable(row.item));
  const canClearCompleted = rows.some((row) => isClearableCompleted(row.item));
  const summary = queueSummary(
    aggregates.completed,
    aggregates.failed,
    aggregates.skipped,
    aggregates.stopped,
  );

  const runAction = async (action: QueuePendingAction, operation: () => Promise<void>) => {
    setActionError(null);
    setPendingAction(action);
    try {
      await operation();
    } catch (error: unknown) {
      console.error(`Queue action ${action ?? "unknown"} failed`, error);
      const message = error instanceof Error ? error.message : "The Queue action failed.";
      setActionError(message);
      toast.error(message);
    } finally {
      setPendingAction(null);
    }
  };

  const addPaths = (kind: "Files" | "Folders", action: QueuePendingAction) => {
    void runAction(action, async () => {
      if (settings === null) throw new Error("Settings have not loaded yet.");
      const paths = await pickPaths(kind, settings.last_input_folder);
      if (paths.length === 0) return;
      await queueAddPaths(paths, "Convert", "ReuseIfFresh", configuredOutputTarget(settings));
    });
  };

  return (
    <div className="flex min-h-full flex-col gap-4 p-4 sm:p-6">
      <div className="flex items-baseline justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold">Queue</h1>
          <p className="text-sm text-muted-foreground">
            {session === "Idle" ? "Ready" : session.replaceAll(/([A-Z])/g, " $1").trim()}
            {summary !== null && ` · ${summary}`}
          </p>
        </div>
      </div>

      <QueueToolbar
        session={session}
        mutationsDisabled={healthReason !== null || settings === null}
        canRemove={canRemove}
        canClear={canClear}
        canClearCompleted={canClearCompleted}
        canStart={canStart}
        pendingAction={pendingAction}
        onAddFiles={() => addPaths("Files", "add-files")}
        onAddFolder={() => addPaths("Folders", "add-folders")}
        onRemove={() => {
          if (selectedId !== null) {
            void runAction("remove", () => queueRemove(selectedId));
          }
        }}
        onClear={() => void runAction("clear", queueClear)}
        onClearCompleted={() => void runAction("clear-completed", queueClearCompleted)}
        onStart={() => void runAction("start", startQueue)}
        onStopAfterCurrent={() => void runAction("stop-after-current", stopAfterCurrent)}
        onForceStop={() => void runAction("force-stop", forceStop)}
      />

      {healthReason !== null && (
        <p
          role="alert"
          className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          Queue changes are unavailable: {healthReason}
        </p>
      )}
      {healthReason === null && startBlock !== null && (
        <p className="flex items-start gap-2 rounded-md border border-warning/40 bg-warning/5 p-3 text-sm text-warning">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          {startBlock}
        </p>
      )}
      {actionError !== null && (
        <p role="alert" className="text-sm text-destructive">
          {actionError}
        </p>
      )}

      {rows.length === 0 ? (
        <div className="min-h-72 flex-1">
          <EmptyState
            icon={ListVideo}
            title="The queue is empty"
            description="Add files or folders here, or send analyzed files from the Analysis view. The engine keeps their authoritative order and state."
          />
        </div>
      ) : (
        <>
          {active !== undefined && <CurrentProcessingCard row={active} />}
          <QueueTable rows={rows} selectedId={selectedId} onSelect={setSelectedId} />
          {selected !== null && <SelectionCard row={selected} />}
        </>
      )}
    </div>
  );
}
