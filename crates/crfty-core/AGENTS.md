# crfty-core

Pure domain crate: state, reducer, fold, policy, journal types.

- No dependencies on filesystem, process, clock, async runtime, or UI crates.
  All effects live in crfty-engine; determinism is the contract.
- All state mutation flows through the reducer (ADR-002). State persists via the
  append-only journal (ADR-004).
- `cargo test -p crfty-core --test export_fold_fixtures` regenerates
  `ui/src/lib/store/fold-fixtures.json` — committed, freshness-gated in CI, and
  consumed by the ui fold mirror. Regenerate whenever fold semantics change.
- Pure-logic changes require focused unit tests.
