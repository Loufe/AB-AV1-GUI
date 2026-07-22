import type { ReactNode } from "react";
import { ExternalLink, FolderSearch, RotateCcw } from "lucide-react";

import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui";
import type { Operation, OutputTarget, OverwriteDecision, QueueItemEdit } from "@/lib/bindings";
import { formatDurationMsCompact } from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";

import { basename, outputTargetLabel } from "./queue-status";
import type { QueueRowData } from "./queue-status";

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[5rem_minmax(0,1fr)] items-center gap-2 text-xs">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}

const emptyEdit = (): QueueItemEdit => ({
  operation: null,
  intent: null,
  output_target: null,
  overwrite: null,
});

function outputKind(target: OutputTarget): "Replace" | "Suffix" | "SeparateFolder" {
  if (target === "Replace") return "Replace";
  return target.Suffix !== undefined ? "Suffix" : "SeparateFolder";
}

export function SelectionCard({
  row,
  editable,
  busy,
  canRecover,
  canRetry,
  suffixDefault,
  separateFolderDefault,
  onEdit,
  onRetry,
  onRecover,
  onOpen,
  onReveal,
}: {
  row: QueueRowData;
  editable: boolean;
  busy: boolean;
  canRecover: boolean;
  canRetry: boolean;
  suffixDefault: string | null;
  separateFolderDefault: string | null;
  onEdit: (patch: QueueItemEdit) => void;
  onRetry: () => void;
  onRecover: (operation: Operation) => void;
  onOpen: () => void;
  onReveal: () => void;
}) {
  const { item } = row;
  const finished = item.state !== "Queued" && "Finished" in item.state;
  const targetKind = outputKind(item.output_target);
  const suffixTarget = item.output_target === "Replace" ? undefined : item.output_target.Suffix;
  const separateFolderTarget =
    item.output_target === "Replace" ? undefined : item.output_target.SeparateFolder;
  return (
    <Card size="sm" className="gap-2">
      <CardHeader className="flex-row items-center justify-between gap-2">
        <CardTitle className="flex min-w-0 items-center gap-1.5 text-sm">
          <span className="truncate">Selection · {basename(item.input)}</span>
          {item.intent === "Refresh" && (
            <span className="inline-flex items-center gap-1 text-xs font-normal text-primary">
              <RotateCcw className="size-3" aria-hidden="true" /> Refresh
            </span>
          )}
        </CardTitle>
        <div className="flex gap-1">
          <Button size="xs" variant="ghost" disabled={busy} onClick={onOpen}>
            <ExternalLink aria-hidden="true" /> Open
          </Button>
          <Button size="xs" variant="ghost" disabled={busy} onClick={onReveal}>
            <FolderSearch aria-hidden="true" /> Reveal
          </Button>
        </div>
      </CardHeader>
      <CardContent className="grid gap-1.5">
        <Detail label="Path">
          <span className="block truncate selectable" title={item.input}>
            {item.input}
          </span>
        </Detail>
        <Detail label="Operation">
          <Select
            value={item.operation}
            disabled={!editable || busy}
            onValueChange={(value) => onEdit({ ...emptyEdit(), operation: value as Operation })}
          >
            <SelectTrigger size="sm" aria-label="Operation">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="Analyze">Analyze</SelectItem>
              <SelectItem value="Convert">Convert</SelectItem>
            </SelectContent>
          </Select>
        </Detail>
        {item.operation === "Convert" && (
          <>
            <Detail label="Output">
              <Select
                value={targetKind}
                disabled={!editable || busy}
                onValueChange={(value) => {
                  let output_target: OutputTarget = "Replace";
                  if (value === "Suffix") {
                    output_target =
                      item.output_target !== "Replace" && item.output_target.Suffix !== undefined
                        ? item.output_target
                        : suffixDefault === null
                          ? item.output_target
                          : { Suffix: { suffix: suffixDefault } };
                  } else if (value === "SeparateFolder") {
                    output_target =
                      item.output_target !== "Replace" &&
                      item.output_target.SeparateFolder !== undefined
                        ? item.output_target
                        : separateFolderDefault === null
                          ? item.output_target
                          : {
                              SeparateFolder: {
                                directory: separateFolderDefault,
                                source_root: null,
                              },
                            };
                  }
                  onEdit({ ...emptyEdit(), output_target });
                }}
              >
                <SelectTrigger size="sm" aria-label="Output target">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="Replace">Replace</SelectItem>
                  <SelectItem
                    value="Suffix"
                    disabled={suffixDefault === null && targetKind !== "Suffix"}
                  >
                    Suffix
                  </SelectItem>
                  <SelectItem
                    value="SeparateFolder"
                    disabled={separateFolderDefault === null && targetKind !== "SeparateFolder"}
                  >
                    {separateFolderDefault === null && targetKind !== "SeparateFolder"
                      ? "Separate folder (configure in Settings)"
                      : "Separate folder"}
                  </SelectItem>
                </SelectContent>
              </Select>
            </Detail>
            {suffixTarget !== undefined && (
              <Detail label="Suffix">
                <Input
                  key={suffixTarget.suffix}
                  aria-label="Output suffix"
                  className="h-7 max-w-52"
                  defaultValue={suffixTarget.suffix}
                  disabled={!editable || busy}
                  onBlur={(event) => {
                    const suffix = event.currentTarget.value;
                    if (suffix.length === 0 || suffix === suffixTarget.suffix) return;
                    onEdit({
                      ...emptyEdit(),
                      output_target: { Suffix: { suffix } },
                    });
                  }}
                />
              </Detail>
            )}
            {separateFolderTarget !== undefined && (
              <Detail label="Folder">
                <Input
                  key={separateFolderTarget.directory}
                  aria-label="Output folder"
                  className="h-7"
                  defaultValue={separateFolderTarget.directory}
                  disabled={!editable || busy}
                  onBlur={(event) => {
                    const directory = event.currentTarget.value;
                    if (directory.length === 0 || directory === separateFolderTarget.directory)
                      return;
                    onEdit({
                      ...emptyEdit(),
                      output_target: {
                        SeparateFolder: {
                          directory,
                          source_root: separateFolderTarget.source_root,
                        },
                      },
                    });
                  }}
                />
              </Detail>
            )}
            <Detail label="Overwrite">
              <Select
                value={item.overwrite}
                disabled={!editable || busy}
                onValueChange={(value) =>
                  onEdit({ ...emptyEdit(), overwrite: value as OverwriteDecision })
                }
              >
                <SelectTrigger size="sm" aria-label="Overwrite decision">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="FollowSettings">Follow Settings</SelectItem>
                  <SelectItem value="Allow">Allow</SelectItem>
                  <SelectItem value="Deny">Deny</SelectItem>
                </SelectContent>
              </Select>
            </Detail>
          </>
        )}
        <Detail label="Stored output">{outputTargetLabel("Convert", item.output_target)}</Detail>
        <Detail label="Input">{row.streams ?? "—"}</Detail>
        <Detail label="Size">{row.sizeBytes === null ? "—" : formatFileSize(row.sizeBytes)}</Detail>
        <Detail label="Time">{formatDurationMsCompact(row.timeMs)}</Detail>
        {finished && (
          <div className="mt-1 flex flex-wrap gap-1 border-t border-border pt-2">
            <Button size="xs" variant="outline" disabled={busy || !canRetry} onClick={onRetry}>
              Retry
            </Button>
            <Button
              size="xs"
              variant="outline"
              disabled={busy || !canRecover}
              onClick={() => onRecover("Convert")}
            >
              Convert anyway
            </Button>
            <Button
              size="xs"
              variant="outline"
              disabled={busy || !canRecover}
              onClick={() => onRecover("Analyze")}
            >
              Re-analyze
            </Button>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
