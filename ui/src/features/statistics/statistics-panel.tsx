import {
  Clock,
  Eye,
  FileCheck2,
  Gauge,
  HardDrive,
  SearchCheck,
  SlidersHorizontal,
  TrendingDown,
} from "lucide-react";
import { useId, type ComponentType } from "react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  LabelList,
  ReferenceLine,
  XAxis,
  YAxis,
} from "recharts";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import type { StatisticsPayload, ValueSpread } from "@/lib/bindings";
import {
  formatDurationMsCompact,
  formatStatisticsCrf,
  formatStatisticsInputThroughput,
  formatStatisticsReduction,
  formatStatisticsVmaf,
} from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";

import {
  codecRows,
  coverageMessage,
  cumulativeRows,
  formatEpochDay,
  formatSignedFileSize,
  reductionRows,
  runOutcomeRows,
} from "./statistics-display";

const SAVINGS_CONFIG = {
  savedBytes: { label: "Cumulative savings", color: "var(--chart-1)" },
} satisfies ChartConfig;

const FILES_CONFIG = {
  files: { label: "Files", color: "var(--chart-1)" },
} satisfies ChartConfig;

interface Metric {
  label: string;
  value: string;
  description: string;
  icon: ComponentType<{ className?: string }>;
}

function StatStrip({ payload }: { payload: StatisticsPayload }) {
  const metrics: Metric[] = [
    {
      label: "Conversion net savings",
      value: formatSignedFileSize(payload.total_saved_bytes),
      description: `${payload.sized_converted_files.toLocaleString()} converted files with both sizes`,
      icon: HardDrive,
    },
    {
      label: "Converted standings",
      value: payload.converted_files.toLocaleString(),
      description: "Current per-content verdicts",
      icon: FileCheck2,
    },
    {
      label: "Processing time",
      value: formatDurationMsCompact(payload.total_time_ms),
      description: "Analysis plus encoding",
      icon: Clock,
    },
  ];

  if (payload.gigabytes_per_hour !== null) {
    metrics.push({
      label: "Input throughput",
      value: formatStatisticsInputThroughput(payload.gigabytes_per_hour),
      description: "Input GiB per processing hour",
      icon: Gauge,
    });
  }
  if (payload.reduction_percent !== null) {
    metrics.push({
      label: "Average reduction",
      value: formatStatisticsReduction(payload.reduction_percent.average),
      description: `${payload.reduction_percent.count.toLocaleString()} sized conversions`,
      icon: TrendingDown,
    });
  }
  if (payload.vmaf !== null) {
    metrics.push({
      label: "Average VMAF",
      value: formatStatisticsVmaf(payload.vmaf.average),
      description: `${payload.vmaf.count.toLocaleString()} converted measurements`,
      icon: Eye,
    });
  }
  if (payload.crf !== null) {
    metrics.push({
      label: "Average CRF",
      value: formatStatisticsCrf(payload.crf.average),
      description: `${payload.crf.count.toLocaleString()} converted measurements`,
      icon: SlidersHorizontal,
    });
  }

  return (
    <dl className="grid overflow-hidden rounded-lg border border-border bg-card sm:grid-cols-2 xl:grid-cols-4">
      {metrics.map(({ label, value, description, icon: Icon }) => (
        <div
          key={label}
          className="flex min-w-0 flex-col gap-1 border-b border-border px-4 py-3 last:border-b-0 sm:border-r sm:nth-[2n]:border-r-0 xl:nth-[2n]:border-r xl:nth-[4n]:border-r-0"
        >
          <dt className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Icon className="size-3.5 shrink-0" aria-hidden="true" />
            <span>{label}</span>
          </dt>
          <dd className="text-xl font-medium tabular-nums">{value}</dd>
          <dd className="text-xs text-muted-foreground">{description}</dd>
        </div>
      ))}
    </dl>
  );
}

function TooltipRow({ name, value }: { name: string; value: string }) {
  return (
    <div className="flex w-full items-center justify-between gap-4">
      <span className="text-muted-foreground">{name}</span>
      <span className="font-mono font-medium text-foreground tabular-nums">{value}</span>
    </div>
  );
}

