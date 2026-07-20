import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui";

import type { Operation } from "@/lib/bindings";

import { basename } from "./queue-status";
import type { QueueRowData } from "./queue-status";

const NOOP = () => undefined;

/**
 * Properties panel for the selected item. Output settings are display-only
 * for now (no change-output command exists); the operation select is wired
 * through `onOperationChange` and disabled for items already past Queued.
 */
export function SelectionCard({
  row,
  onOperationChange = NOOP,
}: {
  row: QueueRowData;
  onOperationChange?: (operation: Operation) => void;
}) {
  const editable = row.item.state === "Queued";
  const isAnalyze = row.item.operation === "Analyze";
  const target = row.item.output_target;
  const suffix = target !== "Replace" && target.Suffix !== undefined ? target.Suffix.suffix : null;
  return (
    <Card size="sm" className="gap-2">
      <CardHeader className="gap-0.5">
        <CardTitle className="text-sm">Selection · {basename(row.item.input)}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <div className="flex items-center gap-2">
          <Label className="w-20 shrink-0 text-xs text-muted-foreground">Operation</Label>
          <Select
            value={row.item.operation}
            onValueChange={(value) => onOperationChange(value as Operation)}
            disabled={!editable}
          >
            <SelectTrigger size="sm" className="flex-1">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="Analyze">Analyze</SelectItem>
              <SelectItem value="Convert">Convert</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-2">
          <Label className="w-20 shrink-0 text-xs text-muted-foreground">Output</Label>
          <Select
            value={
              target === "Replace" ? "replace" : target.Suffix !== undefined ? "suffix" : "folder"
            }
            disabled
          >
            <SelectTrigger size="sm" className="flex-1">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="replace">Replace original</SelectItem>
              <SelectItem value="suffix">Save with suffix</SelectItem>
              <SelectItem value="folder">Separate folder</SelectItem>
            </SelectContent>
          </Select>
        </div>
        {suffix !== null && (
          <div className="flex items-center gap-2">
            <Label className="w-20 shrink-0 text-xs text-muted-foreground">Suffix</Label>
            <Input value={suffix} readOnly disabled className="h-7 flex-1" />
          </div>
        )}
        {isAnalyze && (
          <p className="text-xs text-muted-foreground">
            Analyze produces no output file — output settings are disabled.
          </p>
        )}
      </CardContent>
    </Card>
  );
}
