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
- Every V2 statistic survives as a V3 consumer, with richer evidence where collection allows; the consumer matrix and field dispositions are frozen (`docs/design/history-consumer-matrix.md`)
- Collection budgets are development-time acceptance thresholds, never runtime mechanisms. A collector ships only when it measures under 5 percent of the search it informs on the benchmark corpus. Collection is event-driven, with no periodic resource sampling and no media or network I/O of its own; scan and startup collection add no process and no file read, and the compatibility probe waits for first claim (`docs/design/history-collection-budgets.md`)
- Durable quality targets and scores carry a metric tag, VMAF the sole initial variant; the metric joins the analysis identity and aggregates never blend metrics: ADR-022
- The durable model gains per-stream audio bitrate and first-observed and last-updated stamps, and the not-worthwhile verdict embeds its highest-saving measurement (`docs/design/history-consumer-matrix.md`)
- History browsing guardrail: first paint under 100 ms at 50,000 records, a regression tripwire rather than an engine-forcing constraint
- Failed and stopped runs are browsable behind default-off History filters and excluded from Statistics aggregates; deliberately throttled runs carry a provenance flag and never enter unthrottled time cohorts
- Export and the portable bundle ship post-3.0 while import stays in V3; pooled research over contributed bundles is the bundle's long-term consumer (`docs/design/history-bundle.md`)
- Runnable intermediate: durable observations for every terminal outcome, History browsing, and import v1; 3.0-complete adds Statistics, the failed and stopped filters, wired scrub, and enriched import
- Estimation verdicts: point uncertainty through the presentation ramp, the kernel-weighted quantile successor built first, imported V2 evidence as a down-weighted cold-start prior, and size estimation on a video-stream basis (`docs/design/estimation.md`)

## Open questions

- Durable state model and storage engine for first-class History
- Serving History by request/response instead of the frontend fold
- Estimation as a subsystem separate from History: sufficiency, promise level, evidence seam, evaluation (`docs/design/estimation.md`)
- Whether the ab-av1 maintainer accepts the operation boundary ADR-021 selects (alexheretic/ab-av1#371)

Issue conventions: AGENTS.md "GitHub issues". Milestone: `v3.0`.
