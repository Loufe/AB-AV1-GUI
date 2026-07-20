import { useContext } from "react";

import { SettingsContext } from "./settings-context";

export function useSettings() {
  return useContext(SettingsContext);
}
