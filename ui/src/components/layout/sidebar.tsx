import { Monitor, Moon, Sun } from "lucide-react";

import { VIEWS, type ViewId } from "@/components/layout/views";
import type { Theme } from "@/lib/theme";
import { cn } from "@/lib/utils";

const THEME_ICONS = { system: Monitor, light: Sun, dark: Moon } as const;

interface SidebarProps {
  activeView: ViewId;
  onSelectView: (view: ViewId) => void;
  theme: Theme;
  onCycleTheme: () => void;
}

export function Sidebar({ activeView, onSelectView, theme, onCycleTheme }: SidebarProps) {
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
