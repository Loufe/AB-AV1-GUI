import { useState } from "react";
import {
  ArrowDown,
  ArrowDownToLine,
  ArrowUp,
  ArrowUpToLine,
  FolderPlus,
  Play,
  Plus,
} from "lucide-react";

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
import { Button } from "@/components/ui/button";
import type { SessionState } from "@/lib/bindings";

function ConfirmAction({
  label,
  title,
  description,
  disabled,
  destructive = false,
  onConfirm,
}: {
  label: string;
  title: string;
  description: string;
  disabled: boolean;
  destructive?: boolean;
  onConfirm: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <AlertDialog open={open} onOpenChange={setOpen}>
      <Button size="sm" variant="ghost" disabled={disabled} onClick={() => setOpen(true)}>
        {label}
      </Button>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant={destructive ? "destructive" : "default"}
            onClick={() => {
              setOpen(false);
              onConfirm();
            }}
          >
            {label}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export function QueueToolbar({
  session,
  mutationsDisabled,
  canRemove,
  selectedCount,
  grouped,
  canReorder,
  canClear,
  canClearCompleted,
  canStart,
  pendingAction,
  onAddFiles,
  onAddFolder,
  onRemove,
  onGroupedChange,
  onRegroup,
  onMoveSelected,
  onClear,
  onClearCompleted,
  onStart,
  onStopAfterCurrent,
  onForceStop,
}: {
  session: SessionState;
  mutationsDisabled: boolean;
  canRemove: boolean;
  selectedCount: number;
  grouped: boolean;
  canReorder: boolean;
  canClear: boolean;
  canClearCompleted: boolean;
  canStart: boolean;
  pendingAction: QueuePendingAction;
  onAddFiles: () => void;
  onAddFolder: () => void;
  onRemove: () => void;
  onGroupedChange: (grouped: boolean) => void;
  onRegroup: () => void;
  onMoveSelected: (destination: "up" | "down" | "top" | "bottom") => void;
  onClear: () => void;
  onClearCompleted: () => void;
  onStart: () => void;
  onStopAfterCurrent: () => void;
  onForceStop: () => void;
}) {
  const running = session === "Running";
  const stopRequested = session === "StopAfterCurrent";
  const forceStopping = session === "ForceStopping";
  const mutationPending = pendingAction !== null;
  const blocked = mutationsDisabled || mutationPending;
  const stopPending = pendingAction === "stop-after-current";
  const forcePending = pendingAction === "force-stop";
  return (
    <div className="flex flex-wrap items-center justify-between gap-2" aria-label="Queue actions">
      <div className="flex flex-wrap items-center gap-1.5">
        <Button size="sm" variant="outline" disabled={blocked} onClick={onAddFiles}>
          <Plus data-icon="inline-start" aria-hidden="true" />
          Add Files
        </Button>
        <Button size="sm" variant="outline" disabled={blocked} onClick={onAddFolder}>
          <FolderPlus data-icon="inline-start" aria-hidden="true" />
          Add Folders
        </Button>
        <ConfirmAction
          label={selectedCount > 1 ? `Remove ${selectedCount}` : "Remove"}
          title={`Remove ${selectedCount} selected ${selectedCount === 1 ? "item" : "items"}?`}
          description="Files stay on disk. The selected Queue entries are removed atomically."
          disabled={blocked || !canRemove}
          destructive
          onConfirm={onRemove}
        />
        <Button
          size="sm"
          variant={grouped ? "secondary" : "ghost"}
          aria-pressed={grouped}
          disabled={blocked}
          onClick={() => onGroupedChange(!grouped)}
        >
          Group by folder
        </Button>
        <Button
          size="sm"
          variant="ghost"
          data-queue-focus="toolbar:regroup"
          disabled={blocked || !grouped}
          onClick={onRegroup}
        >
          Regroup pending items
        </Button>
        <div className="flex items-center" aria-label="Move selected">
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label="Move selected to top"
            data-queue-focus="toolbar:move:top"
            disabled={blocked || !canReorder}
            onClick={() => onMoveSelected("top")}
          >
            <ArrowUpToLine />
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label="Move selected up"
            data-queue-focus="toolbar:move:up"
            disabled={blocked || !canReorder}
            onClick={() => onMoveSelected("up")}
          >
            <ArrowUp />
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label="Move selected down"
            data-queue-focus="toolbar:move:down"
            disabled={blocked || !canReorder}
            onClick={() => onMoveSelected("down")}
          >
            <ArrowDown />
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label="Move selected to bottom"
            data-queue-focus="toolbar:move:bottom"
            disabled={blocked || !canReorder}
            onClick={() => onMoveSelected("bottom")}
          >
            <ArrowDownToLine />
          </Button>
        </div>
        <ConfirmAction
          label="Clear"
          title="Clear the Queue?"
          description="All queued and completed entries will be removed. Files on disk are unchanged."
          disabled={blocked || !canClear}
          destructive
          onConfirm={onClear}
        />
        <ConfirmAction
          label="Clear Completed"
          title="Clear completed entries?"
          description="Successful, skipped, stopped, and not-worthwhile entries will be removed. Failed entries remain for inspection."
          disabled={blocked || !canClearCompleted}
          onConfirm={onClearCompleted}
        />
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        <Button size="sm" disabled={blocked || !canStart} onClick={onStart}>
          <Play data-icon="inline-start" aria-hidden="true" />
          Start Queue
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!running || stopPending || forcePending}
          onClick={onStopAfterCurrent}
        >
          {stopPending ? "Requesting Stop…" : "Stop After File"}
        </Button>
        <Button
          size="sm"
          variant="destructive"
          disabled={forcePending || (!running && !stopRequested)}
          onClick={onForceStop}
        >
          {forceStopping || forcePending ? "Force Stopping…" : "Force Stop"}
        </Button>
      </div>
    </div>
  );
}

export type QueuePendingAction =
  | "add-files"
  | "add-folders"
  | "remove"
  | "clear"
  | "clear-completed"
  | "start"
  | "edit"
  | "retry"
  | "open"
  | "reveal"
  | "stop-after-current"
  | "force-stop"
  | null;
