# Rewrite plan

Living state of the V3 rewrite. Decisions get an ADR; long-form design goes in `docs/design/`. GitHub issue bodies own active scope and execution order. The [alpha work](https://github.com/Loufe/AB-AV1-GUI/issues?q=is%3Aissue%20is%3Aopen%20label%3Aalpha) identifies the acceptance issue and required deliverables within the `v3.0` milestone.

## Current phase

The branch has the pinned ab-av1 adapter, job coordinator, journal recovery, queue commands, Tauri shell, and frontend stores with contract coverage. Queue, History, Statistics, and Settings have production views over the current model. Analysis has streaming discovery and bounded Basic Scan in the engine (`docs/ANALYSIS.md`), but its production view remains a disabled empty state. Known adapter-internal probe cancellation, Windows spawn cleanup, and terminal identity defects remain; existing coverage does not establish alpha readiness.

The delivery target is the runnable alpha defined in [the alpha delivery scope](design/alpha.md). It combines a usable Analysis-to-Queue workflow with durable terminal observations, History browsing, import v1, and clean Windows and Linux installation. First-class History is still a redesign of the current projections. Historical estimation and richer collectors remain V3 work beyond alpha.

## Decided

- Three-crate split (core/engine/shell) with one state owner: ADR-001, ADR-002
- Append-only journal, snapshot-head compaction, generation-identity corruption handling: ADR-004, ADR-009, ADR-011
- Pinned ab-av1 adapter; user-supplied FFmpeg verified by a capability probe: ADR-003, ADR-023
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
- Runnable alpha: the workflow and acceptance boundary in `docs/design/alpha.md`; 3.0-complete adds Statistics over first-class observations, the failed and stopped filters, wired scrub, and enriched import
- Estimation verdicts: point uncertainty through the presentation ramp, the kernel-weighted quantile successor built first, imported V2 evidence as a down-weighted cold-start prior, and size estimation on a video-stream basis (`docs/design/estimation.md`)

## Open questions

- Durable state model and storage engine for first-class History
- The bounded History request/response shape and its invalidation contract; serving History from Rust is the selected direction
- Estimation as a subsystem separate from History: sufficiency, promise level, evidence seam, evaluation (`docs/design/estimation.md`)
- Whether the ab-av1 maintainer accepts the operation boundary ADR-021 selects (alexheretic/ab-av1#371)

Issue conventions: AGENTS.md "GitHub issues". Milestone: `v3.0`.
