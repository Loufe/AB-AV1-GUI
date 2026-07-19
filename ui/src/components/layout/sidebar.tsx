import { FlaskConical, Monitor, Moon, Sun } from "lucide-react";

import { VIEWS, type ViewId } from "@/components/layout/views";
import type { Theme } from "@/lib/theme";
import { cn } from "@/lib/utils";

const THEME_ICONS = { system: Monitor, light: Sun, dark: Moon } as const;

interface SidebarProps {
  activeView: ViewId | "kitchen-sink";
  onSelectView: (view: ViewId | "kitchen-sink") => void;
  theme: Theme;
  onCycleTheme: () => void;
  /** Dev builds only: shows the kitchen-sink workshop entry. */
  showKitchenSink: boolean;
}

export function Sidebar({
  activeView,
  onSelectView,
  theme,
  onCycleTheme,
  showKitchenSink,
}: SidebarProps) {
  const ThemeIcon = THEME_ICONS[theme];

  return (
    <aside className="flex w-44 flex-col border-r border-border bg-surface">
      <div className="px-4 py-3">
        <span className="text-lg font-medium">CRFty</span>
      </div>
      <nav className="flex flex-1 flex-col gap-0.5 px-2" aria-label="Views">
        {VIEWS.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            type="button"
            onClick={() => onSelectView(id)}
            aria-current={id === activeView ? "page" : undefined}
            className={cn(
              "flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors duration-(--duration-fast)",
              id === activeView
                ? "bg-muted text-foreground"
                : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
            )}
          >
            <Icon className="size-4 shrink-0" aria-hidden="true" />
            {label}
          </button>
        ))}
      </nav>
      {showKitchenSink && (
        <div className="px-2 pb-1">
          <button
            type="button"
            onClick={() => onSelectView("kitchen-sink")}
            aria-current={activeView === "kitchen-sink" ? "page" : undefined}
            className={cn(
              "flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors duration-(--duration-fast)",
              activeView === "kitchen-sink"
                ? "bg-muted text-foreground"
                : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
            )}
          >
            <FlaskConical className="size-4 shrink-0" aria-hidden="true" />
            Kitchen sink
          </button>
        </div>
      )}
      <div className="border-t border-border p-2">
        <button
          type="button"
          onClick={onCycleTheme}
          className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground transition-colors duration-(--duration-fast) hover:bg-muted/60 hover:text-foreground"
        >
          <ThemeIcon className="size-4 shrink-0" aria-hidden="true" />
          Theme: {theme}
        </button>
      </div>
    </aside>
  );
}
