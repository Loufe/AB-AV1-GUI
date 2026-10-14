import { defaultRangeExtractor, useVirtualizer } from "@tanstack/react-virtual";
import { CircleAlert } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";

import { Checkbox } from "@/components/ui/checkbox";
import type { AnalysisDisplayText, AnalysisRow, AnalysisRowId } from "@/lib/bindings";
import {
  formatDurationMsClock,
  formatEngineCrf,
  formatEngineVmafScore,
} from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";
import { containerLabel, videoCodecLabel } from "@/lib/format/media";
import { metadataResolution, relativeFolder, rowPresentation } from "./analysis-presentation";

const ROW_HEIGHT = 80;
const OVERSCAN_ROWS = 6;
const COLUMN_WIDTHS = [44, 280, 180, 225, 110, 155, 140];
const COLUMN_NAMES = [
  "Selection",
  "File / relative folder",
  "Media",
  "Current status",
  "Current file size",
  "Output size",
  "Encoding time",
];

function bytes(value: number | null): string {
  return value === null ? "—" : formatFileSize(value);
}

function ResultCells({ row }: { row: AnalysisRow }) {
  const { prediction, measured } = rowPresentation(row);
  const result = measured?.Encoded ?? measured?.Remuxed;
  return (
    <>
      <td
        className="flex shrink-0 flex-col justify-center px-2 text-xs tabular-nums"
        style={{ width: COLUMN_WIDTHS[5] }}
      >
        <span>{bytes(prediction?.predicted_size ?? result?.output_size ?? null)}</span>
        <span className="text-muted-foreground">
          {prediction !== null ? "Search prediction" : measured !== null ? "Measured output" : "—"}
        </span>
      </td>
      <td
        className="flex shrink-0 flex-col justify-center px-2 text-xs tabular-nums"
        style={{ width: COLUMN_WIDTHS[6] }}
      >
        <span>
          {formatDurationMsClock(
            prediction?.predicted_duration_ms ?? measured?.Encoded?.encoding_time ?? null,
          )}
        </span>
        <span className="text-muted-foreground">
          {prediction !== null
            ? "Search prediction"
            : measured?.Encoded !== undefined
              ? "Measured encode"
              : "—"}
        </span>
      </td>
    </>
  );
}

