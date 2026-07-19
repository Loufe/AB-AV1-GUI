import { formatCompactTime, formatFileSize } from "@/lib/format/format";

import { Section, ThemePair } from "./theme-pair";

/**
 * Density comparison for the remaining #36 open question: the same queue
 * rows at three padding/type candidates. Pick from rendered evidence.
 */

const GIB = 1024 ** 3;

const ROWS = [
  ["movie-night.mkv", "H264 / AAC", formatFileSize(4.2 * GIB), formatCompactTime(8100, "high")],
  ["family-dinner.mp4", "HEVC / AC3", formatFileSize(2.1 * GIB), formatCompactTime(3900, "medium")],
  ["screencast.webm", "VP9 / OPUS", formatFileSize(0.4 * GIB), formatCompactTime(50, "low")],
] as const;

const CANDIDATES = [
  { name: "A — py-0.5 / text-sm", row: "py-0.5", text: "text-sm" },
  { name: "B — py-1 / text-sm", row: "py-1", text: "text-sm" },
  { name: "C — py-1.5 / text-base", row: "py-1.5", text: "text-base" },
] as const;

export function DensitySection() {
  return (
    <Section title="Density candidates">
      <ThemePair>
        <div className="flex flex-col gap-4">
          {CANDIDATES.map((candidate) => (
            <div key={candidate.name}>
              <p className="pb-1 text-xs text-muted-foreground">{candidate.name}</p>
              <div className="rounded-md border border-border bg-surface">
                {ROWS.map(([name, fmt, size, time]) => (
                  <div
                    key={name}
                    className={`grid grid-cols-[1fr_8rem_6rem_6rem] items-center gap-2 border-b border-border px-2.5 last:border-b-0 ${candidate.row} ${candidate.text}`}
                  >
                    <span className="truncate">{name}</span>
                    <span className="text-muted-foreground">{fmt}</span>
                    <span className="text-right tabular-nums">{size}</span>
                    <span className="text-right text-muted-foreground tabular-nums">{time}</span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </ThemePair>
    </Section>
  );
}
