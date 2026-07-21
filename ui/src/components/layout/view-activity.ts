import { createContext, useContext } from "react";

/**
 * Visibility contract for one production view. React Activity owns effect
 * cleanup while hidden; the explicit value keeps request-driven views from
 * treating retained component state as an active screen.
 */
export const ViewActiveContext = createContext<boolean | null>(null);

/** True only while the containing production view is visible. */
export function useViewActive(): boolean {
  const active = useContext(ViewActiveContext);
  if (active === null) {
    throw new Error("useViewActive must be used inside a production view");
  }
  return active;
}
