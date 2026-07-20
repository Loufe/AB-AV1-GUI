# CRFty UI

React + TypeScript frontend, pnpm-managed. Setup, layout, and environment quirks
(WSLg workaround, degraded mode): see README.md.

Gate — every commit green, mirrored by `.github/workflows/ui.yml`:
`pnpm lint && pnpm format:check && pnpm typecheck && pnpm test && pnpm build`

- `src/lib/bindings.ts` and `src/lib/store/fold-fixtures.json` are GENERATED
  (crfty-shell `export_bindings` / crfty-core `export_fold_fixtures`). Never edit
  by hand; regenerate via cargo when Rust types or fold semantics change.
- Never hand-author IPC or domain types — everything cross-boundary comes from
  `bindings.ts`.
- `src/lib/store/fold.ts` is a pure mirror of `crfty_core::fold`, verified against
  the golden fixtures. Change fold semantics in Rust first, then port.
- Zustand stores are containers only — reduce logic never lives in a store action.
- `src/lib/format/` semantics are frozen from the V2 app in `parity-fixtures.json`.
  The fixtures are hand-maintained spec data — edit them only as a deliberate,
  reviewed change; there is no regeneration path.
- `src/dev/` is dev-gated and never ships in release bundles.
- Before touching the drag, statistics, or queue views, read issue #36 comments
  D6, D7, and D11 (recorded design verdicts).
