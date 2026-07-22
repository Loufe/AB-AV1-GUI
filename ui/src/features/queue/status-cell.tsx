import { CircleAlert, CircleCheck, CircleSlash } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui";
import { formatFileSize } from "@/lib/format/format";
import { cn } from "@/lib/utils";

import type { RowStatus } from "./queue-status";

/** User-facing verb per engine phase ("Encoding" reads as "Converting"). */
const PHASE_LABEL = {
  Preparing: "Preparing",
  Analyzing: "Analyzing",
  Encoding: "Converting",
  Remuxing: "Remuxing",
  Verifying: "Verifying",
  Finalizing: "Finalizing",
} as const;

const TONE_CLASS = {
  success: "text-success",
  warning: "text-warning",
  destructive: "text-destructive",
  muted: "text-muted-foreground",
} as const;

function StatusText({
  tone,
  icon: Icon,
  tooltip,
  children,
}: {
  tone: keyof typeof TONE_CLASS;
  icon?: React.ComponentType<{ className?: string }>;
  /** One line of facts the row doesn't show (D11: reasons ride the item). */
  tooltip?: React.ReactNode;
  children: React.ReactNode;
}) {
  if (!tooltip) {
    return (
      <span className={cn("flex min-w-0 items-center gap-1.5", TONE_CLASS[tone])}>
        {Icon && <Icon className="size-3.5 shrink-0" aria-hidden="true" />}
        <span className="truncate">{children}</span>
      </span>
    );
  }
  // Dotted underline = "more here"; the trigger is focusable so the detail
  // is reachable by keyboard, not hover-only (D8).
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            tabIndex={0}
            className={cn("flex min-w-0 cursor-help items-center gap-1.5", TONE_CLASS[tone])}
          />
        }
      >
        {Icon && <Icon className="size-3.5 shrink-0" aria-hidden="true" />}
        <span className="truncate underline decoration-current/40 decoration-dotted underline-offset-2">
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent>{tooltip}</TooltipContent>
    </Tooltip>
  );
}

/** Working rows carry their progress in the status cell, not a dialog. */
function WorkingStatus({ label, percent }: { label: string; percent: number | null }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 pr-3">
      <span className="text-foreground">
        {label}…{percent !== null && ` ${percent}%`}
      </span>
      <div className="h-0.5 w-full overflow-hidden rounded-full bg-muted">
        <div
          className={cn(
            "h-full rounded-full bg-primary",
            percent === null && "w-1/4 animate-pulse",
          )}
          style={percent === null ? undefined : { width: `${percent}%` }}
        />
      </div>
    </div>
  );
}

export function StatusCell({ status }: { status: RowStatus }) {
  switch (status.kind) {
    case "queued":
      return <StatusText tone="muted">Queued</StatusText>;
    case "starting":
      return <StatusText tone="muted">Starting…</StatusText>;
    case "working":
      return <WorkingStatus label={PHASE_LABEL[status.phase]} percent={status.percent} />;
    case "done": {
      let label =
        status.outcome === "Converted"
          ? "Done"
          : status.outcome === "Remuxed"
            ? "Done · remuxed"
            : "Analyzed";
      if (status.sizeDeltaBytes !== null) {
        label +=
          status.sizeDeltaBytes >= 0
            ? ` · saved ${formatFileSize(status.sizeDeltaBytes)}`
            : ` · grew ${formatFileSize(Math.abs(status.sizeDeltaBytes))}`;
      }
      return (
        <StatusText
          tone="success"
          icon={CircleCheck}
          tooltip={
            status.recovered
              ? "Recovered after restart; detailed run measurements are unavailable"
              : undefined
          }
        >
          {label}
        </StatusText>
      );
    }
    case "skipped":
      return (
        <StatusText tone="warning" icon={CircleSlash} tooltip={status.detail ?? undefined}>
          Skipped · {status.reason}
        </StatusText>
      );
    case "stopped":
      return <StatusText tone="muted">Stopped</StatusText>;
    case "failed":
      return (
        <StatusText
          tone="destructive"
          icon={CircleAlert}
          tooltip={
            status.diagnostic ? <span className="font-mono">{status.diagnostic}</span> : undefined
          }
        >
          Error · {status.message}
        </StatusText>
      );
  }
}
