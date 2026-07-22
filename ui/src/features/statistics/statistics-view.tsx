import { ChartColumn, LoaderCircle } from "lucide-react";

import { EmptyState } from "@/components/empty-state";

import { hasStatisticsData } from "./statistics-display";
import { StatisticsPanel } from "./statistics-panel";
import { useStatisticsRequest } from "./use-statistics-request";

export function StatisticsView() {
  const { payload, phase, error, utcOffsetMinutes } = useStatisticsRequest();
  const loading = payload === null && error === null;

  return (
    <div
      className="mx-auto flex min-h-full max-w-7xl flex-col gap-4 p-6"
      aria-busy={phase !== "idle"}
    >
      <header className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-medium">Statistics</h1>
          <p className="text-sm text-muted-foreground">
            Engine-projected conversion, remux, and terminal-run history
          </p>
        </div>
        {payload !== null && phase === "refreshing" && (
          <p className="flex items-center gap-1.5 text-xs text-muted-foreground" role="status">
            <LoaderCircle className="size-3.5 animate-spin" aria-hidden="true" />
            Refreshing statistics…
          </p>
        )}
      </header>

      {error !== null && (
        <p
          className="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          role="alert"
        >
          {error}
          {payload !== null && " Showing the last valid response."}
        </p>
      )}

      {loading && (
        <div className="flex flex-1 items-center justify-center" role="status">
          <EmptyState
            icon={LoaderCircle}
            title="Loading statistics"
            description={`Requesting daily aggregates for UTC${utcOffsetMinutes >= 0 ? "+" : ""}${utcOffsetMinutes / 60}.`}
          />
        </div>
      )}

      {payload === null && error !== null && (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={ChartColumn}
            title="Statistics unavailable"
            description="Statistics will be requested again when this view is reopened or the application regains focus."
          />
        </div>
      )}

      {payload !== null && !hasStatisticsData(payload) && (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={ChartColumn}
            title="No statistics yet"
            description="Conversion, remux, analysis, and other terminal run outcomes will appear here automatically."
          />
        </div>
      )}

      {payload !== null && hasStatisticsData(payload) && <StatisticsPanel payload={payload} />}
    </div>
  );
}
