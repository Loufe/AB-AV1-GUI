# CRFty

Desktop application for quality-targeted AV1 analysis and conversion. V3 is a
ground-up Rust rewrite of the Python application retained on `main`.

## Stack

- Rust stable, Edition 2024
- Tauri with a React/TypeScript frontend when the shell phase begins
- External FFmpeg/ffprobe processes
- A pinned, narrowly patched ab-av1 library dependency in the engine adapter

## Workspace boundaries

- `crfty-core` is pure domain code. It must not depend on filesystem, process,
  clock, async runtime, or UI crates.
- `crfty-engine` owns processes and filesystem I/O but must not depend on Tauri.
- The future Tauri shell is a thin command and event bridge with no domain logic.
- Mutable application state has one owner: the synchronous driver/reducer.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo deny check
cargo vet
```

## Strict rules

- First-party crates forbid unsafe Rust. Do not weaken this globally. If a
  platform acceptance test eventually proves direct OS calls unavoidable, isolate
  them in the separately reviewed platform crate described by ADR-005.
- Do not use `unwrap`, `expect`, unchecked indexing, `todo!`, or `unimplemented!`
  in production code.
- Keep `Cargo.lock`, git dependency revisions, and structured-tool versions pinned.
  The Rust compiler itself follows the stable channel.
- Add dependencies only with cargo-deny and cargo-vet policy updates in the same
  change.
- Pure logic must have focused tests. Process behavior requires real-process
  contract tests in addition to unit tests.
- Do not introduce a generic encoder trait until a second backend is implemented.
- Do not parse human-oriented process output as an application contract.
- Log caught errors with context. Conversions may run for hours; non-critical
  telemetry or UI failures must not abort them.
- Never read logs or history containing real paths. Stop if unanonymized paths are
  encountered and do not quote them.
- Do not provide effort or duration estimates.

## Zero backwards compatibility

There are no external consumers of the rewrite. Change APIs and schemas directly,
update all call sites in the same change, and leave no shims, aliases, fallback
imports, migrations, or obsolete artifacts. The one-time Python history adoption
is an explicit product requirement, not general compatibility policy.

## Architecture decisions

ADRs use MADR and live in `docs/adr/`. Accepted ADRs are immutable; supersede them
with a new record when a decision changes. Issue #33 remains the detailed narrative
and research record.
