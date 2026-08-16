# Rewrite Plan

Living state of the V3 rewrite. Update when a phase lands or a direction is decided. Decisions get an ADR; long-form design goes in `docs/design/`.

## Current phase

Engine and queue foundations are complete and contract-tested: pinned ab-av1 adapter, durable job coordinator, journal replay and crash recovery, the queue command surface, the bounded event stream, the Tauri shell, and the UI fold over golden fixtures. Current work is growing the views over the store: History, Statistics, and the final Queue integration. Analysis has generation-scoped streaming discovery and the bounded Basic Scan pipeline (`docs/ANALYSIS.md`); later tiers add estimates, actions, and presentation.

## Decided

- Three-crate split (core/engine/shell) with one state owner: ADR-001, ADR-002
- Append-only journal, snapshot-head compaction, generation-identity corruption handling: ADR-004, ADR-009, ADR-011
- Pinned ab-av1 adapter; vendored, checksummed FFmpeg: ADR-003, ADR-010
- IPC bindings generated from Rust via tauri-specta: ADR-006
- History and Statistics derive as pure projections, imported history projected separately: ADR-015 (supersedes ADR-012)
- Analysis identity pins decode mode; queue adds filter at enqueue: ADR-007, ADR-013
- No first-party unsafe Rust: ADR-005; engine-owned data-dir lock: ADR-008
- Durable facts keyed by sampled content identity: ADR-019
- Output promotion owned as a journaled transaction: ADR-020

## Open questions

- Durable state model and storage engine for first-class History (#89)
- History observation and export contract: fields, budgets, consumers (#90, #92)
- Serving History by request/response instead of the frontend fold (#77, #78)
- Which estimates must improve over V2, and from which evidence stage (#57)
- One supervision kit for job cancellation and completion: ADR-018 proposed (#85)
- Whether ab-av1 can be used in-process, and the upstream lifecycle boundary it would need (#104)

Issue conventions: AGENTS.md "GitHub issues". Milestone: `v3.0`.
