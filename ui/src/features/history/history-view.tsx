import { History } from "lucide-react";
import { useMemo } from "react";

import { EmptyState } from "@/components/empty-state";
import { historyRows } from "@/lib/projection/history-rows";
import { useAppStore } from "@/lib/store/app-store";

import { historyDisplayRows } from "./history-model";
import { HistoryTable } from "./history-table";

export function HistoryView() {
  const durable = useAppStore((state) => state.durable);
  const snapshotReady = useAppStore((state) => state.settings !== null);
  const displayRows = useMemo(() => historyDisplayRows(historyRows(durable), durable), [durable]);

  if (!snapshotReady) {
    return (
      <EmptyState
        icon={History}
        title="Loading history…"
        description="Waiting for the desktop engine's current snapshot."
      />
    );
  }
  if (displayRows.length > 0) {
    return <HistoryTable rows={displayRows} />;
  }
  return (
    <EmptyState
      icon={History}
      title="No records yet"
      description="Every analyzed or converted file appears here with its outcome, sizes, and quality results."
    />
  );
}
