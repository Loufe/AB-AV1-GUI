import { ChartColumn } from "lucide-react";

import { EmptyState } from "@/components/empty-state";

export function StatisticsView() {
  return (
    <EmptyState
      icon={ChartColumn}
      title="No statistics yet"
      description="Once files are converted, savings, encoding time, and quality distributions appear here automatically."
    />
  );
}
