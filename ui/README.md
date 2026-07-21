# CRFty UI

React + TypeScript frontend for the CRFty Tauri shell (issue #36 is the
design record). Managed with pnpm.

```bash
pnpm install
pnpm dev           # Vite dev server (polls for changes — WSL2 /mnt/c)
pnpm tauri:dev     # full desktop app (shell crate + this frontend)
pnpm test          # Node unit tests + headless Chromium component tests
pnpm test:node     # reducers, projections, formatters, and other pure logic
pnpm test:browser  # real-browser component and interaction tests
pnpm typecheck     # tsc -b
pnpm lint          # oxlint
pnpm format        # oxfmt (format:check in CI)
pnpm build         # tsc -b && vite build
```

Install the browser binary once after `pnpm install`:

```bash
pnpm exec playwright install chromium
```

CI installs Chromium and its Linux system dependencies explicitly with the
Playwright version pinned in `pnpm-lock.yaml`.

Pure tests stay beside their source as `*.test.ts`. Tests that require a real
DOM, focus, keyboard input, portals, or React lifecycle behavior use
`*.browser.test.tsx`; shared browser helpers live in `src/test/browser/`.
Use `renderApp` for the production root providers and isolated Zustand state,
and `installTauriMock` to exercise generated commands and stream subscriptions
without mocking `src/lib/bindings.ts`.

`pnpm tauri:dev` builds `crates/crfty-shell` and opens the window against the
dev server; Linux needs the Tauri webkit2gtk prerequisites installed (see
`.github/workflows/rust.yml` for the package list). Without ffmpeg/ffprobe on
PATH (or `CRFTY_FFMPEG`/`CRFTY_FFPROBE` set) the app opens degraded: the
stream reports why and commands fail with `engine_unavailable`. Under WSLg
the webview crashes on the GPU path — launch with
`WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 pnpm tauri:dev`.

## Layout

- `src/components/ui/` — shadcn (Base UI) primitives after the house pass:
  token colors only, house radius/density, reviewed in both themes.
- `src/components/layout/` — sidebar shell and the `VIEWS`/`DEV_VIEWS` registries.
- `src/features/<view>/` — one folder per view; Settings reads the store,
  the other views ship empty states until they grow selectors over it.
- `src/lib/bindings.ts` — generated from the Rust types (see AGENTS.md);
  excluded from oxfmt/oxlint.
- `src/lib/ipc/` — `isTauri()` guard, event-stream subscription, and command
  helpers over the generated bindings.
- `src/lib/store/` — the state layer (#36 D5): `fold.ts` mirrors
  `crfty_core::fold`, verified against generated `fold-fixtures.json` (see
  AGENTS.md); `app-store.ts`/`progress-store.ts` are the Zustand containers
  (telemetry separate so progress ticks skip tree subscribers); `connect.ts`
  is the single stream consumer with the sequence tripwire.
- `src/lib/format/` — display formatters; semantics frozen from the V2 app
  as `parity-fixtures.json` (hand-maintained spec data, no regeneration path).
- `src/dev/` — dev-gated (never in release bundles): the kitchen sink
  (token/primitive gallery + approved view mockups for queue and
  statistics) and the winning drag-spike reference (`@dnd-kit/react`).
