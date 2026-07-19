import { TriangleAlert } from "lucide-react";

import { Section, ThemePair } from "./theme-pair";

const COLOR_TOKENS = [
  "background",
  "surface",
  "elevated",
  "overlay",
  "muted",
  "border",
  "primary",
  "accent",
  "destructive",
  "success",
  "warning",
] as const;

const TYPE_RAMP = [
  ["text-xs", "11px"],
  ["text-sm", "12px"],
  ["text-base", "14px — body"],
  ["text-lg", "16px"],
  ["text-xl", "18px"],
  ["text-2xl", "20px"],
] as const;

const SPACING = [4, 8, 12, 16, 24, 32, 48] as const;

export function TokensSection() {
  return (
    <>
      <Section title="Color tokens">
        <ThemePair>
          <div className="grid grid-cols-4 gap-2">
            {COLOR_TOKENS.map((token) => (
              <div key={token} className="flex items-center gap-2">
                <div
                  className="size-8 shrink-0 rounded border border-border"
                  style={{ backgroundColor: `var(--${token})` }}
                />
                <span className="text-xs text-muted-foreground">{token}</span>
              </div>
            ))}
          </div>
        </ThemePair>
      </Section>

      <Section title="Elevation tier">
        <ThemePair>
          <div className="rounded-lg bg-background p-3">
            <span className="text-xs text-muted-foreground">background</span>
            <div className="mt-2 rounded-lg border border-border bg-surface p-3">
              <span className="text-xs text-muted-foreground">surface</span>
              <div className="mt-2 rounded-lg border border-border bg-elevated p-3">
                <span className="text-xs text-muted-foreground">elevated</span>
                <div className="mt-2 rounded-lg border border-border bg-overlay p-3">
                  <span className="text-xs text-muted-foreground">overlay</span>
                </div>
              </div>
            </div>
          </div>
        </ThemePair>
      </Section>

      <Section title="Primary vs warning adjacency">
        <ThemePair>
          <div className="flex items-center gap-3">
            <button
              type="button"
              className="rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground"
            >
              Start conversion
            </button>
            <span className="flex items-center gap-1.5 text-sm text-warning">
              <TriangleAlert className="size-4" aria-hidden="true" />
              VMAF target lowered to 93
            </span>
          </div>
        </ThemePair>
      </Section>

      <Section title="Type ramp">
        <ThemePair>
          <div className="flex flex-col gap-1">
            {TYPE_RAMP.map(([cls, label]) => (
              <p key={cls} className={cls}>
                {label} — Sphinx of black quartz, judge my vow
              </p>
            ))}
          </div>
        </ThemePair>
      </Section>

      <Section title="Spacing scale">
        <div className="flex items-end gap-2">
          {SPACING.map((px) => (
            <div key={px} className="flex flex-col items-center gap-1">
              <div className="w-6 rounded-sm bg-primary/50" style={{ height: `${px}px` }} />
              <span className="text-xs text-muted-foreground">{px}</span>
            </div>
          ))}
        </div>
      </Section>
    </>
  );
}
