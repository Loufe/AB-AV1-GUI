import { Play } from "lucide-react";

import { Button, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui";

import type { SessionState } from "@/lib/bindings";

const NOOP = () => undefined;

/** A disabled button that still explains itself on hover/focus. */
function DisabledWithReason({ label, reason }: { label: string; reason: string }) {
  return (
    <Tooltip>
      <TooltipTrigger render={<span tabIndex={0} />}>
        <Button size="sm" variant="outline" disabled>
          {label}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{reason}</TooltipContent>
    </Tooltip>
  );
}

export function QueueToolbar({
  session,
  queueEmpty,
  hasSelection,
  onAddFiles = NOOP,
  onRemove = NOOP,
  onStart = NOOP,
  onStopAfterCurrent = NOOP,
  onForceStop = NOOP,
}: {
  session: SessionState;
  queueEmpty: boolean;
  hasSelection: boolean;
  onAddFiles?: () => void;
  onRemove?: () => void;
  onStart?: () => void;
  onStopAfterCurrent?: () => void;
  onForceStop?: () => void;
}) {
  const running = session !== "Idle";
  return (
    <TooltipProvider>
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5">
          <Button size="sm" variant="outline" onClick={onAddFiles}>
            + Add Files
          </Button>
          <DisabledWithReason
            label="+ Add Folder"
            reason="Folder scanning arrives with the Analysis view"
          />
          <Button size="sm" variant="ghost" disabled={!hasSelection || running} onClick={onRemove}>
            Remove
          </Button>
          <DisabledWithReason label="Clear" reason="Not available yet" />
          <DisabledWithReason label="Clear Completed" reason="Not available yet" />
        </div>
        <div className="flex items-center gap-1.5">
          <Button size="sm" disabled={running || queueEmpty} onClick={onStart}>
            <Play data-icon="inline-start" aria-hidden="true" />
            Start Queue
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={session !== "Running"}
            onClick={onStopAfterCurrent}
          >
            Stop After File
          </Button>
          <Button size="sm" variant="destructive" disabled={!running} onClick={onForceStop}>
            Force Stop
          </Button>
        </div>
      </div>
    </TooltipProvider>
  );
}
