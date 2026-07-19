import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  description: string;
  /** Optional call-to-action, e.g. a button routing to another view. */
  action?: ReactNode;
}

/** Named per-view empty/first-run states (#36 D11). */
export function EmptyState({ icon: Icon, title, description, action }: EmptyStateProps) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
      <Icon className="mb-2 size-10 text-muted-foreground/60" aria-hidden="true" />
      <p className="text-lg">{title}</p>
      <p className="max-w-md text-sm text-muted-foreground">{description}</p>
      {action !== undefined && <div className="mt-3">{action}</div>}
    </div>
  );
}
