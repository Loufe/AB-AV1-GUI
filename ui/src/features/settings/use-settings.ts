import type { Settings } from "@/lib/bindings";
import { useAppStore } from "@/lib/store/app-store";

/** The acknowledged settings from the stream; null until the first snapshot. */
export function useSettings(): Settings | null {
  return useAppStore((state) => state.settings);
}
