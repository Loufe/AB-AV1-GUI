import { Activity, lazy, Suspense, useEffect, useState } from "react";
import { Toaster } from "sonner";

import { CloseDialog } from "@/components/close-dialog";
import { ErrorBoundary } from "@/components/error-boundary";
import { Sidebar } from "@/components/layout/sidebar";
import { ViewActiveContext } from "@/components/layout/view-activity";
import type { DevViewId, ViewId } from "@/components/layout/views";
import { VIEWS } from "@/components/layout/views";
import { SecondInstanceScreen } from "@/components/second-instance";
import { AnalysisView } from "@/features/analysis/analysis-view";
import { HistoryView } from "@/features/history/history-view";
import { QueueView } from "@/features/queue/queue-view";
import { SettingsView } from "@/features/settings/settings-view";
import { closeAppWindow, fetchAppVersion, isTauri } from "@/lib/ipc";
import { useAppStore } from "@/lib/store/app-store";
import { connectStream } from "@/lib/store/connect";
import { getTheme, setTheme, watchSystemTheme, type Theme } from "@/lib/theme";

const THEME_ORDER: Theme[] = ["system", "light", "dark"];

// Recharts is intentionally loaded only on the first Statistics visit. Once
// visited, Activity retains the view and its local presentation state.
const LazyStatisticsView = lazy(() =>
  import("@/features/statistics").then(({ StatisticsView }) => ({ default: StatisticsView })),
);

function StatisticsRoute() {
  return (
    <Suspense
      fallback={
        <div
          className="flex h-full items-center justify-center text-sm text-muted-foreground"
          role="status"
        >
          Loading Statistics view…
        </div>
      }
    >
      <LazyStatisticsView />
    </Suspense>
  );
}

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
    }
  : null;

type AppView = ViewId | DevViewId;

const VIEW_COMPONENTS: Record<ViewId, { label: string; Component: () => React.ReactNode }> = {
  queue: { label: "Queue view", Component: QueueView },
  analysis: { label: "Analysis view", Component: AnalysisView },
  history: { label: "History view", Component: HistoryView },
  statistics: { label: "Statistics view", Component: StatisticsRoute },
  settings: { label: "Settings view", Component: SettingsView },
};

function isDevView(view: AppView): view is DevViewId {
  return view === "kitchen-sink";
}

export default function App() {
  const [activeView, setActiveView] = useState<AppView>("queue");
  // Activity preserves a production view after its first visit. Keeping the
  // visited set explicit prevents hidden views from eagerly rendering or
  // starting work during application startup.
  const [visitedViews, setVisitedViews] = useState<ReadonlySet<ViewId>>(
    () => new Set<ViewId>(["queue"]),
  );
  const [theme, setThemeState] = useState<Theme>(getTheme);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const secondInstance = useAppStore((state) => state.health.secondInstance);
  const session = useAppStore((state) => state.session);
  const quitAfterSession = useAppStore((state) => state.quitAfterSession);

  useEffect(() => watchSystemTheme(), []);

  // A close-dialog choice armed the quit; with the session idle, the shell
  // now lets the re-issued close through (#33 §12).
  useEffect(() => {
    if (!isTauri() || !quitAfterSession || session !== "Idle") {
      return;
    }
    closeAppWindow().catch((error: unknown) => {
      console.error("failed to close the window", error);
    });
  }, [quitAfterSession, session]);

  // Under Tauri, connect the stores to the shell's ordered event stream and
  // show the shell version. connectStream is idempotent, so StrictMode's
  // double-mount reuses the first connection.
  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    connectStream();
    fetchAppVersion()
      .then(setAppVersion)
      .catch((error: unknown) => {
        console.error("failed to fetch the shell version", error);
      });
  }, []);

  const cycleTheme = () => {
    const next = THEME_ORDER[(THEME_ORDER.indexOf(theme) + 1) % THEME_ORDER.length];
    setTheme(next);
    setThemeState(next);
  };

  const selectView = (view: AppView) => {
    setActiveView(view);
    if (isDevView(view)) {
      return;
    }
    setVisitedViews((visited) => {
      if (visited.has(view)) {
        return visited;
      }
      const next = new Set(visited);
      next.add(view);
      return next;
    });
  };

  // A duplicate instance must not present a working app over state it cannot
  // touch: replace everything with the explicit already-running screen.
  if (secondInstance !== null) {
    return <SecondInstanceScreen lockPath={secondInstance} />;
  }

  return (
    <div className="flex h-full">
      <Sidebar
        activeView={activeView}
        onSelectView={selectView}
        theme={theme}
        onCycleTheme={cycleTheme}
        showDevViews={DEV_COMPONENTS !== null}
        appVersion={appVersion}
      />
      <main className="min-w-0 flex-1 overflow-hidden">
        {VIEWS.map(({ id }) => {
          if (!visitedViews.has(id)) {
            return null;
          }
          const { label, Component } = VIEW_COMPONENTS[id];
          const active = activeView === id;
          return (
            <Activity key={id} mode={active ? "visible" : "hidden"}>
              <section
                id={`view-panel-${id}`}
                aria-labelledby={`view-nav-${id}`}
                className="h-full overflow-y-auto"
              >
                <ViewActiveContext value={active}>
                  <ErrorBoundary label={label}>
                    <Component />
                  </ErrorBoundary>
                </ViewActiveContext>
              </section>
            </Activity>
          );
        })}
        {isDevView(activeView) &&
          DEV_COMPONENTS !== null &&
          (() => {
            const { label, Component } = DEV_COMPONENTS[activeView];
            return (
              <section
                id={`view-panel-${activeView}`}
                aria-labelledby={`view-nav-${activeView}`}
                className="h-full overflow-y-auto"
              >
                <ErrorBoundary key={activeView} label={label}>
                  <Suspense fallback={null}>
                    <Component />
                  </Suspense>
                </ErrorBoundary>
              </section>
            );
          })()}
      </main>
      <CloseDialog />
      <Toaster position="bottom-right" theme={theme} />
    </div>
  );
}
