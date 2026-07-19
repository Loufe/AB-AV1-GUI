import {
  ChartColumn,
  FolderSearch,
  History,
  ListVideo,
  Settings,
  type LucideIcon,
} from "lucide-react";

export type ViewId = "queue" | "analysis" | "history" | "statistics" | "settings";

export interface ViewDefinition {
  id: ViewId;
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
