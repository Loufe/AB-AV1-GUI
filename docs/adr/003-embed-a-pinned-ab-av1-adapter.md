---
status: accepted
date: 2026-07-19
---

# Embed a pinned ab-av1 adapter

## Context and problem statement

The Python application parses human-oriented ab-av1 subprocess output. V3 needs a typed quality-search and encoding boundary with deterministic cancellation and cleanup in a long-lived process.

## Decision drivers

* Remove regex parsing and duplicate serialization of existing Rust types
* Receive typed search and encode progress
* Cancel, reap children, clean temporary state, and run another job safely
* Keep the upstream delta narrow and reviewable

## Considered options

* Continue parsing human-readable subprocess output
* Use structured NDJSON subprocess output permanently
* Pin a minimally patched ab-av1 revision as a library dependency
* Abstract every encoder behind one integration trait so future backends share a single mechanism (rejected: speculative generality, and the second backend does not exist yet)

## Decision outcome

Chosen option: **Pin a minimally patched ab-av1 library adapter**, because it provides typed progress and errors while making upstream changes compile-time integration work.

The initial real-process prototype demonstrated typed progress, cancellation, cleanup, and successful second-job recovery for the path it exercised. It did not cover the detached sample producer or the unmanaged sample-copy FFmpeg path; ADR-021 owns the complete operation-lifecycle boundary.

The engine permits only one active ab-av1 job. The exact dependency revision and patch remain adapter-private. Structured NDJSON is retained only as a contingency if native platform containment cannot satisfy the lifecycle contract.

### Consequences

* Good: Upstream changes surface as compile-time integration work
* Good: Application code consumes typed events rather than output text
* Bad: CRFty maintains a small patch until suitable interfaces land upstream

## More information

### Error discrimination: what a subprocess boundary can actually report

ab-av1's `main.rs` collapses every failure to exit code 1, so an exit code cannot separate "no suitable CRF" from an encoder crash or a missing input. Structured output exists only for `sample-encode` (`--stdout-format json`); `crf-search`, `auto-encode`, and `encode` have none. The one dependable signal is the final `Error: ...` line, which is the top-level error's `Display` rather than incidental log text. `NoGoodCrf` renders as exactly "Failed to find a suitable crf", byte-identical from v0.4.0 through v0.11.4. A subprocess boundary therefore yields exactly one trustworthy discrimination, obtained by comparing a sentence.

That one discrimination is the one the application needs, because it is the not-worthwhile verdict and the VMAF fallback trigger. An audit of the Python callers found it was also the only exception any caller branched on; the rest of the regex-derived hierarchy changed log wording and nothing else. Under the library adapter the same fact arrives as a typed variant matched in a `match` arm (`crf_search::Error::NoGoodCrf { last }`, carrying the last search outcome), which is the concrete payoff of this record.

### Upstream and patch-fork strategy

The dependency is a fork pinned to an exact reviewed commit and built with a `library` feature, so updating it is an application rebuild and upstream API changes surface as compile failures. The reviewable delta is proposed upstream where generally useful. It provides a library target without CLI completion wiring, a typed encode-update stream lifted from the internal FFmpeg event loop, and a per-job cancellation handle with graceful and force modes. MIT licensing permits the fork; attribution and license text ship with the application. Only one ab-av1 job runs at a time because upstream's child and temp registries are process-global.

### The NDJSON contingency, and what it would still cost

Upstream PR 368 (`crf-search --stdout-format json`) is the contingency's prerequisite and remains tracked upstream. The planned V2 cutover to that stream was abandoned because its only consumer would have been the Python parser. Two upstream deferrals bound the contingency even after PR 368 lands. Encode-update messages are a later PR, so encode progress would still come from human FFmpeg output. Structured error reasons were also deferred, leaving failure detection as the exit code plus the "Failed to find a suitable crf" string. Upstream issue 369 (`sample-encode-update`) no longer matters to the rewrite because the typed `crf_search::Update` stream already carries sample status.

### Boundary for future backends

This record binds the ab-av1 adapter, not every future encoder. No ab-av1 argument or result type leaves the adapter; the driver speaks only application-owned job requests, coarse phases, progress, checkpoints, and terminal outcomes. A future backend's lifecycle determines its integration. Prefer a crate with a repeatable typed lifecycle, programmatic cancellation, and no uncontrolled process-global state. Prefer a contained subprocess when it owns a complex descendant tree, carries significant unsafe or FFI code, calls `process::exit`, or exposes a stable structured protocol. Regex scraping is never the price of subprocess isolation.

Related: ADR-005 (unsafe policy) and ADR-010 (the FFmpeg binaries this adapter drives). The job runtime, cancellation contract, and output-promotion transaction are in `docs/design/lifecycle.md`; the hazards this boundary carries into the port are in `docs/design/porting-hazards.md`.
