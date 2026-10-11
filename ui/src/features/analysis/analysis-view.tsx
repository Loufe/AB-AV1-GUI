import { FolderOpen } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";

import { EmptyState } from "@/components/empty-state";
import { useViewActive } from "@/components/layout/view-activity";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import type {
  AnalysisGenerationId,
  AnalysisQueueSummary,
  AnalysisRow,
  AnalysisRowId,
  Operation,
} from "@/lib/bindings";
import {
  basicScanAnalysis,
  cancelAnalysis,
  discoverAnalysis,
  enqueueAnalysis,
} from "@/lib/ipc/analysis";
import { pickPaths } from "@/lib/ipc/path-picker";
import { analysisStore, type NormalizedAnalysisGeneration } from "@/lib/store/analysis-store";
import { appStore, useAppStore } from "@/lib/store/app-store";
import { startBlockReason } from "@/lib/tools";
import { activityText, exclusionText } from "./analysis-presentation";
import { AnalysisDetails, AnalysisTable } from "./analysis-table";

function GenerationView({
  generation,
  epoch,
  blocked,
  choosing,
}: {
  generation: NormalizedAnalysisGeneration;
  epoch: number;
  blocked: string | null;
  choosing: boolean;
}) {
  const tools = useAppStore((state) => state.tools);
  const [selection, setSelection] = useState<ReadonlySet<AnalysisRowId>>(new Set());
  const [query, setQuery] = useState("");
  const [inspected, setInspected] = useState<AnalysisRowId | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<AnalysisQueueSummary | null>(null);
  const busy = useRef(false);
  const container = useRef<HTMLDivElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const rows = useMemo(
    () =>
      Object.values(generation.rows)
        .filter(
          (row): row is AnalysisRow =>
            row !== undefined &&
            (row.entry.File !== undefined ||
              (row.entry.Folder !== undefined && row.entry.Folder.failure !== null)),
        )
        .sort((left, right) => left.id - right.id),
    [generation.rows],
  );
  const fileIds = useMemo(
    () => new Set(rows.filter((row) => row.entry.File !== undefined).map((row) => row.id)),
    [rows],
  );
  const selected = useMemo(
    () => new Set([...selection].filter((id) => fileIds.has(id))),
    [selection, fileIds],
  );
  const filtered = useMemo(() => {
    const search = query.trim().toLocaleLowerCase();
    return search.length === 0
      ? rows
      : rows.filter((row) => row.display_path.text.toLocaleLowerCase().includes(search));
  }, [query, rows]);
  const visibleFiles = filtered.filter((row) => row.entry.File !== undefined);
  const live = generation.activity !== "Cancelled" && typeof generation.activity === "string";
  const scanning = generation.activity === "Discovering" || generation.activity === "BasicScanning";
  const disabled = blocked !== null || pending !== null || choosing;
  const toolBlock = startBlockReason(tools);
  const isCurrent = () =>
    appStore.getState().snapshotGeneration === epoch &&
    analysisStore.getState().current?.id === generation.id;
  const run = async (label: string, action: () => Promise<void>) => {
    if (busy.current || !isCurrent()) return;
    busy.current = true;
    setPending(label);
    setError(null);
    setSummary(null);
    try {
      await action();
    } catch (failure: unknown) {
      console.error(`Analysis ${label} failed`, failure);
      if (isCurrent()) setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      if (isCurrent()) {
        setPending(null);
        busy.current = false;
      }
    }
  };
  const enqueue = (operation: Operation, all: boolean) =>
    void run(`${operation} queue action`, async () => {
      const result = await enqueueAnalysis({
        generation: generation.id,
        operation,
        selection: all
          ? "AllFiles"
          : { Selected: { row_ids: [...selected].sort((left, right) => left - right) } },
      });
      if (isCurrent()) setSummary(result);
    });
  const details = inspected === null ? undefined : generation.rows[inspected];
  const allVisibleSelected =
    visibleFiles.length > 0 && visibleFiles.every((row) => selected.has(row.id));
  return (
    <div ref={container} className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-4 py-3">
        <Button
          variant="outline"
          disabled={
            disabled ||
            !(generation.activity === "Discovered" || generation.activity === "Ready") ||
            toolBlock !== null
          }
          onClick={() => void run("Basic Scan", () => basicScanAnalysis(generation.id))}
        >
          Basic Scan
        </Button>
        <Button
          variant="outline"
          disabled={disabled || !scanning}
          onClick={() => void run("Cancel scan", cancelAnalysis)}
        >
          Cancel scan
        </Button>
        <span role="status" className="text-sm text-muted-foreground">
          {activityText(generation.activity)} · {fileIds.size.toLocaleString()} files ·{" "}
          {selected.size.toLocaleString()} selected
        </span>
        {toolBlock !== null && <p className="w-full text-xs text-muted-foreground">{toolBlock}</p>}
        <Input
          ref={searchInput}
          aria-label="Search Analysis"
          placeholder="Search files and folders…"
          className="ml-auto max-w-xs"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-4 py-2">
        <Checkbox
          aria-label="Select visible files"
          checked={allVisibleSelected}
          indeterminate={!allVisibleSelected && visibleFiles.some((row) => selected.has(row.id))}
          disabled={visibleFiles.length === 0}
          onCheckedChange={(checked) =>
            setSelection((previous) => {
              const next = new Set(previous);
              for (const row of visibleFiles) {
                if (checked) next.add(row.id);
                else next.delete(row.id);
              }
              return next;
            })
          }
        />
        <span className="text-xs">Select visible files</span>
        <Button
          variant="ghost"
          disabled={selected.size === 0}
          onClick={() => setSelection(new Set())}
        >
          Clear selection
        </Button>
        <Button
          variant="outline"
          disabled={disabled || !live || selected.size === 0}
          onClick={() => enqueue("Analyze", false)}
        >
          Analyze selected
        </Button>
        <Button
          disabled={disabled || !live || selected.size === 0}
          onClick={() => enqueue("Convert", false)}
        >
          Convert selected
        </Button>
        <Button
          variant="outline"
          disabled={disabled || !live || fileIds.size === 0}
          onClick={() => enqueue("Analyze", true)}
        >
          Analyze all
        </Button>
        <Button
          variant="outline"
          disabled={disabled || !live || fileIds.size === 0}
          onClick={() => enqueue("Convert", true)}
        >
          Convert all
        </Button>
      </div>
      <div className="shrink-0 px-4 py-2 text-xs text-muted-foreground">
        <p>
          Analyze and Convert add work to Queue. Start Queue to run it. All actions include every
          discovered file, including files outside the search.
        </p>
        {pending !== null && <p role="status">{pending}…</p>}
        {error !== null && (
          <p role="alert" className="break-words text-destructive">
            {error}
          </p>
        )}
        {summary !== null && (
          <div role="status" className="text-foreground">
            <p>{summary.added} added to Queue.</p>
            {summary.excluded.length > 0 && (
              <ul className="flex flex-wrap gap-x-4">
                {summary.excluded.map(([reason, count], index) => (
                  <li key={index}>
                    {exclusionText(reason)}: {count}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>
      {filtered.length > 0 ? (
        <AnalysisTable
          rows={filtered}
          roots={generation.roots}
          selected={selected}
          onToggle={(id) =>
            setSelection((previous) => {
              const next = new Set(previous);
              if (next.has(id)) next.delete(id);
              else next.add(id);
              return next;
            })
          }
          onInspect={setInspected}
        />
      ) : (
        <EmptyState
          icon={FolderOpen}
          title={
            query.trim()
              ? "No matching files"
              : scanning
                ? "Discovering files…"
                : "No video files found"
          }
          description={
            query.trim()
              ? "Try another file or folder name."
              : "Folder failures appear as rows. Scan extensions are configured in Settings."
          }
        />
      )}
      {details !== undefined && (
        <AnalysisDetails
          row={details}
          onClose={() => {
            setInspected(null);
            const trigger = container.current?.querySelector<HTMLElement>(
              `[data-analysis-inspect="${details.id}"]`,
            );
            (trigger ?? searchInput.current)?.focus({ preventScroll: true });
          }}
        />
      )}
    </div>
  );
}

export function AnalysisView() {
  const active = useViewActive();
  const current = useStore(analysisStore, (state) => state.current);
  const settings = useAppStore((state) => state.settings);
  const epoch = useAppStore((state) => state.snapshotGeneration);
  const health = useAppStore((state) => state.health);
  const [choosing, setChoosing] = useState(false);
  const [feedback, setFeedback] = useState<{
    epoch: number;
    generation: AnalysisGenerationId | null;
    message: string;
  } | null>(null);
  const error =
    feedback?.epoch === epoch && feedback.generation === (current?.id ?? null)
      ? feedback.message
      : null;
  const pickerBusy = useRef(false);
  const chooseButton = useRef<HTMLButtonElement>(null);
  const focusWithin = useRef(false);
  const view = useRef<HTMLDivElement>(null);
  const blocked =
    health.unavailable ??
    health.fatal ??
    health.secondInstance ??
    health.degraded?.reason ??
    (settings === null ? "Waiting for the desktop engine’s current snapshot." : null);
  useEffect(() => {
    if (active && focusWithin.current) chooseButton.current?.focus();
  }, [current?.id, epoch, active, choosing]);
  const choose = async () => {
    if (pickerBusy.current || blocked !== null) return;
    pickerBusy.current = true;
    setChoosing(true);
    setFeedback(null);
    const generation = current?.id ?? null;
    const isCurrent = () =>
      appStore.getState().snapshotGeneration === epoch &&
      (analysisStore.getState().current?.id ?? null) === generation;
    try {
      const roots = await pickPaths("Folders", settings?.last_input_folder ?? null);
      if (roots.length > 0 && isCurrent()) {
        await discoverAnalysis(roots);
      }
    } catch (failure: unknown) {
      console.error("Analysis folder discovery failed", failure);
      if (isCurrent())
        setFeedback({
          epoch,
          generation,
          message: failure instanceof Error ? failure.message : String(failure),
        });
    } finally {
      setChoosing(false);
      pickerBusy.current = false;
    }
  };
  return (
    <div
      ref={view}
      className="flex h-full min-h-0 flex-col"
      onFocusCapture={() => {
        focusWithin.current = true;
      }}
      onBlurCapture={(event) => {
        if (event.relatedTarget !== null && !view.current?.contains(event.relatedTarget as Node))
          focusWithin.current = false;
      }}
    >
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-3">
        <div>
          <h1 className="font-medium">Analysis</h1>
          <p className="text-xs text-muted-foreground">
            Discover files, inspect quality searches, and add work to Queue.
          </p>
        </div>
        <Button
          ref={chooseButton}
          disabled={blocked !== null || choosing}
          onClick={() => void choose()}
        >
          Choose folders…
        </Button>
        {blocked !== null && (
          <p role="status" className="w-full text-sm text-muted-foreground">
            {blocked}
          </p>
        )}
        {error !== null && (
          <p role="alert" className="w-full break-words text-sm text-destructive">
            {error}
          </p>
        )}
      </div>
      {current === null ? (
        <EmptyState
          icon={FolderOpen}
          title="No folder selected"
          description="Choose folders to discover videos. Basic Scan reads metadata; Analyze runs quality searches through Queue."
        />
      ) : (
        <GenerationView
          key={`${epoch}:${current.id}`}
          generation={current}
          epoch={epoch}
          blocked={blocked}
          choosing={choosing}
        />
      )}
    </div>
  );
}
