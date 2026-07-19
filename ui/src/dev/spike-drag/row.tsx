import { FileVideo, Folder, GripVertical } from "lucide-react";

import { cn } from "@/lib/utils";

import type { SpikeRow } from "./data";

export const ROW_HEIGHT = 32;

interface SpikeRowViewProps {
  row: SpikeRow;
  ref?: React.Ref<HTMLDivElement>;
  style?: React.CSSProperties;
  /** Source row still in the list while its copy rides the overlay/preview. */
  isDragSource?: boolean;
  /** Rendered inside a drag overlay / custom native preview. */
  isOverlay?: boolean;
  handleRef?: React.Ref<HTMLButtonElement>;
  handleProps?: React.ComponentPropsWithoutRef<"button">;
  /** Extra root-element props (e.g. hello-pangea's draggableProps). */
  rootProps?: React.HTMLAttributes<HTMLDivElement>;
  /** Drop indicator (absolutely positioned) and similar adornments. */
  children?: React.ReactNode;
}

/**
 * Presentation shared by both drag candidates so the comparison only
 * exercises the drag layer, never the row rendering.
 */
export function SpikeRowView({
  row,
  ref,
  style,
  isDragSource,
  isOverlay,
  handleRef,
  handleProps,
  rootProps,
  children,
}: SpikeRowViewProps) {
  const Icon = row.kind === "folder" ? Folder : FileVideo;
  return (
    <div
      ref={ref}
      {...rootProps}
      style={style}
      className={cn(
        "relative flex h-8 items-center gap-1.5 border-b border-border/40 bg-background px-2 text-sm",
        row.kind === "folder" && "bg-surface font-medium",
        isDragSource && "opacity-40",
        isOverlay && "w-80 rounded-md border border-border bg-elevated shadow-lg",
      )}
    >
      <button
        ref={handleRef}
        type="button"
        aria-label={`Reorder ${row.label}`}
        className="cursor-grab rounded p-0.5 text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        {...handleProps}
      >
        <GripVertical className="size-3.5" aria-hidden="true" />
      </button>
      <Icon
        className={cn("size-4 shrink-0 text-muted-foreground", row.kind === "file" && "ml-5")}
        aria-hidden="true"
      />
      <span className="truncate">{row.label}</span>
      {children}
    </div>
  );
}
