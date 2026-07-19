import { FolderOpen } from "lucide-react";

import { EmptyState } from "@/components/empty-state";
import { Button } from "@/components/ui/button";

export function AnalysisView() {
  return (
    <EmptyState
      icon={FolderOpen}
      title="No folder selected"
      description="Choose a folder to scan for videos. Scanning is instant; estimates appear after a basic scan."
      action={
        <Button disabled title="Available once the engine is connected">
          Choose folder…
        </Button>
      }
    />
  );
}
