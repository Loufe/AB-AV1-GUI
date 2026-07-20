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
cargo vet
```

The frontend gate runs from `ui/` — see `ui/AGENTS.md`.

## Strict rules

- Unsafe code, `unwrap`, `expect`, unchecked indexing, `todo!`, and `unimplemented!`
  are forbidden via `[workspace.lints]` — do not weaken this; ADR-005 documents the
  only escape hatch. Unit tests may narrowly use `unwrap`, `expect`, and indexing
  for fixture setup; integration-test crates declare those three Clippy allowances
  at their crate root; unsafe remains forbidden in all test code.
- Keep `Cargo.lock`, git dependency revisions, and structured-tool versions pinned.
  The Rust compiler itself follows the stable channel.
- Add dependencies only with cargo-deny and cargo-vet policy updates in the same
  change.
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
  spec data maintained by hand.

## Zero backwards compatibility

No external consumers exist. Change APIs and schemas directly, update all call
sites in the same change, and leave no compatibility artifacts. The one-time
Python history adoption is a product requirement, not compatibility policy.

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
