import type { ReactNode } from "react";

import type { Settings } from "@/lib/bindings";

import { SettingsContext } from "./settings-context";

export function SettingsProvider({
  settings,
  children,
}: {
  settings: Settings | null;
  children: ReactNode;
}) {
  return <SettingsContext value={settings}>{children}</SettingsContext>;
}
