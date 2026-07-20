import {
  ChartColumn,
  FlaskConical,
  FolderSearch,
  History,
  ListVideo,
  Settings,
  type LucideIcon,
} from "lucide-react";

export type ViewId = "queue" | "analysis" | "history" | "statistics" | "settings";

/** Dev-build-only workshop views (kitchen sink, spikes). */
export type DevViewId = "kitchen-sink";

export interface ViewDefinition<Id extends string = ViewId> {
  id: Id;
  label: string;
  icon: LucideIcon;
}

export const VIEWS: readonly ViewDefinition[] = [
  { id: "queue", label: "Queue", icon: ListVideo },
  { id: "analysis", label: "Analysis", icon: FolderSearch },
  { id: "history", label: "History", icon: History },
  { id: "statistics", label: "Statistics", icon: ChartColumn },
  { id: "settings", label: "Settings", icon: Settings },
];

export const DEV_VIEWS: readonly ViewDefinition<DevViewId>[] = [
  { id: "kitchen-sink", label: "Kitchen sink", icon: FlaskConical },
];
