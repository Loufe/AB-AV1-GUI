import { FileVideo } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import {
  formatDurationMsCompact,
  formatEngineCrf,
  formatEngineVmafScore,
} from "@/lib/format/engine-values";
import { useProgressStore } from "@/lib/store/progress-store";

import { basename, deriveRowStatus } from "./queue-status";
import type { QueueRowData } from "./queue-status";

function LabeledBar({ label, pct }: { label: string; pct: number }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-14 shrink-0 text-xs text-muted-foreground">{label}</span>
      <Progress value={pct} aria-label={`${label} progress`} className="flex-1" />
      <span className="w-9 shrink-0 text-right text-xs tabular-nums">{pct}%</span>
    </div>
  );
}

/**
 * The active item's detail card. `detail` is one prose line of facts
 * (VMAF/CRF/preset/output size/elapsed/ETA) assembled by the wiring layer —
 * whatever subset is actually known.
 */
function NowProcessingCard({
  name,
  analyzePercent,
  encodePercent,
  detail,
}: {
  name: string;
  analyzePercent: number | null;
  encodePercent: number | null;
  detail: string | null;
}) {
  return (
    <Card size="sm" className="gap-2">
      <CardHeader className="gap-0.5">
        <CardTitle className="flex items-center gap-1.5 text-sm">
          <FileVideo className="size-4 text-muted-foreground" aria-hidden="true" />
          Now processing · {name}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-1.5">
        {analyzePercent !== null && <LabeledBar label="Analyze" pct={analyzePercent} />}
        {encodePercent !== null && <LabeledBar label="Encode" pct={encodePercent} />}
        {detail !== null && <p className="pt-1 text-xs text-muted-foreground">{detail}</p>}
      </CardContent>
    </Card>
  );
}

/** Live active-item surface with a per-RunId telemetry subscription. */
export function CurrentProcessingCard({ row }: { row: QueueRowData }) {
  const telemetry = useProgressStore((state) =>
    row.runId === null ? null : (state.telemetry[row.runId] ?? null),
  );
  const status = deriveRowStatus(row.item.state, telemetry, row.mediaDurationMs, null);
  const percent = status.kind === "working" ? status.percent : null;
  const analyzePercent =
    status.kind === "working" && status.phase === "Analyzing"
      ? percent
      : status.kind === "working" && row.crf !== null
        ? 100
        : null;
  const encodePercent =
    status.kind === "working" && (status.phase === "Encoding" || status.phase === "Remuxing")
      ? percent
      : null;
  const facts: string[] = [];
  if (row.vmaf !== null) facts.push(`VMAF ${formatEngineVmafScore(row.vmaf)}`);
  if (row.crf !== null) facts.push(`CRF ${formatEngineCrf(row.crf)}`);
  if (telemetry?.fps_centi !== null && telemetry?.fps_centi !== undefined) {
    facts.push(`${(telemetry.fps_centi / 100).toFixed(2)} fps`);
  }
  if (telemetry?.eta_ms !== null && telemetry?.eta_ms !== undefined) {
    facts.push(`ETA ${formatDurationMsCompact(telemetry.eta_ms)}`);
  }
  return (
    <NowProcessingCard
      name={basename(row.item.input)}
      analyzePercent={analyzePercent}
      encodePercent={encodePercent}
      detail={facts.length === 0 ? null : facts.join(" · ")}
    />
  );
}
