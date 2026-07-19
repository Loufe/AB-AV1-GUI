import { lazy, Suspense, useEffect, useState } from "react";
import { Toaster } from "sonner";

import { ErrorBoundary } from "@/components/error-boundary";
import { Sidebar } from "@/components/layout/sidebar";
import type { DevViewId, ViewId } from "@/components/layout/views";
import { AnalysisView } from "@/features/analysis";
import { HistoryView } from "@/features/history";
import { QueueView } from "@/features/queue";
import { SettingsView } from "@/features/settings";
import { StatisticsView } from "@/features/statistics";
import { getTheme, setTheme, watchSystemTheme, type Theme } from "@/lib/theme";

const THEME_ORDER: Theme[] = ["system", "light", "dark"];

// Dev-only workshop (#36 D10): the DEV gate is statically replaced in
// release builds, so the dynamic imports and their chunks are eliminated.
const DEV_COMPONENTS: Record<
  DevViewId,
  { label: string; Component: React.LazyExoticComponent<() => React.ReactNode> }
> | null = import.meta.env.DEV
  ? {
      "kitchen-sink": {
        label: "kitchen sink",
        Component: lazy(() => import("./dev/kitchen-sink")),
      },
      "spike-drag": {
        label: "drag spike",
        Component: lazy(() => import("./dev/spike-drag")),
      },
    }
  : null;

type AppView = ViewId | DevViewId;

const VIEW_COMPONENTS: Record<ViewId, { label: string; Component: () => React.ReactNode }> = {
  queue: { label: "Queue view", Component: QueueView },
  analysis: { label: "Analysis view", Component: AnalysisView },
  history: { label: "History view", Component: HistoryView },
  statistics: { label: "Statistics view", Component: StatisticsView },
  settings: { label: "Settings view", Component: SettingsView },
};

function isDevView(view: AppView): view is DevViewId {
  return view === "kitchen-sink" || view === "spike-drag";
}

export default function App() {
  const [activeView, setActiveView] = useState<AppView>("queue");
  const [theme, setThemeState] = useState<Theme>(getTheme);

  useEffect(() => watchSystemTheme(), []);

  const cycleTheme = () => {
    const next = THEME_ORDER[(THEME_ORDER.indexOf(theme) + 1) % THEME_ORDER.length];
    setTheme(next);
    setThemeState(next);
  };

  return (
    <div className="flex h-full">
      <Sidebar
        activeView={activeView}
        onSelectView={setActiveView}
        theme={theme}
        onCycleTheme={cycleTheme}
        showDevViews={DEV_COMPONENTS !== null}
      />
      <main className="flex-1 overflow-y-auto">
        {isDevView(activeView)
          ? DEV_COMPONENTS !== null &&
            (() => {
              const { label, Component } = DEV_COMPONENTS[activeView];
              return (
                <ErrorBoundary key={activeView} label={label}>
                  <Suspense fallback={null}>
                    <Component />
                  </Suspense>
                </ErrorBoundary>
              );
            })()
          : (() => {
              const { label, Component } = VIEW_COMPONENTS[activeView];
              return (
                <ErrorBoundary key={activeView} label={label}>
                  <Component />
                </ErrorBoundary>
              );
            })()}
      </main>
      <Toaster position="bottom-right" theme={theme} />
    </div>
  );
}
