import { useState } from "react";
import { FolderPlus, Play, Plus } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
} from "@/components/ui";
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
  canClear,
  canClearCompleted,
  canStart,
  pendingAction,
  onAddFiles,
  onAddFolder,
  onRemove,
  onClear,
  onClearCompleted,
  onStart,
  onStopAfterCurrent,
  onForceStop,
}: {
  session: SessionState;
  mutationsDisabled: boolean;
  canRemove: boolean;
  canClear: boolean;
  canClearCompleted: boolean;
  canStart: boolean;
  pendingAction: QueuePendingAction;
  onAddFiles: () => void;
  onAddFolder: () => void;
  onRemove: () => void;
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
          label="Remove"
          title="Remove selected item?"
          description="The file stays on disk. Only this Queue entry is removed."
          disabled={blocked || !canRemove}
          onConfirm={onRemove}
        />
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
  | "stop-after-current"
  | "force-stop"
  | null;
