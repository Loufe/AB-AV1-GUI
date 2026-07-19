---
status: accepted
date: 2026-07-19
---

# Embed a Pinned ab-av1 Adapter

## Context and Problem Statement

The Python application parses human-oriented ab-av1 subprocess output. V3 needs a
typed quality-search and encoding boundary with deterministic cancellation and
cleanup in a long-lived process.

## Decision Drivers

* Remove regex parsing and duplicate serialization of existing Rust types
* Receive typed search and encode progress
* Cancel, reap children, clean temporary state, and run another job safely
* Keep the upstream delta narrow and reviewable

## Considered Options

* Continue parsing human-readable subprocess output
* Use structured NDJSON subprocess output permanently
* Pin a minimally patched ab-av1 revision as a library dependency

## Decision Outcome

Chosen option: **Pin a minimally patched ab-av1 library adapter**, because the
real-process lifecycle proof demonstrated typed progress, cancellation, cleanup,
and successful second-job recovery.

The engine permits only one active ab-av1 job. The exact dependency revision and
patch remain adapter-private. Structured NDJSON is retained only as a contingency
if native platform containment cannot satisfy the lifecycle contract.

### Consequences

* Good: Upstream changes surface as compile-time integration work
* Good: Application code consumes typed events rather than output text
* Bad: CRFty maintains a small patch until suitable interfaces land upstream

## More Information

See issue #33, sections 2, 3, and 9.
