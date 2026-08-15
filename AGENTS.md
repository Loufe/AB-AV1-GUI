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
  only unsafe escape hatch. `#[allow(...)]` is forbidden. Tests follow the same
  defaults as production: refactor first, and use the narrowest item-scoped
  `#[expect(..., reason = "...")]` only when a test invariant is clearer than
  propagating setup failure. Unsafe remains forbidden in all test code.
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

## Comments

A comment states a constraint or rationale the code cannot show: invariants,
cross-boundary contracts, non-obvious "why". Module docs (`//!`) stating a
subsystem's contract are encouraged. Delete on sight:

- File-path headers (`// crates/crfty-core/src/foo.rs`)
- Section banners (`// ---- helpers ----`)
- Narration of the next line
- Change commentary; "why this edit is correct" belongs in the commit message
- Issue references (`#NN`); state the constraint inline or cite an ADR

Partially enforced by `crates/crfty-engine/tests/source_policy.rs`.

## Zero backwards compatibility

No external consumers exist. Change APIs and schemas directly, update all call
sites in the same change, and leave no compatibility artifacts. The one-time
Python history adoption is a product requirement, not compatibility policy.

- **External contracts included** — When a dependency or external tool changes
  format, update the required version and replace the old handling. Never support
  both formats.

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

## GitHub Issues

- Caps: body 30 lines (epics 50), goal 3 sentences, acceptance 5 checkboxes
- Link the parent issue; never restate its content
- Design/research content goes in `docs/design/`; the issue links the doc
- Epics: a 2-3 line header plus a checkbox list of child issues, nothing else
- No meta-process prose (scope disclaimers, "this issue does not decide...")
- Draft from `docs/templates/issue.md` / `epic.md`; assign milestone `v3.0`

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
ADRs are immutable — supersede, don't rewrite). Current rewrite state, decided
directions, and open questions: `docs/PLAN.md`.
