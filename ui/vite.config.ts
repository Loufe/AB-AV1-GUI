/// <reference types="vitest/config" />
import path from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vite";

const DEFAULT_BROWSER_API_PORT = 63315;
const configuredBrowserPort = Number.parseInt(
  process.env.VITEST_BROWSER_PORT ?? String(DEFAULT_BROWSER_API_PORT),
  10,
);
const browserApiPort =
  Number.isInteger(configuredBrowserPort) && configuredBrowserPort > 0
    ? configuredBrowserPort
    : DEFAULT_BROWSER_API_PORT;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    // The Tauri shell's devUrl points here (crates/crfty-shell/tauri.conf.json).
    port: 5173,
    strictPort: true,
    // Dev runs under WSL2 against /mnt/c, where native file-change events
    // don't cross the Windows filesystem boundary — without polling, edits
    // leave stale transforms in the module cache and HMR never fires.
    watch: {
      usePolling: true,
      interval: 300,
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    projects: [
      {
        extends: true,
        test: {
          name: "node",
          environment: "node",
          include: ["src/**/*.test.{ts,tsx}"],
          exclude: ["src/**/*.browser.test.{ts,tsx}"],
        },
      },
      {
        extends: true,
        test: {
          name: "browser",
          include: ["src/**/*.browser.test.{ts,tsx}"],
          setupFiles: ["./src/test/browser/setup.ts"],
          browser: {
            enabled: true,
            headless: true,
            api: { port: browserApiPort },
            screenshotFailures: false,
            provider: playwright(),
            instances: [{ browser: "chromium" }],
          },
        },
      },
    ],
  },
});
