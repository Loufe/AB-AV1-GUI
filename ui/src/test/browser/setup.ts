import "@/index.css";
import "vitest-browser-react";

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, vi } from "vitest";
import { cleanup } from "vitest-browser-react";
import { configure } from "vitest-browser-react/pure";

import { resetTestStores } from "./render";

configure({ reactStrictMode: true });

function resetBrowserState(): void {
  resetTestStores();
  localStorage.removeItem("crfty-theme");
  document.documentElement.classList.remove("dark");
}

beforeEach(() => {
  resetBrowserState();
});

afterEach(async () => {
  await cleanup();
  clearMocks();
  vi.useRealTimers();
  resetBrowserState();
});
