import { lazy, Suspense, useEffect, useState } from "react";
import { Toaster } from "sonner";

import { CloseDialog } from "@/components/close-dialog";
import { ErrorBoundary } from "@/components/error-boundary";
import { Sidebar } from "@/components/layout/sidebar";
import type { DevViewId, ViewId } from "@/components/layout/views";
import { SecondInstanceScreen } from "@/components/second-instance";
import { AnalysisView } from "@/features/analysis";
import { HistoryView } from "@/features/history";
import { QueueView } from "@/features/queue";
import { SettingsView } from "@/features/settings";
import { StatisticsView } from "@/features/statistics";
import { closeAppWindow, fetchAppVersion, isTauri } from "@/lib/ipc";
import { useAppStore } from "@/lib/store/app-store";
import { connectStream } from "@/lib/store/connect";
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
  return view === "kitchen-sink";
}

export default function App() {
  const [activeView, setActiveView] = useState<AppView>("queue");
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

  // A duplicate instance must not present a working app over state it cannot
  // touch: replace everything with the explicit already-running screen.
  if (secondInstance !== null) {
    return <SecondInstanceScreen lockPath={secondInstance} />;
  }

  return (
    <div className="flex h-full">
      <Sidebar
        activeView={activeView}
        onSelectView={setActiveView}
        theme={theme}
        onCycleTheme={cycleTheme}
        showDevViews={DEV_COMPONENTS !== null}
        appVersion={appVersion}
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
      <CloseDialog />
      <Toaster position="bottom-right" theme={theme} />
    </div>
  );
}
