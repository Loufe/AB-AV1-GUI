/// <reference types="vitest/config" />
import path from "node:path";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

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
    environment: "node",
  },
});