function CumulativeSavingsCard({ payload }: { payload: StatisticsPayload }) {
  const gradientId = useId().replace(/:/g, "");
  const rows = cumulativeRows(payload.cumulative_savings);

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>Cumulative conversion savings</h2>
        </CardTitle>
        <CardDescription>
          Daily local-calendar totals; the line can fall when outputs grow
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {rows.length === 0 ? (
          <p className="py-12 text-center text-sm text-muted-foreground">
            No sized conversion dates are available.
          </p>
        ) : (
          <ChartContainer
            config={SAVINGS_CONFIG}
            className="aspect-auto h-64 w-full"
            aria-label="Daily cumulative conversion savings chart"
          >
            <AreaChart data={rows} accessibilityLayer margin={{ left: 4, right: 12, top: 8 }}>
              <defs>
                <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="5%" stopColor="var(--color-savedBytes)" stopOpacity={0.5} />
                  <stop offset="95%" stopColor="var(--color-savedBytes)" stopOpacity={0.04} />
                </linearGradient>
              </defs>
              <CartesianGrid vertical={false} />
              <XAxis
                dataKey="date"
                tickLine={false}
                axisLine={false}
                tickMargin={8}
                minTickGap={32}
              />
              <YAxis
                tickLine={false}
                axisLine={false}
                tickMargin={4}
                width={72}
                tickFormatter={(value: number) => formatSignedFileSize(value)}
              />
              <ReferenceLine y={0} stroke="var(--border)" />
              <ChartTooltip
                cursor
                isAnimationActive={false}
                content={
                  <ChartTooltipContent
                    indicator="line"
                    formatter={(value) => (
                      <TooltipRow name="Net savings" value={formatSignedFileSize(Number(value))} />
                    )}
                  />
                }
              />
              <Area
                dataKey="savedBytes"
                type="linear"
                baseValue={0}
                fill={`url(#${gradientId})`}
                stroke="var(--color-savedBytes)"
                strokeWidth={2}
                isAnimationActive={false}
              />
            </AreaChart>
          </ChartContainer>
        )}
        {rows.length > 0 && (
          <details className="selectable text-xs text-muted-foreground">
            <summary className="w-fit cursor-pointer text-foreground">View daily values</summary>
            <table className="mt-2 w-full max-w-sm border-collapse text-left">
              <caption className="sr-only">Daily cumulative conversion savings</caption>
              <thead>
                <tr className="border-b border-border">
                  <th className="py-1 font-medium">Local date</th>
                  <th className="py-1 text-right font-medium">Net savings</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={row.date} className="border-b border-border/50 last:border-0">
                    <td className="py-1">{row.date}</td>
                    <td className="py-1 text-right tabular-nums">
                      {formatSignedFileSize(row.savedBytes)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
        )}
      </CardContent>
    </Card>
  );
}

function ReductionHistogramCard({ payload }: { payload: StatisticsPayload }) {
  const rows = reductionRows(payload.reduction_bins);
  const hasBins = rows.some(({ files }) => files > 0);

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>Size reduction</h2>
        </CardTitle>
        <CardDescription>
          Non-negative reductions by 10% band; grew files are reported separately
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {hasBins ? (
          <ChartContainer
            config={FILES_CONFIG}
            className="aspect-auto h-56 w-full"
            aria-label="Converted files by non-negative size reduction band"
          >
            <BarChart data={rows} accessibilityLayer margin={{ left: 4, right: 4, top: 8 }}>
              <CartesianGrid vertical={false} />
              <XAxis dataKey="label" tickLine={false} axisLine={false} tickMargin={8} />
              <YAxis tickLine={false} axisLine={false} tickMargin={4} width={36} />
              <ChartTooltip
                cursor
                isAnimationActive={false}
                content={
                  <ChartTooltipContent
                    labelFormatter={(label) => `${String(label)} smaller`}
                    formatter={(value) => (
                      <TooltipRow name="Files" value={Number(value).toLocaleString()} />
                    )}
                  />
                }
              />
              <Bar dataKey="files" fill="var(--color-files)" radius={3} isAnimationActive={false} />
            </BarChart>
          </ChartContainer>
        ) : (
          <p className="py-10 text-center text-sm text-muted-foreground">
            No non-negative reduction measurements are available.
          </p>
        )}
        <p className="text-xs text-muted-foreground">
          <span className="font-medium text-foreground tabular-nums">
            {payload.grew_count.toLocaleString()}
          </span>{" "}
          converted outputs grew and are not included in these bins.
        </p>
        {rows.length > 0 && (
          <details className="selectable text-xs text-muted-foreground">
            <summary className="w-fit cursor-pointer text-foreground">
              View reduction counts
            </summary>
            <ul className="mt-2 grid grid-cols-2 gap-x-6 gap-y-1 sm:grid-cols-5">
              {rows.map(({ label, files }) => (
                <li key={label} className="flex justify-between gap-2">
                  <span>{label}</span>
                  <span className="tabular-nums">{files.toLocaleString()}</span>
                </li>
              ))}
            </ul>
          </details>
        )}
      </CardContent>
    </Card>
  );
}

