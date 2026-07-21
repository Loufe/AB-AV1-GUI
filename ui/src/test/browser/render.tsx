import type { ReactNode } from "react";
import type { RootOptions } from "react-dom/client";
import { Toaster } from "sonner";
import { render, type RenderResult } from "vitest-browser-react";

import { ErrorBoundary } from "@/components/error-boundary";
import { TooltipProvider } from "@/components/ui/tooltip";
import { appStore, initialAppState, type AppStoreState } from "@/lib/store/app-store";
import {
  emptySessionAggregates,
  progressStore,
  type ProgressStoreState,
} from "@/lib/store/progress-store";
import { setTheme, type Theme } from "@/lib/theme";

export interface RenderAppOptions {
  appState?: Partial<AppStoreState>;
  progressState?: Partial<ProgressStoreState>;
  theme?: Exclude<Theme, "system">;
  createRootOptions?: RootOptions;
}

/** Restore both production stores to independent, deterministic test state. */
export function resetTestStores(
  appState: Partial<AppStoreState> = {},
  progressState: Partial<ProgressStoreState> = {},
): void {
  appStore.setState({ ...initialAppState(), ...appState }, true);
  progressStore.setState(
    {
      telemetry: {},
      aggregates: emptySessionAggregates(),
      ...progressState,
    },
    true,
  );
}

/**
 * Render through the providers shared by production views. The Zustand stores
 * are reset before every render because production hooks intentionally bind to
 * singleton stores rather than context providers.
 */
export async function renderApp(
  component: ReactNode,
  { appState = {}, progressState = {}, theme = "light", createRootOptions }: RenderAppOptions = {},
): Promise<RenderResult> {
  resetTestStores(appState, progressState);
  setTheme(theme);

  const rendered = await render(
    <>
      <ErrorBoundary label="test view">
        <TooltipProvider>{component}</TooltipProvider>
      </ErrorBoundary>
      <Toaster position="bottom-right" theme={theme} />
    </>,
    { createRootOptions },
  );
  // Match the desktop root's `html, body, #root { height: 100% }` contract so
  // full-height application layouts have a real scroll viewport in Chromium.
  rendered.container.style.height = "100vh";
  return rendered;
}
