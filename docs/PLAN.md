# Rewrite plan

Living state of the V3 rewrite. Update when a phase lands or a direction is decided. Decisions get an ADR; long-form design goes in `docs/design/`.

## Current phase

Engine and queue foundations are complete and contract-tested: pinned ab-av1 adapter, durable job coordinator, journal replay and crash recovery, the queue command surface, the bounded event stream, the Tauri shell, and the UI fold over golden fixtures. Current work is growing the views over the store: History, Statistics, and the final Queue integration. Analysis has generation-scoped streaming discovery and the bounded Basic Scan pipeline (`docs/ANALYSIS.md`); later tiers add estimates, actions, and presentation.

## Decided

- Three-crate split (core/engine/shell) with one state owner: ADR-001, ADR-002
- Append-only journal, snapshot-head compaction, generation-identity corruption handling: ADR-004, ADR-009, ADR-011
- Pinned ab-av1 adapter; vendored, checksummed FFmpeg: ADR-003, ADR-010
- IPC bindings generated from Rust via tauri-specta: ADR-006
- History and Statistics derive as pure projections, with imported history projected separately: ADR-015
- Analysis identity pins decode mode; queue adds filter at enqueue: ADR-007, ADR-013
- Analysis work is scoped to ephemeral generations and its paths stay engine-native: ADR-016, ADR-017
- Path scrubbing happens inside the log sink: ADR-014
- No first-party unsafe Rust: ADR-005; engine-owned data-dir lock: ADR-008
- Durable facts keyed by sampled content identity: ADR-019
- Output promotion owned as a journaled transaction: ADR-020
- One supervision kit for job cancellation and completion, not yet implemented: ADR-018
- The caller-driven owned ab-av1 operation, pending upstream: ADR-021
- Every V2 statistic survives as a V3 consumer, with richer evidence where collection allows; collection budgets are policy caps, expressed as a share of the search a collector informs, with absolute targets only for Basic Scan, startup, and browsing
- History browsing guardrail: first paint under 100 ms at 50,000 records, a regression tripwire rather than an engine-forcing constraint
- Failed and stopped runs are browsable behind default-off History filters and excluded from Statistics aggregates; deliberately throttled runs carry a provenance flag and never enter unthrottled time cohorts
- Export and the portable bundle ship post-3.0 while import stays in V3; pooled research over contributed bundles is the bundle's long-term consumer (`docs/design/history-bundle.md`)
- Runnable intermediate: durable observations for every terminal outcome, History browsing, and import v1; 3.0-complete adds Statistics, the failed and stopped filters, wired scrub, and enriched import
- Estimation verdicts: point uncertainty through the presentation ramp, the kernel-weighted quantile successor built first, imported V2 evidence as a down-weighted cold-start prior (`docs/design/estimation.md`)

## Open questions

- Durable state model and storage engine for first-class History
- History observation contract: field selection and the consumer-to-evidence matrix
- Serving History by request/response instead of the frontend fold
- Estimation as a subsystem separate from History: sufficiency, promise level, evidence seam, evaluation (`docs/design/estimation.md`)
- Whether the ab-av1 maintainer accepts the operation boundary ADR-021 selects (alexheretic/ab-av1#371)

Issue conventions: AGENTS.md "GitHub issues". Milestone: `v3.0`.