function CodecBreakdownCard({ payload }: { payload: StatisticsPayload }) {
  const rows = codecRows(payload.codecs);
  const height = Math.max(176, rows.length * 32 + 24);

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>Source codecs</h2>
        </CardTitle>
        <CardDescription>Every codec reported for converted standings</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {rows.length === 0 ? (
          <p className="py-10 text-center text-sm text-muted-foreground">
            No converted codec facts are available.
          </p>
        ) : (
          <ChartContainer
            config={FILES_CONFIG}
            className="aspect-auto w-full"
            style={{ height }}
            aria-label="Converted files by source codec"
          >
            <BarChart
              data={rows}
              layout="vertical"
              accessibilityLayer
              margin={{ left: 4, right: 48, top: 8 }}
            >
              <XAxis type="number" hide />
              <YAxis
                dataKey="label"
                type="category"
                tickLine={false}
                axisLine={false}
                tickMargin={4}
                width={88}
              />
              <ChartTooltip
                cursor
                isAnimationActive={false}
                content={
                  <ChartTooltipContent
                    formatter={(value) => (
                      <TooltipRow name="Files" value={Number(value).toLocaleString()} />
                    )}
                  />
                }
              />
              <Bar dataKey="files" fill="var(--color-files)" radius={3} isAnimationActive={false}>
                <LabelList
                  dataKey="files"
                  position="right"
                  className="fill-foreground"
                  fontSize={11}
                  formatter={(value) => Number(value).toLocaleString()}
                />
              </Bar>
            </BarChart>
          </ChartContainer>
        )}
        {rows.length > 0 && (
          <details className="selectable text-xs text-muted-foreground">
            <summary className="w-fit cursor-pointer text-foreground">View codec counts</summary>
            <ul className="mt-2 grid max-w-sm grid-cols-2 gap-x-6 gap-y-1">
              {rows.map(({ label, files }, index) => (
                <li key={`${label}-${index}`} className="flex justify-between gap-2">
                  <span>{label}</span>
                  <span className="tabular-nums">{files.toLocaleString()}</span>
                </li>
              ))}
            </ul>
          </details>
        )}
      </CardContent>
    </Card>
  );
}

function SpreadRow({
  label,
  spread,
  format,
}: {
  label: string;
  spread: ValueSpread;
  format: (value: number | null) => string;
}) {
  return (
    <div className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-0.5">
      <dt>{label}</dt>
      <dd className="text-right font-medium text-foreground tabular-nums">
        {format(spread.average)} average
      </dd>
      <dd className="col-span-2 text-xs">
        {format(spread.minimum)} to {format(spread.maximum)} across {spread.count.toLocaleString()}{" "}
        samples
      </dd>
    </div>
  );
}

