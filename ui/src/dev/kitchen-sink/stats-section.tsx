import { Clock, Eye, FileCheck2, Gauge, HardDrive, TrendingDown } from "lucide-react";
import { useId } from "react";
import { Area, AreaChart, Bar, BarChart, CartesianGrid, LabelList, XAxis, YAxis } from "recharts";

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui";
import {
  formatDurationMsCompact,
  formatStatisticsInputThroughput,
  formatStatisticsReduction,
  formatStatisticsVmaf,
} from "@/lib/format/engine-values";
import { formatFileSize } from "@/lib/format/format";

import { Section } from "./theme-pair";

/**
 * Statistics view mockup (#36 D7) — hardcoded data, judged on localhost like
 * the drag spike; the verdict becomes a D7 amendment on #36.
 *
 * Design decisions under review:
 * - Layout inverts the V2 tab: metric cards first, hero time-series,
 *   secondary breakdowns last (Plausible/Umami pattern).
 * - Source codecs render as horizontal bars, not the V2 pie (NN/g pie
 *   critique; recharts#6338 keyboard-tooltip bug; direct labels kill the
 *   legend).
 * - Tooltips are supplementary precision only — axes carry coarse values.
 *   Recharts v3's accessibilityLayer (default-on) drives them from the
 *   keyboard, satisfying D8's no-hover-only rule.
 * - No entrance/count-up animation (D8): isAnimationActive is off everywhere.
 */

const GIB = 1024 ** 3;

// Statistics values are backend-side aggregates. Unlike History facts, its
// spreads are already normalized human-scale floats.
const SUMMARY = {
  converted_files: 1284,
  total_saved_bytes: 412.4 * GIB,
  reduction_percent: { average: 54, minimum: -2, maximum: 91, count: 1284 },
  total_time_ms: (214 * 3600 + 36 * 60) * 1000,
  gigabytes_per_hour: 1.92,
  vmaf: { average: 95.4, minimum: 90.1, maximum: 99.2, count: 1284 },
};

/** Cumulative space saved, monthly points (bytes). */
const CUMULATIVE = [
  { month: "Jun ’25", saved: 18 * GIB },
  { month: "Jul ’25", saved: 41 * GIB },
  { month: "Aug ’25", saved: 77 * GIB },
  { month: "Sep ’25", saved: 112 * GIB },
  { month: "Oct ’25", saved: 138 * GIB },
  { month: "Nov ’25", saved: 171 * GIB },
  { month: "Dec ’25", saved: 214 * GIB },
  { month: "Jan ’26", saved: 232 * GIB },
  { month: "Feb ’26", saved: 268 * GIB },
  { month: "Mar ’26", saved: 301 * GIB },
  { month: "Apr ’26", saved: 342 * GIB },
  { month: "May ’26", saved: 371 * GIB },
  { month: "Jun ’26", saved: 396 * GIB },
  { month: "Jul ’26", saved: SUMMARY.total_saved_bytes },
];

/** Size-reduction histogram, 10% bins (sums to totalFiles). */
const REDUCTION_BINS = [
  { bin: "0–10%", files: 3 },
  { bin: "10–20%", files: 9 },
  { bin: "20–30%", files: 24 },
  { bin: "30–40%", files: 78 },
  { bin: "40–50%", files: 154 },
  { bin: "50–60%", files: 236 },
  { bin: "60–70%", files: 310 },
  { bin: "70–80%", files: 262 },
  { bin: "80–90%", files: 158 },
  { bin: "90–100%", files: 50 },
];

/** Source codec counts (sums to totalFiles). */
const CODECS = [
  { codec: "H.264", files: 812 },
  { codec: "HEVC", files: 236 },
  { codec: "MPEG-4", files: 118 },
  { codec: "VC-1", files: 74 },
  { codec: "Other", files: 44 },
];

const SAVED_CONFIG = {
  saved: { label: "Space saved", color: "var(--chart-1)" },
} satisfies ChartConfig;

const FILES_CONFIG = {
  files: { label: "Files", color: "var(--chart-1)" },
} satisfies ChartConfig;

interface Metric {
  label: string;
  value: string;
  icon: React.ComponentType<{ className?: string }>;
}

/**
 * Summary metrics as one stat strip — a single card with divider-separated
 * cells instead of six identical boxes (denser, no repeated chrome).
 */
function StatStrip({ metrics }: { metrics: Metric[] }) {
  return (
    <Card className="grid grid-cols-6 gap-0 divide-x divide-border py-0">
      {metrics.map(({ label, value, icon: Icon }) => (
        <div key={label} className="flex flex-col gap-1 px-4 py-3">
          <div className="flex items-center gap-1.5 text-muted-foreground">
            <Icon className="size-3.5" aria-hidden="true" />
            <span className="truncate text-xs">{label}</span>
          </div>
          <span className="text-xl font-medium tabular-nums">{value}</span>
        </div>
      ))}
    </Card>
  );
}

/** Tooltip value cell: name left, precise value right (house tooltip row). */
function TooltipRow({ name, value }: { name: string; value: string }) {
  return (
    <div className="flex w-full items-center justify-between gap-4">
      <span className="text-muted-foreground">{name}</span>
      <span className="font-mono font-medium text-foreground tabular-nums">{value}</span>
    </div>
  );
}

