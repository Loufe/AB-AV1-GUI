import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui";
import type { ReactNode } from "react";
import { formatDurationMsCompact } from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";

import { basename, outputTargetLabel } from "./queue-status";
import type { QueueRowData } from "./queue-status";

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[5rem_minmax(0,1fr)] gap-2 text-xs">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate">{children}</dd>
    </div>
  );
}

/** Read-only details; editing belongs to #67. */
export function SelectionCard({ row }: { row: QueueRowData }) {
  return (
    <Card size="sm" className="gap-2">
      <CardHeader className="gap-0.5">
        <CardTitle className="text-sm">Selection · {basename(row.item.input)}</CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="flex flex-col gap-1.5">
          <Detail label="Path">
            <span className="selectable" title={row.item.input}>
              {row.item.input}
            </span>
          </Detail>
          <Detail label="Operation">{row.item.operation}</Detail>
          <Detail label="Output">
            {outputTargetLabel(row.item.operation, row.item.output_target)}
          </Detail>
          <Detail label="Input">{row.streams ?? "—"}</Detail>
          <Detail label="Size">
            {row.sizeBytes === null ? "—" : formatFileSize(row.sizeBytes)}
          </Detail>
          <Detail label="Time">{formatDurationMsCompact(row.timeMs)}</Detail>
        </dl>
      </CardContent>
    </Card>
  );
}
