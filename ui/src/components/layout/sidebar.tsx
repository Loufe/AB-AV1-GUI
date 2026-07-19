import { Monitor, Moon, Sun } from "lucide-react";

import { DEV_VIEWS, VIEWS, type DevViewId, type ViewId } from "@/components/layout/views";
import type { Theme } from "@/lib/theme";
import { cn } from "@/lib/utils";

const THEME_ICONS = { system: Monitor, light: Sun, dark: Moon } as const;

type AnyViewId = ViewId | DevViewId;

interface SidebarProps {
  activeView: AnyViewId;
  onSelectView: (view: AnyViewId) => void;
  theme: Theme;
  onCycleTheme: () => void;
  /** Dev builds only: shows the workshop entries (kitchen sink, spikes). */
  showDevViews: boolean;
}

export function Sidebar({
  activeView,
  onSelectView,
  theme,
  onCycleTheme,
  showDevViews,
}: SidebarProps) {
  const ThemeIcon = THEME_ICONS[theme];

  return (
    <aside className="flex w-44 flex-col border-r border-border bg-surface">
      <div className="px-4 py-3">
        <span className="text-lg font-medium">CRFty</span>
      </div>
      <nav className="flex flex-1 flex-col gap-0.5 px-2" aria-label="Views">
        {VIEWS.map((view) => (
          <NavButton
            key={view.id}
            icon={view.icon}
            label={view.label}
            active={view.id === activeView}
            onClick={() => onSelectView(view.id)}
          />
        ))}
      </nav>
      {showDevViews && (
        <div className="flex flex-col gap-0.5 px-2 pb-1">
          {DEV_VIEWS.map((view) => (
            <NavButton
              key={view.id}
              icon={view.icon}
              label={view.label}
              active={view.id === activeView}
              onClick={() => onSelectView(view.id)}
            />
          ))}
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

function NavButton({
  icon: Icon,
  label,
  active,
  onClick,
}: {
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" }>;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={cn(
        "flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors duration-(--duration-fast)",
        active
          ? "bg-muted text-foreground"
          : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
      )}
    >
      <Icon className="size-4 shrink-0" aria-hidden="true" />
      {label}
    </button>
  );
}
