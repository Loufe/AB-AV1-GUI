# UI Design Verdicts

Status: stable reference; code comments cite the bare D-labels defined here  
Owning issue: [#36](https://github.com/Loufe/AB-AV1-GUI/issues/36)  
Last updated: 2026-08-16

## Purpose and boundary

Recorded design decisions for the V3 frontend. They were judged during the native UI design passes and are labeled D2 through D11; code comments cite the bare labels. This document is the authoritative record and survives the closure of its owning issue, because the labels remain cited from source.

It records what was decided, not how a view is built. Implementation work lives in the owning issue and its children.

- **D2, theme ownership**: the light/dark elevation tier is frontend-only state; the backend and journal store none of it. The tier is the single source of truth: shadcn component vocabulary resolves against it rather than holding its own colors.
- **D3, design tokens**: raw values live in `:root`/`.dark` as CSS custom properties and `@theme inline` references them, because literal colors in a plain `@theme` would be inlined at build time and break runtime theme switching.
- **D5, state layer and view isolation**: Zustand stores are containers only. State is written from outside React by the single stream consumer, and no reduce logic lives in a store action; deltas apply through the pure fold functions verified against the Rust fold's golden fixtures. Each view sits behind its own error boundary: a render crash in one view must not white-screen the app while a conversion is running.
- **D6, queue drag**: drag-reorder operates on pending items only and a multi-selection moves as one stable block; the optimistic dnd-kit index is translated to a destination in the authoritative order before submission. The active item never moves and pending work never lands ahead of it.
- **D7, statistics charts**: `chart-1` is the identity orange; the remaining series are hue-spaced for adjacent-series contrast at matched lightness.
- **D8, information access**: essential information is reachable by keyboard, never by hover alone, and displayed values use fixed unambiguous formats (dates render as `YYYY-MM-DD`).
- **D9, settings composition** (from Handy): `SettingsGroup` (titled card) contains `SettingContainer` (row primitive) contains one small component per setting. No optimistic updates: rows show pending state while the acknowledged whole-object write is in flight.
- **D10, dev workshop**: the kitchen sink is a dev-only component workshop, excluded from release bundles by the statically replaced `DEV` gate, so its chunks are never emitted.
- **D11, presentation language**: status columns use text color and weight plus at most one small icon, no pill chips. Estimates step down a three-state confidence ramp (exact after analyze, muted estimate after scan, blank before) replacing the `~`/`~~` jargon; estimated times explain their basis on demand. Reasons ride the item they describe, and every view has a named empty/first-run state.

Both themes ship at v3 with equal polish; the kitchen sink's `ThemePair` exists to keep that dual review cheap.
