import { useEffect, useState } from "react";
import { Toaster } from "sonner";

import { ErrorBoundary } from "@/components/error-boundary";
import { Sidebar } from "@/components/layout/sidebar";
import type { ViewId } from "@/components/layout/views";
import { AnalysisView } from "@/features/analysis";
import { HistoryView } from "@/features/history";
import { QueueView } from "@/features/queue";
import { SettingsView } from "@/features/settings";
import { StatisticsView } from "@/features/statistics";
import { getTheme, setTheme, watchSystemTheme, type Theme } from "@/lib/theme";

const THEME_ORDER: Theme[] = ["system", "light", "dark"];

const VIEW_COMPONENTS: Record<ViewId, { label: string; Component: () => React.ReactNode }> = {
  queue: { label: "Queue view", Component: QueueView },
  analysis: { label: "Analysis view", Component: AnalysisView },
  history: { label: "History view", Component: HistoryView },
  statistics: { label: "Statistics view", Component: StatisticsView },
  settings: { label: "Settings view", Component: SettingsView },
};

export default function App() {
  const [activeView, setActiveView] = useState<ViewId>("queue");
  const [theme, setThemeState] = useState<Theme>(getTheme);

  useEffect(() => watchSystemTheme(), []);

  const cycleTheme = () => {
    const next = THEME_ORDER[(THEME_ORDER.indexOf(theme) + 1) % THEME_ORDER.length];
    setTheme(next);
    setThemeState(next);
  };

  const { label, Component } = VIEW_COMPONENTS[activeView];

  return (
    <div className="flex h-full">
      <Sidebar
        activeView={activeView}
        onSelectView={setActiveView}
        theme={theme}
        onCycleTheme={cycleTheme}
      />
      <main className="flex-1 overflow-y-auto">
        <ErrorBoundary key={activeView} label={label}>
          <Component />
        </ErrorBoundary>
      </main>
      <Toaster position="bottom-right" theme={theme} />
    </div>
  );
}