export function AnalysisDetails({ row, onClose }: { row: AnalysisRow; onClose: () => void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  const facts = rowPresentation(row);
  const prediction = facts.prediction;
  useEffect(() => {
    heading.current?.focus();
  }, [row.id]);
  return (
    <section
      aria-label="File details"
      className="max-h-52 shrink-0 overflow-auto border-t border-border p-3 text-sm"
    >
      <div className="flex items-center justify-between gap-2">
        <h2 ref={heading} tabIndex={-1} className="font-medium outline-none">
          Details for {row.display_name.text}
        </h2>
        <button className="rounded px-2 py-1 underline focus-visible:ring-2" onClick={onClose}>
          Close details
        </button>
      </div>
      <p className="break-all font-mono text-xs">{row.display_path.text}</p>
      {(row.display_path.lossy || row.display_name.lossy) && (
        <p>
          The display path contains replacement characters. Actions use the engine’s native path.
        </p>
      )}
      <p>{facts.label}</p>
      {facts.detail !== null && (
        <p
          className={
            facts.failed ? "whitespace-pre-wrap text-destructive" : "text-muted-foreground"
          }
        >
          {facts.detail}
        </p>
      )}
      {facts.analyze !== null && (
        <p>
          Analyze: {facts.analyze}. Convert: {facts.convert}.
        </p>
      )}
      {prediction !== null && (
        <p>
          Applicable search prediction: {bytes(prediction.predicted_size)},{" "}
          {formatDurationMsClock(prediction.predicted_duration_ms)} encode time, VMAF{" "}
          {formatEngineVmafScore({ metric: prediction.metric, score: prediction.score })}, CRF{" "}
          {formatEngineCrf(prediction.crf)}. Requested target {prediction.requested_target};
          successful target {prediction.successful_target}.
        </p>
      )}
    </section>
  );
}

export function AnalysisTable({
  rows,
  roots,
  selected,
  onToggle,
  onInspect,
}: {
  rows: readonly AnalysisRow[];
  roots: readonly AnalysisDisplayText[];
  selected: ReadonlySet<AnalysisRowId>;
  onToggle: (id: AnalysisRowId) => void;
  onInspect: (id: AnalysisRowId) => void;
}) {
  const scroll = useRef<HTMLDivElement>(null);
  const [focused, setFocused] = useState<AnalysisRowId | null>(null);
  const requestedFocus = useRef<AnalysisRowId | null>(null);
  const focusedIndex = rows.findIndex((row) => row.id === focused);
  const rangeExtractor = useCallback(
    (range: Parameters<typeof defaultRangeExtractor>[0]) => {
      const visible = defaultRangeExtractor(range);
      return focusedIndex < 0
        ? visible
        : [...new Set([...visible, focusedIndex])].sort((left, right) => left - right);
    },
    [focusedIndex],
  );
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroll.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => rows[index]?.id ?? index,
    initialRect: { width: 0, height: 520 },
    overscan: OVERSCAN_ROWS,
    rangeExtractor,
  });
  const virtualRows = virtualizer.getVirtualItems();
  useEffect(() => {
    if (requestedFocus.current === null) return;
    const target = scroll.current?.querySelector<HTMLElement>(
      `[data-analysis-focus="${requestedFocus.current}"]`,
    );
    if (target !== null && target !== undefined) {
      target.focus({ preventScroll: true });
      requestedFocus.current = null;
    }
  });
  const navigate = (event: KeyboardEvent, index: number) => {
    const keys: Record<string, number> = {
      ArrowDown: index + 1,
      ArrowUp: index - 1,
      Home: 0,
      End: rows.length - 1,
      PageDown: index + Math.max(1, Math.floor((scroll.current?.clientHeight ?? 520) / ROW_HEIGHT)),
      PageUp: index - Math.max(1, Math.floor((scroll.current?.clientHeight ?? 520) / ROW_HEIGHT)),
    };
    const next = keys[event.key];
    if (next === undefined) return;
    event.preventDefault();
    const nextIndex = Math.max(0, Math.min(rows.length - 1, next));
    const row = rows[nextIndex];
    if (row === undefined) return;
    requestedFocus.current = row.id;
    setFocused(row.id);
    virtualizer.scrollToIndex(nextIndex, { align: "auto" });
  };
  return (
    <div ref={scroll} className="min-h-0 flex-1 overflow-auto" data-analysis-scroll>
      <table
        aria-label="Analysis files"
        aria-rowcount={rows.length + 1}
        className="w-full min-w-[1134px] text-left"
      >
        <thead className="sticky top-0 z-10 bg-muted text-xs text-muted-foreground">
          <tr className="flex">
            {COLUMN_NAMES.map((label, index) => (
              <th
                key={label}
                scope="col"
                className="shrink-0 px-2 py-2 font-medium"
                style={{ width: COLUMN_WIDTHS[index] }}
              >
                <span className={index === 0 ? "sr-only" : undefined}>{label}</span>
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="relative grid" style={{ height: virtualizer.getTotalSize() }}>
          {virtualRows.map((virtual) => {
            const row = rows[virtual.index];
            if (row === undefined) return null;
            const facts = rowPresentation(row);
            const file = row.entry.File !== undefined;
            return (
              <tr
                key={row.id}
                data-analysis-row={row.id}
                aria-rowindex={virtual.index + 2}
                className="absolute flex w-full border-b border-border/50 bg-background text-sm data-[selected=true]:bg-primary/8"
                data-selected={selected.has(row.id) ? "true" : undefined}
                style={{ height: ROW_HEIGHT, transform: `translateY(${virtual.start}px)` }}
                onFocusCapture={() => setFocused(row.id)}
                onKeyDown={(event) => navigate(event, virtual.index)}
              >
                <td className="flex shrink-0 items-center px-2" style={{ width: COLUMN_WIDTHS[0] }}>
                  {file && (
                    <Checkbox
                      aria-label={`Select ${row.display_name.text}`}
                      data-analysis-focus={row.id}
                      checked={selected.has(row.id)}
                      onCheckedChange={() => onToggle(row.id)}
                    />
                  )}
                </td>
                <td
                  className="flex min-w-0 shrink-0 flex-col justify-center px-2"
                  style={{ width: COLUMN_WIDTHS[1] }}
                >
                  <button
                    data-analysis-focus={file ? undefined : row.id}
                    data-analysis-inspect={row.id}
                    className="truncate text-left font-medium underline decoration-transparent focus-visible:decoration-current focus-visible:outline-ring hover:decoration-current/40"
                    aria-label={`Inspect ${row.display_name.text}`}
                    onClick={() => onInspect(row.id)}
                  >
                    {row.display_name.text}
                  </button>
                  <span className="truncate text-xs text-muted-foreground">
                    {relativeFolder(row, roots)}
                  </span>
                </td>
                <td
                  className="flex shrink-0 flex-col justify-center px-2 text-xs"
                  style={{ width: COLUMN_WIDTHS[2] }}
                >
                  <span>
                    {videoCodecLabel(facts.metadata?.codec ?? null)} ·{" "}
                    {containerLabel(facts.metadata?.container ?? null)}
                  </span>
                  <span className="text-muted-foreground">
                    {metadataResolution(facts.metadata)}
                  </span>
                  <span className="text-muted-foreground tabular-nums">
                    {formatDurationMsClock(facts.metadata?.duration_ms ?? null)}
                  </span>
                </td>
                <td
                  className="flex shrink-0 flex-col justify-center px-2 text-xs"
                  style={{ width: COLUMN_WIDTHS[3] }}
                >
                  <span
                    className={
                      facts.failed
                        ? "flex items-center gap-1 font-medium text-destructive"
                        : "font-medium"
                    }
                  >
                    {facts.failed && <CircleAlert className="size-3" aria-hidden="true" />}
                    {facts.label}
                  </span>
                  {facts.analyze !== null && (
                    <span className="truncate text-muted-foreground">Analyze: {facts.analyze}</span>
                  )}
                  {facts.convert !== null && (
                    <span className="truncate text-muted-foreground">Convert: {facts.convert}</span>
                  )}
                  {facts.failed && (
                    <span className="text-muted-foreground">Inspect for details</span>
                  )}
                </td>
                <td
                  className="flex shrink-0 items-center px-2 text-xs tabular-nums"
                  style={{ width: COLUMN_WIDTHS[4] }}
                >
                  {bytes(facts.metadata?.size_bytes ?? null)}
                </td>
                <ResultCells row={row} />
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
