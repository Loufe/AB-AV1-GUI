import type { ReactNode } from "react";

import { Separator } from "@/components/ui/separator";

/**
 * D9 settings composition (from Handy): SettingsGroup (titled card) →
 * SettingContainer (row primitive) → one small component per setting.
 * Handy's optimistic-update path is deliberately absent: rows show pending
 * state while the acknowledged whole-object write is in flight.
 */

interface SettingsGroupProps {
  title: string;
  children: ReactNode;
}

export function SettingsGroup({ title, children }: SettingsGroupProps) {
  return (
    <section className="rounded-lg border border-border bg-card">
      <h2 className="px-4 pt-3 pb-1 text-sm font-medium text-muted-foreground">{title}</h2>
      <div className="flex flex-col">{children}</div>
    </section>
  );
}

interface SettingContainerProps {
  label: string;
  description?: string;
  /** Associates the row label with a native/form control when applicable. */
  htmlFor?: string;
  /** The control rendered on the row's trailing edge. */
  children: ReactNode;
  last?: boolean;
}

export function SettingContainer({
  label,
  description,
  htmlFor,
  children,
  last,
}: SettingContainerProps) {
  return (
    <>
      <div className="flex items-center justify-between gap-4 px-4 py-2.5">
        <div className="min-w-0">
          {htmlFor === undefined ? (
            <p className="text-sm">{label}</p>
          ) : (
            <label className="text-sm" htmlFor={htmlFor}>
              {label}
            </label>
          )}
          {description !== undefined && (
            <p className="text-xs text-muted-foreground">{description}</p>
          )}
        </div>
        <div className="shrink-0">{children}</div>
      </div>
      {!last && <Separator />}
    </>
  );
}
