import type { Settings, StreamPayload } from "@/lib/bindings";

export function foldSettings(current: Settings | null, payload: StreamPayload): Settings | null {
  if ("Snapshot" in payload && payload.Snapshot !== undefined) {
    return payload.Snapshot.settings;
  }
  if ("Config" in payload && payload.Config !== undefined) {
    return payload.Config.SettingsChanged.settings;
  }
  return current;
}
