---
status: proposed
date: 2026-07-21
---

# Scope Analysis Work to Ephemeral Generations

## Context and Problem Statement

The Analysis view must stream a large directory tree before probing finishes,
cancel obsolete work when the selected root changes, reject late worker
results, and recover coherently when the webview reconnects. Its rows are a
working view over filesystem observations and durable content facts, not new
durable facts themselves. Putting the tree in the journal would persist stale
filesystem topology; keeping it only in React would let engine work and UI
state diverge.

The application already has one ordered stream and reducer-owned standing
ephemeral state for the conversion session, tools, aggregates, and telemetry.
Analysis needs the same ownership discipline without sending the complete tree
after every incremental batch.

## Decision Drivers

* Obsolete discovery and probe workers must be unable to mutate a newer view
* Reconnect must restore one coherent current tree without replaying missed
  transient batches
* Incremental batches must remain bounded; full-tree replacement per batch is
  unacceptable
* Durable observations and their Analysis presentation must appear in causal
  order on the stream
* Filesystem paths, cancellation handles, workers, and child processes must
  remain outside pure core state
* UI expansion, selection, sorting, and scroll position must not become domain
  state

## Considered Options

* Persist the Analysis tree in `DurableState`
* Keep the tree and generation entirely in the frontend
* Keep the generation in the engine and trust workers to check cancellation
* Own a foldable ephemeral Analysis model in core and mirror it in the shell
* Publish a complete Analysis snapshot after every batch

## Decision Outcome

Chosen option: **own a foldable ephemeral Analysis model in core, scoped by a
reducer-allocated generation, and mirror it in the shell**, because this keeps
the reducer as the final stale-result guard while allowing the engine and UI to
consume bounded incremental deltas.

The ownership split is:

* Durable core state keeps only established media observations, content
  records, analyses, verdicts, runs, and imported provenance.
* Ephemeral core state keeps the current Analysis generation, phase, public
  row facts, progress, and per-row failures. It is never journaled and resets
  on process restart.
* Driver-local state keeps each generation's native `PathBuf` registry,
  cancellation source, pending work, concurrency permits, and child-process
  handles.
* UI-only state keeps expansion, selection, sorting, column layout, and scroll
  position.

The reducer allocates a monotonically increasing `AnalysisGenerationId` when a
new root or explicit rescan supersedes the current generation. Callers never
choose a generation. Every engine batch, completion, failure, and row-targeted
command carries the generation. The reducer accepts a worker mutation only
when it names the current generation; cancellation is an optimization, while
generation validation is the correctness boundary.

Normal operation uses bounded `AnalysisDelta` batches. The driver and shell
fold those deltas into standing Analysis state. On subscription, the shell
emits the durable `AppSnapshot` first and then one Analysis replacement delta
containing the complete current ephemeral snapshot. No live delta can
interleave with that replay because subscription already holds the stream
lock. A process restart starts with no Analysis generation and discovers
again; journal replay never reconstructs the tree.

Analysis deltas that summarize newly written durable facts are emitted after
their durable deltas. Discovery-only and progress deltas may use the ordinary
ephemeral position. This extends the existing `SessionAggregates` ordering
rule: a consumer never sees a projection of a fact before the fact itself.

Starting a new generation cancels the prior driver-local generation and
immediately replaces its public state. Late results remain harmless even if
the underlying OS operation cannot be interrupted. Basic Scan starts only
when ffprobe is available and vendor activity is idle. Discovery may coexist
with a conversion session. Basic Scan may coexist with conversion under its
own bounded worker limit; it must not borrow or mutate the conversion
worker's active-job state. Vendor replacement cannot start while Basic Scan is
active, and Basic Scan cannot start during vendor replacement.

Current Analysis level and historical achievement are separate concepts. A
row may expose historical imported `Analyzed` provenance while its current
applicable level remains `SCANNED`. Only a native analysis that passes the
existing exact reuse policy can establish current `ANALYZED`.

### Consequences

* Good: Cancellation races cannot leak stale rows across generations
* Good: Reconnect receives a complete current view without journaling it
* Good: Discovery and probing remain incremental and bounded
* Good: The UI never owns domain freshness or level decisions
* Good: Durable observations precede the rows that summarize them
* Bad: Core, shell, and frontend each need a small pure Analysis fold
* Bad: The shell retains another standing ephemeral model for reconnect
* Bad: Restart intentionally discards in-progress discovery and scanning

## More Information

See issues #42, #53, #55, #56, #57, and #59; ADR-002, ADR-004, ADR-006,
ADR-007, and ADR-012. ADR-015 is reserved by #52 for imported-history
projection and provenance decisions.
