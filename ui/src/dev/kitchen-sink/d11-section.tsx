import { CircleAlert, CircleCheck, CircleSlash } from "lucide-react";

import { formatCompactTime, formatEfficiency, formatFileSize } from "@/lib/format/format";

import { Section, ThemePair } from "./theme-pair";

/**
 * D11 presentation mockups, rendered with the real formatters: the status
 * column (text color/weight + at most one small icon, no pill chips) and the
 * three-state estimate-confidence treatment replacing "~"/"~~" jargon.
 */

const GIB = 1024 ** 3;

function StatusRow({
  name,
  status,
  detail,
}: {
  name: string;
  status: React.ReactNode;
  detail: string;
}) {
  return (
    <div className="grid grid-cols-[1fr_10rem_8rem] items-center gap-2 border-b border-border py-1 text-sm last:border-b-0">
      <span className="truncate">{name}</span>
      {status}
      <span className="text-right text-muted-foreground tabular-nums">{detail}</span>
    </div>
  );
}

export function D11Section() {
  return (
    <>
      <Section title="Status column (no chips, one icon max)">
        <ThemePair>
          <div>
            <StatusRow
              name="vacation-2019.mp4"
              status={
                <span className="flex items-center gap-1.5 text-success">
                  <CircleCheck className="size-3.5" aria-hidden="true" />
                  Done
                </span>
              }
              detail={formatFileSize(1.4 * GIB)}
            />
            <StatusRow
              name="concert-4k.mkv"
              status={<span className="text-muted-foreground">Converting… 62%</span>}
              detail={formatCompactTime(4260, "high")}
            />
            <StatusRow
              name="old-drama.avi"
              status={
                <span className="flex items-center gap-1.5 text-warning">
                  <CircleSlash className="size-3.5" aria-hidden="true" />
                  Skipped — not worthwhile
                </span>
              }
              detail={formatFileSize(0.7 * GIB)}
            />
            <StatusRow
              name="broken-file.wmv"
              status={
                <span className="flex items-center gap-1.5 text-destructive">
                  <CircleAlert className="size-3.5" aria-hidden="true" />
                  Error — input unreadable
                </span>
              }
              detail="—"
            />
            <p className="pt-2 text-xs text-muted-foreground">
              Folder: 3 succeeded · 1 skipped · 1 failed
            </p>
          </div>
        </ThemePair>
      </Section>

      <Section title="Estimate confidence (exact / estimate / rough)">
        <ThemePair>
          <div className="flex flex-col gap-1 text-sm">
            <div className="grid grid-cols-[8rem_1fr] gap-2">
              <span className="text-muted-foreground">exact</span>
              <span className="tabular-nums">{formatFileSize(1.23 * GIB)} saved</span>
            </div>
            <div className="grid grid-cols-[8rem_1fr] gap-2">
              <span className="text-muted-foreground">estimate</span>
              <span className="text-muted-foreground tabular-nums">
                {formatFileSize(1.23 * GIB)} saved
              </span>
            </div>
            <div className="grid grid-cols-[8rem_1fr] gap-2">
              <span className="text-muted-foreground">rough</span>
              <span className="text-muted-foreground/60 tabular-nums">
                {formatFileSize(1.23 * GIB)} saved
              </span>
            </div>
            <p className="pt-2 text-xs text-muted-foreground">
              Candidate: muted-color ramp; efficiency column example —{" "}
              {formatEfficiency(2.5 * GIB, 3600)}
            </p>
          </div>
        </ThemePair>
      </Section>
    </>
  );
}