function CumulativeSavingsCard() {
  // Per-instance gradient id: the section renders twice (light + dark) and
  // SVG ids are document-global, so a fixed id would leak the light gradient
  // into the dark panel.
  const gradientId = useId();
  return (
    <Card>
      <CardHeader>
        <CardTitle>Cumulative space saved</CardTitle>
        <CardDescription>All conversions since first use</CardDescription>
      </CardHeader>
      <CardContent>
        <ChartContainer config={SAVED_CONFIG} className="aspect-auto h-64 w-full">
          <AreaChart data={CUMULATIVE} margin={{ left: 4, right: 12, top: 8 }}>
            <defs>
              <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="var(--color-saved)" stopOpacity={0.5} />
                <stop offset="95%" stopColor="var(--color-saved)" stopOpacity={0.04} />
              </linearGradient>
            </defs>
            <CartesianGrid vertical={false} />
            <XAxis dataKey="month" tickLine={false} axisLine={false} tickMargin={8} />
            <YAxis
              tickLine={false}
              axisLine={false}
              tickMargin={4}
              width={56}
              tickFormatter={(v: number) => `${Math.round(v / GIB)} GB`}
            />
            <ChartTooltip
              cursor
              content={
                <ChartTooltipContent
                  indicator="line"
                  formatter={(value) => (
                    <TooltipRow name="Space saved" value={formatFileSize(Number(value))} />
                  )}
                />
              }
            />
            <Area
              dataKey="saved"
              type="monotone"
              fill={`url(#${gradientId})`}
              stroke="var(--color-saved)"
              strokeWidth={2}
              isAnimationActive={false}
            />
          </AreaChart>
        </ChartContainer>
      </CardContent>
    </Card>
  );
}

function ReductionHistogramCard() {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Size reduction</CardTitle>
        <CardDescription>Files by reduction achieved</CardDescription>
      </CardHeader>
      <CardContent>
        <ChartContainer config={FILES_CONFIG} className="aspect-auto h-56 w-full">
          <BarChart data={REDUCTION_BINS} margin={{ left: 4, right: 4, top: 8 }}>
            <CartesianGrid vertical={false} />
            <XAxis dataKey="bin" tickLine={false} axisLine={false} tickMargin={8} />
            <YAxis tickLine={false} axisLine={false} tickMargin={4} width={36} />
            <ChartTooltip
              cursor
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
      </CardContent>
    </Card>
  );
}

function CodecBreakdownCard() {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Source codecs</CardTitle>
        <CardDescription>What the converted files started as</CardDescription>
      </CardHeader>
      <CardContent>
        <ChartContainer config={FILES_CONFIG} className="aspect-auto h-56 w-full">
          <BarChart data={CODECS} layout="vertical" margin={{ left: 4, right: 40, top: 8 }}>
            <XAxis type="number" hide />
            <YAxis
              dataKey="codec"
              type="category"
              tickLine={false}
              axisLine={false}
              tickMargin={4}
              width={64}
            />
            <ChartTooltip
              cursor
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
                formatter={(v) => Number(v).toLocaleString()}
              />
            </Bar>
          </BarChart>
        </ChartContainer>
      </CardContent>
    </Card>
  );
}

function StatsPanel() {
  const metrics: Metric[] = [
    { label: "Space saved", value: formatFileSize(SUMMARY.total_saved_bytes), icon: HardDrive },
    {
      label: "Files converted",
      value: SUMMARY.converted_files.toLocaleString(),
      icon: FileCheck2,
    },
    {
      label: "Avg reduction",
      value: formatStatisticsReduction(SUMMARY.reduction_percent.average),
      icon: TrendingDown,
    },
    {
      label: "Throughput",
      value: formatStatisticsInputThroughput(SUMMARY.gigabytes_per_hour),
      icon: Gauge,
    },
    {
      label: "Processing time",
      value: formatDurationMsCompact(SUMMARY.total_time_ms),
      icon: Clock,
    },
    { label: "Avg VMAF", value: formatStatisticsVmaf(SUMMARY.vmaf.average), icon: Eye },
  ];
  return (
    <div className="flex flex-col gap-4">
      <StatStrip metrics={metrics} />
      <CumulativeSavingsCard />
      <div className="grid grid-cols-2 gap-4">
        <ReductionHistogramCard />
        <CodecBreakdownCard />
      </div>
    </div>
  );
}

export function StatsSection() {
  return (
    <Section title="Statistics view (D7 mockup)">
      <p className="text-sm text-muted-foreground">
        Metric cards → hero time-series → breakdowns; codecs as horizontal bars instead of the old
        pie. Hover any chart for the tooltip, or Tab to a chart and use arrow keys — the tooltip
        follows the keyboard too. Full-width light panel first, then dark (charts are too wide for
        the side-by-side pair).
      </p>
      <div className="rounded-lg border border-border bg-background p-4">
        <StatsPanel />
      </div>
      <div className="dark rounded-lg border border-border bg-background p-4 text-foreground">
        <StatsPanel />
      </div>
    </Section>
  );
}
