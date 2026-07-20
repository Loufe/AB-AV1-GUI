# CRFty UI

React + TypeScript frontend for the CRFty Tauri shell (issue #36 is the
design record). Managed with pnpm.

```bash
pnpm install
pnpm dev           # Vite dev server (polls for changes — WSL2 /mnt/c)
pnpm tauri:dev     # full desktop app (shell crate + this frontend)
pnpm test          # vitest (includes Python-parity fixtures)
pnpm typecheck     # tsc -b
pnpm lint          # oxlint
pnpm format        # oxfmt (format:check in CI)
pnpm build         # tsc -b && vite build
```

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
- `src/lib/format/` — display formatters ported from the Python app;
  parity enforced by fixtures regenerated with
  `scripts/generate-parity-fixtures.py /path/to/main-checkout`.
- `src/dev/` — dev-gated (never in release bundles): the kitchen sink
  (token/primitive gallery + approved view mockups for queue and
  statistics) and the winning drag-spike reference (`@dnd-kit/react`).
