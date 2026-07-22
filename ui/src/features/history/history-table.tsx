import {
  type Column,
  type ColumnDef,
  type ColumnFiltersState,
  flexRender,
  getCoreRowModel,
  getFilteredRowModel,
  getSortedRowModel,
  type Row,
  type RowSelectionState,
  type SortingState,
  useReactTable,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  CircleAlert,
  CircleCheck,
  CircleSlash,
  Ellipsis,
  ExternalLink,
  FileSearch,
  FolderSearch,
  Search,
  SquareStop,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

import {
  Button,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
  Input,
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui";
import {
  formatDurationMsClock,
  formatEngineCrf,
  formatEngineVmafScore,
  formatUnixMillisDate,
} from "@/lib/format/engine-values";
import { formatFileSize, formatResolution } from "@/lib/format/format";
import { openPath, revealInFileManager } from "@/lib/ipc";
import { cn } from "@/lib/utils";

import {
  audioSummary,
  compareHistoryDefault,
  containerLabel,
  HISTORY_STATUSES,
  type HistoryDisplayRow,
  type HistoryStatusLabel,
  historyTotals,
  videoCodecLabel,
} from "./history-model";

const ROW_HEIGHT = 52;
const OVERSCAN_ROWS = 6;
const EM_DASH = "—";

function formatBytes(value: number | null): string {
  return value === null ? EM_DASH : formatFileSize(value);
}

function formatReductionChange(value: number | null): { text: string; label: string } {
  if (value === null || !Number.isFinite(value)) return { text: EM_DASH, label: "Unavailable" };
  if (value > 0) {
    const percent = value.toFixed(1);
    return { text: `−${percent}%`, label: `${percent}% smaller` };
  }
  if (value < 0) {
    const percent = Math.abs(value).toFixed(1);
    return { text: `+${percent}%`, label: `${percent}% larger` };
  }
  return { text: "0.0%", label: "No size change" };
}

function formatSignedSavedBytes(value: number): string {
  if (!Number.isFinite(value)) return EM_DASH;
  if (value > 0) return `${formatFileSize(value)} saved`;
  if (value < 0) return `${formatFileSize(Math.abs(value))} larger`;
  return "No size change";
}

function PathText({ displayRow }: { displayRow: HistoryDisplayRow }) {
  const provenance =
    displayRow.provenance === "adopted"
      ? "Imported"
      : displayRow.provenance === "parked"
        ? "Unresolved import"
        : null;
  const secondary =
    provenance ?? (displayRow.path === null ? "No path recorded" : displayRow.label);
  return (
    <div className="flex min-w-0 flex-col justify-center">
      {displayRow.path === null ? (
        <span className="truncate font-medium">{displayRow.basename}</span>
      ) : (
        <Tooltip>
          <TooltipTrigger
            render={
              <span
                tabIndex={0}
                className="truncate text-left font-medium underline decoration-transparent underline-offset-2 hover:decoration-current/30 focus:decoration-current/40"
              />
            }
          >
            {displayRow.basename}
          </TooltipTrigger>
          <TooltipContent className="max-w-lg break-all font-mono">
            {displayRow.label}
          </TooltipContent>
        </Tooltip>
      )}
      <span className="truncate text-[11px] text-muted-foreground">{secondary}</span>
    </div>
  );
}

function MediaText({ displayRow }: { displayRow: HistoryDisplayRow }) {
  const { row } = displayRow;
  const video = videoCodecLabel(row.codec);
  const container = containerLabel(row.container);
  const primary =
    video === EM_DASH && container === EM_DASH
      ? EM_DASH
      : [video, container].filter((value) => value !== EM_DASH).join(" · ");
  const resolution = formatResolution(row.width, row.height);
  const duration = formatDurationMsClock(row.duration_ms);
  const audio = audioSummary(row.audio);
  const secondary = [resolution, duration, audio].filter((value) => value !== EM_DASH).join(" · ");
  return (
    <div className="flex min-w-0 flex-col justify-center text-xs">
      <span className="truncate">{primary}</span>
      <span className="truncate text-[11px] text-muted-foreground">{secondary || EM_DASH}</span>
    </div>
  );
}

function QualityText({ displayRow }: { displayRow: HistoryDisplayRow }) {
  const { crf, vmaf } = displayRow.row;
  if (crf === null && vmaf === null) {
    return <span className="text-muted-foreground">{EM_DASH}</span>;
  }
  return (
    <span className="whitespace-nowrap text-muted-foreground tabular-nums">
      VMAF {formatEngineVmafScore(vmaf)} · CRF {formatEngineCrf(crf)}
    </span>
  );
}

const STATUS_ICONS = {
  Converted: CircleCheck,
  Remuxed: CircleCheck,
  "Not Worthwhile": CircleSlash,
  Analyzed: FileSearch,
  Failed: CircleAlert,
  Stopped: SquareStop,
} satisfies Record<HistoryStatusLabel, React.ComponentType<{ className?: string }>>;

function StatusText({ displayRow }: { displayRow: HistoryDisplayRow }) {
  const Icon = STATUS_ICONS[displayRow.status];
  const tone =
    displayRow.status === "Converted" || displayRow.status === "Remuxed"
      ? "text-success"
      : displayRow.status === "Failed"
        ? "text-destructive"
        : displayRow.status === "Not Worthwhile"
          ? "text-warning"
          : "text-muted-foreground";
  const body = (
    <span className={cn("flex min-w-0 items-center gap-1.5", tone)}>
      <Icon className="size-3.5 shrink-0" aria-hidden="true" />
      <span className="truncate">{displayRow.status}</span>
    </span>
  );
  if (displayRow.statusDetail === null) return body;
  return (
    <Tooltip>
      <TooltipTrigger
        render={<span tabIndex={0} className="min-w-0 cursor-help underline decoration-dotted" />}
      >
        {body}
      </TooltipTrigger>
      <TooltipContent className="max-w-sm">{displayRow.statusDetail}</TooltipContent>
    </Tooltip>
  );
}

function reportActionFailure(action: string, error: unknown): void {
  console.error(`failed to ${action} History path`, error);
  toast.error(error instanceof Error ? error.message : `Could not ${action} this path`);
}

function runPathAction(action: "open" | "reveal", path: string): void {
  const request = action === "open" ? openPath(path) : revealInFileManager(path);
  void request.catch((error: unknown) => reportActionFailure(action, error));
}

function RowActions({ displayRow }: { displayRow: HistoryDisplayRow }) {
  if (displayRow.path === null) {
    return <span className="text-muted-foreground">{EM_DASH}</span>;
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={`Actions for ${displayRow.basename}`}
            onClick={(event) => event.stopPropagation()}
          />
        }
      >
        <Ellipsis aria-hidden="true" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {displayRow.provenance !== "parked" && (
          <DropdownMenuItem onClick={() => runPathAction("open", displayRow.path ?? "")}>
            <ExternalLink aria-hidden="true" />
            Open file
          </DropdownMenuItem>
        )}
        <DropdownMenuItem onClick={() => runPathAction("reveal", displayRow.path ?? "")}>
          <FolderSearch aria-hidden="true" />
          Reveal in file manager
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function SortHeader({ column, label }: { column: Column<HistoryDisplayRow>; label: string }) {
  const direction = column.getIsSorted();
  const Icon = direction === "asc" ? ArrowUp : direction === "desc" ? ArrowDown : ArrowUpDown;
  return (
    <Button
      variant="ghost"
      size="xs"
      className="-ml-2 h-6 px-1.5 text-xs font-medium text-muted-foreground"
      onClick={column.getToggleSortingHandler()}
      aria-label={
        direction === false
          ? `Sort by ${label}`
          : `Sort by ${label}, currently ${direction === "asc" ? "ascending" : "descending"}`
      }
    >
      {label}
      <Icon aria-hidden="true" />
    </Button>
  );
}

function columns(): ColumnDef<HistoryDisplayRow>[] {
  return [
    {
      id: "file",
      accessorKey: "basename",
      header: ({ column }) => <SortHeader column={column} label="File" />,
      cell: ({ row }) => <PathText displayRow={row.original} />,
      size: 245,
    },
    {
      id: "date",
      accessorFn: (displayRow) => displayRow.row.happened_at ?? undefined,
      header: ({ column }) => <SortHeader column={column} label="Date" />,
      cell: ({ row }) => (
        <span className="text-muted-foreground tabular-nums">
          {row.original.row.happened_at === null
            ? EM_DASH
            : formatUnixMillisDate(row.original.row.happened_at)}
        </span>
      ),
      sortUndefined: "last",
      size: 105,
    },
    {
      id: "media",
      header: "Media",
      cell: ({ row }) => <MediaText displayRow={row.original} />,
      enableSorting: false,
      size: 195,
    },
    {
      id: "before",
      accessorFn: (displayRow) => displayRow.row.input_size_bytes ?? undefined,
      header: ({ column }) => <SortHeader column={column} label="Before" />,
      cell: ({ row }) => formatBytes(row.original.row.input_size_bytes),
      sortUndefined: "last",
      size: 92,
      meta: { align: "right" },
    },
    {
      id: "after",
      accessorFn: (displayRow) => displayRow.row.output_size_bytes ?? undefined,
      header: ({ column }) => <SortHeader column={column} label="After" />,
      cell: ({ row }) => formatBytes(row.original.row.output_size_bytes),
      sortUndefined: "last",
      size: 92,
      meta: { align: "right" },
    },
    {
      id: "change",
      accessorKey: "reductionPercent",
      header: ({ column }) => <SortHeader column={column} label="Change" />,
      cell: ({ row }) => {
        const change = formatReductionChange(row.original.reductionPercent);
        return (
          <span
            aria-label={change.label}
            className={cn(
              "font-medium tabular-nums",
              row.original.reductionPercent !== null && row.original.reductionPercent > 0
                ? "text-success"
                : row.original.reductionPercent !== null && row.original.reductionPercent < 0
                  ? "text-destructive"
                  : "text-muted-foreground",
            )}
          >
            {change.text}
          </span>
        );
      },
      sortUndefined: "last",
      size: 82,
      meta: { align: "right" },
    },
    {
      id: "quality",
      accessorFn: (displayRow) => displayRow.row.crf ?? undefined,
      header: ({ column }) => <SortHeader column={column} label="Quality" />,
      cell: ({ row }) => <QualityText displayRow={row.original} />,
      sortUndefined: "last",
      size: 156,
      meta: { align: "right" },
    },
    {
      id: "time",
      accessorFn: (displayRow) => displayRow.row.encoding_time_ms ?? undefined,
      header: ({ column }) => <SortHeader column={column} label="Took" />,
      cell: ({ row }) => (
        <span className="text-muted-foreground tabular-nums">
          {formatDurationMsClock(row.original.row.encoding_time_ms)}
        </span>
      ),
      sortUndefined: "last",
      size: 82,
      meta: { align: "right" },
    },
    {
      id: "status",
      accessorKey: "status",
      header: ({ column }) => <SortHeader column={column} label="Outcome" />,
      cell: ({ row }) => <StatusText displayRow={row.original} />,
      filterFn: "equalsString",
      size: 158,
    },
    {
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: ({ row }) => <RowActions displayRow={row.original} />,
      enableSorting: false,
      size: 42,
      meta: { align: "center" },
    },
  ];
}

function cellAlignment(column: Column<HistoryDisplayRow>): string {
  const align = (column.columnDef.meta as { align?: "center" | "right" } | undefined)?.align;
  return align === "right"
    ? "justify-end text-right"
    : align === "center"
      ? "justify-center text-center"
      : "justify-start text-left";
}

interface VirtualBodyProps {
  rows: Row<HistoryDisplayRow>[];
  scrollRef: React.RefObject<HTMLDivElement | null>;
  focusedRowId: string | null;
  onRowFocus: (rowId: string) => void;
}

interface VirtualHistoryRowProps {
  row: Row<HistoryDisplayRow>;
  start: number;
  scrollRef: React.RefObject<HTMLDivElement | null>;
  focusedRowId: string | null;
  onRowFocus: (rowId: string) => void;
}

function VirtualHistoryRow({
  row,
  start,
  scrollRef,
  focusedRowId,
  onRowFocus,
}: VirtualHistoryRowProps) {
  const rowRef = useRef<HTMLTableRowElement>(null);

  useEffect(() => {
    if (focusedRowId !== row.id) return;
    const activeElement = document.activeElement;
    if (activeElement === document.body || activeElement === scrollRef.current) {
      rowRef.current?.focus({ preventScroll: true });
    }
  }, [focusedRowId, row.id, scrollRef]);

  return (
    <tr
      ref={rowRef}
      data-history-row
      data-status={row.original.status}
      data-selected={row.getIsSelected() ? "true" : undefined}
      tabIndex={0}
      className="absolute flex w-full border-b border-border/50 bg-background text-sm outline-none hover:bg-muted/50 focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/70 data-[selected=true]:bg-primary/8"
      style={{ height: `${ROW_HEIGHT}px`, transform: `translateY(${start}px)` }}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onRowFocus(row.id);
      }}
      onClick={() => row.toggleSelected()}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          row.toggleSelected();
        }
      }}
    >
      {row.getVisibleCells().map((cell) => (
        <td
          key={cell.id}
          className={cn("flex min-w-0 shrink-0 items-center px-2 py-1", cellAlignment(cell.column))}
          style={{ width: `${cell.column.getSize()}px` }}
        >
          {flexRender(cell.column.columnDef.cell, cell.getContext())}
        </td>
      ))}
    </tr>
  );
}

