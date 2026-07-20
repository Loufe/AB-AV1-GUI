import { createContext } from "react";

import type { Settings } from "@/lib/bindings";

export const SettingsContext = createContext<Settings | null>(null);
