# CRFty

Desktop application for quality-targeted AV1 analysis and conversion. V3 is a
ground-up Rust rewrite of the Python application retained on `main`.

## Stack

Rust stable (Edition 2024); Tauri shell; `ui/` frontend (Vite + React +
TypeScript + Tailwind v4, pnpm-managed); external FFmpeg/ffprobe processes.

## Workspace

- `crfty-core` — pure domain logic (state, reducer, fold, policy); no I/O.
- `crfty-engine` — external processes and filesystem I/O; no Tauri.
- `crfty-shell` — thin Tauri command/event bridge; no domain logic.
- `ui/` — React frontend; consumes generated bindings only.
- Mutable application state has one owner: the synchronous driver/reducer.

Each crate and `ui/` carries its own AGENTS.md with subsystem rules.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo deny check
```

The frontend gate runs from `ui/` — see `ui/AGENTS.md`.

## Strict rules

- Unsafe code, `unwrap`, `expect`, unchecked indexing, `todo!`, and `unimplemented!`
  are forbidden via `[workspace.lints]` — do not weaken this; ADR-005 documents the
  only escape hatch. Unit tests may narrowly use `unwrap`, `expect`, and indexing
  for fixture setup; integration-test crates declare those three Clippy allowances
  at their crate root; unsafe remains forbidden in all test code.
- Keep `Cargo.lock`, git dependency revisions, and structured-tool versions pinned.
  Manifest requirements use normal caret ranges; exact `=` pins are reserved for
  the specta pre-release family until it stabilizes. The Rust compiler itself
  follows the stable channel.
- Add dependencies only with cargo-deny policy updates in the same change.
- Pure logic must have focused tests.
- Log caught errors with context. Conversions may run for hours; non-critical
  telemetry or UI failures must not abort them.
- Never read logs or history containing real paths. Stop if unanonymized paths are
  encountered and do not quote them.
- Do not provide effort or duration estimates.
- Never commit Python — no scripts, no tooling, no dev dependencies. The V2 app
  retained on `main` is a read-only oracle: consult it via `git show main:<path>`,
  and freeze any semantics worth keeping as committed JSON fixtures. Fixture
  generation scripts are throwaway and never committed; once frozen, fixtures are
  spec data maintained by hand. Sole exception: `tools/export_history_v3.py` (and
  its test), the user-facing V2 history converter — standalone stdlib-only Python,
  tested via `uvx pytest tools/test_export_history_v3.py`, never imported by the
  build.

## Zero backwards compatibility

No external consumers exist. Change APIs and schemas directly, update all call
sites in the same change, and leave no compatibility artifacts. The one-time
Python history adoption is a product requirement, not compatibility policy.

## Design discipline

- Prefer the design where an invariant is unrepresentable over the design where
  its violation is well-tested. Two implementations kept equivalent by a test
  harness is a defect to eliminate, not a pattern to maintain.
- No mechanism ahead of measurement. Delta streams, mirrors, caches, debouncing,
  and concurrency tokens require an observed problem on real hardware, not an
  anticipated one. Start boring (send the whole state, recompute on request);
  an ADR introducing such machinery must cite the measurement.
- Parity preserves intentional semantics only. Never freeze accidental V2
  behavior as spec.
- "Unused" claims require tool verification (compiler, knip, cargo-machete),
  never text search alone; barrels and re-exports defeat grep.

## Worktrees

The main checkout stays on `main` — never edit files in it. It is used only for
read-only inspection, merges, and worktree management.

- Before any file modification, enter a git worktree on a `feature/*`, `fix/*`,
  or `refactor/*` branch. Canonical location: `.worktrees/<name>` at the repo
  root (`git worktree add .worktrees/<name> -b <type>/<name>`).
- Claude Code's `WorktreeCreate` hook (`.claude/settings.json`) automates this;
  other agents, and any subagent that writes files, follow it manually.
- After merging: push the target branch to origin, remove the worktree, and
  delete the branch. Don't leave finished worktrees, dead branches, or unpushed
  merges behind.

Architecture decisions: MADR records in `docs/adr/` (see its AGENTS.md; accepted
ADRs are immutable — supersede, don't rewrite). Issue #33 is the research narrative.