function VirtualBody({ rows, scrollRef, focusedRowId, onRowFocus }: VirtualBodyProps) {
  const rowVirtualizer = useVirtualizer<HTMLDivElement, HTMLTableRowElement>({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    // The first render can precede ResizeObserver (including in a freshly
    // activated retained view). Seed a desktop-sized window, then let the
    // real scroll element measurement replace it.
    initialRect: { width: 0, height: 520 },
    getItemKey: (index) => rows[index]?.id ?? index,
    overscan: OVERSCAN_ROWS,
  });
  return (
    <tbody className="relative grid" style={{ height: `${rowVirtualizer.getTotalSize()}px` }}>
      {rowVirtualizer.getVirtualItems().map((virtualRow) => {
        const row = rows[virtualRow.index];
        if (row === undefined) return null;
        return (
          <VirtualHistoryRow
            key={row.id}
            row={row}
            start={virtualRow.start}
            scrollRef={scrollRef}
            focusedRowId={focusedRowId}
            onRowFocus={onRowFocus}
          />
        );
      })}
    </tbody>
  );
}

export function HistoryTable({ rows: projectedRows }: { rows: readonly HistoryDisplayRow[] }) {
  const data = useMemo(() => [...projectedRows].sort(compareHistoryDefault), [projectedRows]);
  const tableColumns = useMemo(columns, []);
  const [sorting, setSorting] = useState<SortingState>([{ id: "date", desc: true }]);
  const [globalFilter, setGlobalFilter] = useState("");
  const [columnFilters, setColumnFilters] = useState<ColumnFiltersState>([]);
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const [focusedRowId, setFocusedRowId] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const table = useReactTable({
    data,
    columns: tableColumns,
    getRowId: (row) => row.id,
    state: { sorting, globalFilter, columnFilters, rowSelection },
    onSortingChange: setSorting,
    onGlobalFilterChange: setGlobalFilter,
    onColumnFiltersChange: setColumnFilters,
    onRowSelectionChange: setRowSelection,
    enableMultiRowSelection: false,
    globalFilterFn: (row, _columnId, query: string) =>
      row.original.label.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
    getCoreRowModel: getCoreRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });
  const filteredRows = table.getRowModel().rows;
  const totals = useMemo(
    () => historyTotals(filteredRows.map((row) => row.original)),
    [filteredRows],
  );
  const statusCounts = useMemo(() => {
    const counts = Object.fromEntries(HISTORY_STATUSES.map((status) => [status, 0])) as Record<
      HistoryStatusLabel,
      number
    >;
    for (const row of data) counts[row.status] += 1;
    return counts;
  }, [data]);
  const activeStatus = (table.getColumn("status")?.getFilterValue() as HistoryStatusLabel) ?? null;
  const setStatus = useCallback(
    (status: HistoryStatusLabel | null) => table.getColumn("status")?.setFilterValue(status),
    [table],
  );

  return (
    <div className="flex h-full min-h-0 flex-col gap-3 p-4">
      <header className="flex shrink-0 items-end justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold">History</h1>
          <p className="text-sm text-muted-foreground">
            Historical outcomes and recorded facts for observed and imported files.
          </p>
        </div>
        <span className="text-xs text-muted-foreground tabular-nums">
          {projectedRows.length.toLocaleString()} records
        </span>
      </header>

      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2">
        <label className="relative w-64 max-w-full">
          <span className="sr-only">Search History</span>
          <Search
            className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <Input
            value={globalFilter}
            onChange={(event) => setGlobalFilter(event.target.value)}
            placeholder="Search files…"
            className="h-7 pl-7"
          />
        </label>
        <div className="flex max-w-full flex-wrap items-center gap-1" aria-label="History outcomes">
          <Button
            size="sm"
            variant={activeStatus === null ? "secondary" : "ghost"}
            aria-pressed={activeStatus === null}
            onClick={() => setStatus(null)}
          >
            All · {data.length}
          </Button>
          {HISTORY_STATUSES.map((status) => (
            <Button
              key={status}
              size="sm"
              variant={activeStatus === status ? "secondary" : "ghost"}
              aria-pressed={activeStatus === status}
              onClick={() => setStatus(activeStatus === status ? null : status)}
            >
              {status} · {statusCounts[status]}
            </Button>
          ))}
        </div>
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-md border border-border">
        <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto" data-history-scroll>
          <table className="grid min-w-full text-sm" style={{ width: `${table.getTotalSize()}px` }}>
            <thead className="sticky top-0 z-20 grid border-b border-border bg-surface">
              {table.getHeaderGroups().map((headerGroup) => (
                <tr key={headerGroup.id} className="flex h-8 w-full">
                  {headerGroup.headers.map((header) => (
                    <th
                      key={header.id}
                      scope="col"
                      className={cn(
                        "flex min-w-0 shrink-0 items-center px-2 text-xs font-medium text-muted-foreground",
                        cellAlignment(header.column),
                      )}
                      style={{ width: `${header.getSize()}px` }}
                      aria-sort={
                        header.column.getIsSorted() === "asc"
                          ? "ascending"
                          : header.column.getIsSorted() === "desc"
                            ? "descending"
                            : undefined
                      }
                    >
                      {header.isPlaceholder
                        ? null
                        : flexRender(header.column.columnDef.header, header.getContext())}
                    </th>
                  ))}
                </tr>
              ))}
            </thead>
            <VirtualBody
              rows={filteredRows}
              scrollRef={scrollRef}
              focusedRowId={focusedRowId}
              onRowFocus={setFocusedRowId}
            />
          </table>
          {filteredRows.length === 0 && (
            <div className="flex h-32 items-center justify-center text-sm text-muted-foreground">
              No matching records
            </div>
          )}
        </div>
        <footer className="flex min-h-9 shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-surface px-3 py-1 text-xs">
          <span className="font-medium tabular-nums">{totals.records.toLocaleString()} shown</span>
          {totals.sizedRecords > 0 ? (
            <>
              <span className="text-muted-foreground tabular-nums">
                Before {formatFileSize(totals.inputBytes)}
              </span>
              <span className="text-muted-foreground tabular-nums">
                After {formatFileSize(totals.outputBytes)}
              </span>
              <span
                className={cn(
                  "font-medium tabular-nums",
                  totals.savedBytes > 0
                    ? "text-success"
                    : totals.savedBytes < 0
                      ? "text-destructive"
                      : "text-muted-foreground",
                )}
              >
                {formatSignedSavedBytes(totals.savedBytes)}
              </span>
            </>
          ) : (
            <span className="text-muted-foreground">No complete size pairs</span>
          )}
        </footer>
      </div>
    </div>
  );
}
