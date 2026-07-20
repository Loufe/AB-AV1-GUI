# crfty-shell

Thin Tauri command and event bridge — no domain logic (ADR-001, ADR-006). Commands
delegate to the engine; events forward the ordered delta stream.

- `cargo test -p crfty-shell --test export_bindings` generates
  `ui/src/lib/bindings.ts` from the Rust types (tauri-specta, ADR-006). Committed,
  never hand-edited, freshness-gated in CI. Regenerate whenever cross-boundary
  types change.
- All cross-boundary types are defined in Rust (issue #33); the frontend must never
  hand-author IPC or domain types.
- Run via `pnpm tauri:dev` from `ui/` (Linux prerequisites: see `ui/README.md`).
  Tauri capability grants live in `capabilities/`.
