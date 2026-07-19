import { History } from "lucide-react";

import { EmptyState } from "@/components/empty-state";

export function HistoryView() {
  return (
    <EmptyState
      icon={History}
      title="No records yet"
      description="Every analyzed or converted file appears here with its outcome, sizes, and quality results."
    />
  );
}
