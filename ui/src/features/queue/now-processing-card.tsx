import { FileVideo } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle, Progress } from "@/components/ui";

function LabeledBar({ label, pct }: { label: string; pct: number }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-14 shrink-0 text-xs text-muted-foreground">{label}</span>
      <Progress value={pct} className="flex-1" />
      <span className="w-9 shrink-0 text-right text-xs tabular-nums">{pct}%</span>
    </div>
  );
}

/**
 * The active item's detail card. `detail` is one prose line of facts
 * (VMAF/CRF/preset/output size/elapsed/ETA) assembled by the wiring layer —
 * whatever subset is actually known.
 */
export function NowProcessingCard({
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
          {name}
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