function OutcomeDetailsCard({ payload }: { payload: StatisticsPayload }) {
  const runRows = runOutcomeRows(payload.runs);

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>Outcomes and coverage</h2>
        </CardTitle>
        <CardDescription>
          Current per-content standings and terminal run outcomes are separate populations
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-6 lg:grid-cols-3">
        <section aria-labelledby="statistics-standings-heading">
          <h3 id="statistics-standings-heading" className="mb-2 font-medium">
            Current standings
          </h3>
          <dl className="grid gap-1 text-sm text-muted-foreground">
            <div className="flex justify-between gap-4">
              <dt>Converted</dt>
              <dd className="text-foreground tabular-nums">
                {payload.converted_files.toLocaleString()}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Converted with both sizes</dt>
              <dd className="text-foreground tabular-nums">
                {payload.sized_converted_files.toLocaleString()}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Remuxed</dt>
              <dd className="text-foreground tabular-nums">
                {payload.remuxed_files.toLocaleString()}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Not worthwhile</dt>
              <dd className="text-foreground tabular-nums">
                {payload.not_worthwhile_files.toLocaleString()}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Converted outputs that grew</dt>
              <dd className="text-foreground tabular-nums">
                {payload.grew_count.toLocaleString()}
              </dd>
            </div>
          </dl>
        </section>

        <section aria-labelledby="statistics-savings-heading">
          <h3 id="statistics-savings-heading" className="mb-2 font-medium">
            Size facts
          </h3>
          <dl className="grid gap-1 text-sm text-muted-foreground">
            <div className="flex justify-between gap-4">
              <dt>Conversion input</dt>
              <dd className="text-foreground tabular-nums">
                {formatFileSize(payload.total_input_bytes)}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Conversion output</dt>
              <dd className="text-foreground tabular-nums">
                {formatFileSize(payload.total_output_bytes)}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Conversion net savings</dt>
              <dd className="text-foreground tabular-nums">
                {formatSignedFileSize(payload.total_saved_bytes)}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Remux savings</dt>
              <dd className="text-foreground tabular-nums">
                {formatSignedFileSize(payload.remux_saved_bytes)}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt>Local date range</dt>
              <dd className="text-right text-foreground tabular-nums">
                {payload.first_epoch_day === null ? "—" : formatEpochDay(payload.first_epoch_day)}
                {" – "}
                {payload.last_epoch_day === null ? "—" : formatEpochDay(payload.last_epoch_day)}
              </dd>
            </div>
          </dl>
        </section>

        <section aria-labelledby="statistics-runs-heading">
          <h3 id="statistics-runs-heading" className="mb-2 font-medium">
            Terminal runs
          </h3>
          <dl className="grid gap-1 text-sm text-muted-foreground">
            {runRows.map(({ label, count }) => (
              <div key={label} className="flex justify-between gap-4">
                <dt>{label}</dt>
                <dd className="text-foreground tabular-nums">{count.toLocaleString()}</dd>
              </div>
            ))}
          </dl>
        </section>

        {(payload.reduction_percent !== null || payload.vmaf !== null || payload.crf !== null) && (
          <section className="lg:col-span-3" aria-labelledby="statistics-measurements-heading">
            <h3 id="statistics-measurements-heading" className="mb-2 font-medium">
              Converted measurements
            </h3>
            <dl className="grid gap-3 text-sm text-muted-foreground md:grid-cols-3">
              {payload.reduction_percent !== null && (
                <SpreadRow
                  label="Size reduction"
                  spread={payload.reduction_percent}
                  format={formatStatisticsReduction}
                />
              )}
              {payload.vmaf !== null && (
                <SpreadRow label="VMAF" spread={payload.vmaf} format={formatStatisticsVmaf} />
              )}
              {payload.crf !== null && (
                <SpreadRow label="CRF" spread={payload.crf} format={formatStatisticsCrf} />
              )}
            </dl>
          </section>
        )}
      </CardContent>
    </Card>
  );
}

export function StatisticsPanel({ payload }: { payload: StatisticsPayload }) {
  const coverage = coverageMessage(payload);

  return (
    <div className="flex flex-col gap-4">
      <StatStrip payload={payload} />
      {coverage !== null && (
        <p className="flex items-start gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
          <SearchCheck className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>{coverage}</span>
        </p>
      )}
      <CumulativeSavingsCard payload={payload} />
      <div className="grid gap-4 xl:grid-cols-2">
        <ReductionHistogramCard payload={payload} />
        <CodecBreakdownCard payload={payload} />
      </div>
      <OutcomeDetailsCard payload={payload} />
    </div>
  );
}
