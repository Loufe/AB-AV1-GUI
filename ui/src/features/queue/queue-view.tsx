import { ListVideo } from "lucide-react";

import { EmptyState } from "@/components/empty-state";

export function QueueView() {
  return (
    <EmptyState
      icon={ListVideo}
      title="The queue is empty"
      description="Add files from the Analysis view to analyze or convert them. Items keep their order and can be rearranged before you start."
    />
  );
}
