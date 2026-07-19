import type { ReactNode } from "react";

/**
 * Renders the same content in light and dark side by side (#36: both themes
 * ship at v3 with equal polish — this keeps the dual review cheap). The dark
 * copy works by scoping the .dark class, which re-resolves every token.
 */
export function ThemePair({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-2 gap-3">
      <div className="rounded-lg border border-border bg-background p-4">{children}</div>
      <div className="dark rounded-lg border border-border bg-background p-4 text-foreground">
        {children}
      </div>
    </div>
  );
}

export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-xl">{title}</h2>
      {children}
    </section>
  );
}
