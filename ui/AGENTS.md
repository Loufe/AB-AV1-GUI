# CRFty UI

React + TypeScript frontend, pnpm-managed. Setup, layout, and environment quirks
(WSLg workaround, degraded mode): see README.md.

Gate — every commit green, mirrored by `.github/workflows/ui.yml`:
`pnpm lint && pnpm format:check && pnpm typecheck && pnpm knip && pnpm test && pnpm build`

Knip keeps the tree free of dead exports: no barrel files, no unused exports
(de-export symbols used only in-file), no speculative "future UI" surface.
Deleted code is recoverable from git history or the shadcn registry.

- `src/lib/bindings.ts`, `src/lib/store/fold-fixtures.json`, and
  `src/lib/projection/projection-fixtures.json` are GENERATED (crfty-shell
  `export_bindings` / crfty-core `export_fold_fixtures` /
  `export_projection_fixtures`). Never edit by hand; regenerate via cargo when
  Rust types or projection/fold semantics change.
- Never hand-author IPC or domain types — everything cross-boundary comes from
  `bindings.ts`.
- `src/lib/store/fold.ts` is a pure mirror of `crfty_core::fold`, and
  `src/lib/projection/history-rows.ts` of `crfty_core::history_rows`; both are
  verified against the golden fixtures. Change semantics in Rust first, then port.
- Zustand stores are containers only — reduce logic never lives in a store action.
- `src/lib/format/` semantics are frozen from the V2 app in `parity-fixtures.json`.
  The fixtures are hand-maintained spec data — edit them only as a deliberate,
  reviewed change; there is no regeneration path.
- `src/dev/` is dev-gated and never ships in release bundles.
- Before touching the drag, statistics, or queue views, read issue #36 comments
  D6, D7, and D11 (recorded design verdicts).
