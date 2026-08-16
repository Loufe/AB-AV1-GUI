# CRFty UI

React + TypeScript frontend, pnpm-managed. Setup, layout, and environment quirks
(WSLg workaround, degraded mode): see README.md.

Gate (every commit green, mirrored by `.github/workflows/ui.yml`):
`pnpm lint && pnpm format:check && pnpm typecheck && pnpm knip && pnpm test && pnpm build`

Knip keeps the tree free of dead exports: no barrel files, no unused exports
(de-export symbols used only in-file), no speculative "future UI" surface.
Deleted code is recoverable from git history or the shadcn registry.

- `src/lib/bindings.ts`, `src/lib/store/fold-fixtures.json`, and
  `src/lib/projection/projection-fixtures.json` are GENERATED (crfty-shell
  `export_bindings` / crfty-core `export_fold_fixtures` /
  `export_projection_fixtures`). Never edit by hand; regenerate via cargo when
  Rust types or projection/fold semantics change.
- Never hand-author IPC or domain types: everything cross-boundary comes from
  `bindings.ts`.
- `src/lib/store/fold.ts` is a pure mirror of `crfty_core::fold`, and
  `src/lib/projection/history-rows.ts` of `crfty_core::history_rows`; both are
  verified against the golden fixtures. Change semantics in Rust first, then port.
- Zustand stores are containers only: reduce logic never lives in a store action.
- `src/lib/format/` semantics are frozen from the V2 app in `parity-fixtures.json`.
  The fixtures are hand-maintained spec data: edit them only as a deliberate,
  reviewed change; there is no regeneration path.
- `src/dev/` is dev-gated and never ships in release bundles.
- Before touching the drag, statistics, or queue views, read the recorded
  design verdicts (D6, D7, D11) in `docs/design/ui-verdicts.md`.

## Ownership boundary

The frontend owns presentation, accessibility, and interaction. It does not own durable authority, History or Statistics aggregation, Analysis eligibility, or item-state transitions: those are engine facts that arrive on the stream, and the UI changes them only by submitting a command from `src/lib/ipc/` and folding the delta that comes back.

- Frontend-owned state stays in React or a view module: active and visited views, per-view scroll and focus, table sort and filter, queue selection, drag interaction, theme, and request lifecycle. None of it is persisted or sent to the engine.
- Engine-owned state is read, never authored: `app-store` (durable state, settings, session, tools, health), `analysis-store` (the current generation), `progress-store` (telemetry and session aggregates). `connect.ts` is the only writer of domain values; a component may write nothing but the frontend's own intent fields.
- Durable order and membership change only when the engine's delta arrives. A drag's optimistic movement is gesture feedback: translate it into a destination for the engine (`queue-dnd-adapter.ts`) and never treat it as the new order.
- `connect.ts` is the single stream consumer and the only place payloads are routed. Components subscribe to stores, never to the channel.
- Resubscription is the one resync primitive: a sequence gap reconnects, and the replayed snapshot supersedes anything the gap lost. Do not add a second sync mechanism (polling, refetch-on-invalidate, or a reconciliation pass).
- Statistics is request-driven and never replayed on subscribe, so a view that needs it re-requests after every snapshot. History is folded locally from the durable snapshot through `history-rows.ts`; keep it there until the engine serves History by request.
- High-rate data stays out of `app-store`: telemetry and analysis rows have their own stores so progress ticks never rerender queue or history subscribers.
- No state-machine library (XState or any FSM package): the fold over typed deltas already supplies that discipline, and the transitions are the engine's, mirrored here rather than modeled a second time.

## Cross-view requirements

These hold for all five production views. A new view is not finished until it meets them.

- Register in `VIEWS` and `VIEW_COMPONENTS`. Views mount under `<Activity>` after their first visit, so local presentation state, scroll position, and focus survive navigation and must not reset on re-show.
- Each view scrolls in its own container. Never hoist scrolling to the window or share a scroll position across views.
- Work that should happen only while a view is on screen is gated on `useViewActive()`. Retained component state is not an active screen.
- Essential information is readable without hover and without color alone: pair every tone with a label or an icon, and make a tooltip trigger focusable so its detail is reachable by keyboard.
- Every control is keyboard-operable, state changes that matter announce through `role="status"` or `role="alert"`, and decorative icons are `aria-hidden`.
- Reduced motion is handled once, globally, in `index.css`. Do not add per-component motion queries, and never let a state change be visible only as an animation.
- Views stay usable at realistic volumes. History virtualizes its rows, so row identity comes from a tagged domain id, never a visible index or label. Large-folder Analysis presentation is an unresolved design question; do not invent a windowing strategy ahead of that decision.
