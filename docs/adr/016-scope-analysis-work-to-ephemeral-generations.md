---
status: accepted
date: 2026-07-21
---

# Scope Analysis Work to Ephemeral Generations

## Context and Problem Statement

The Analysis view must stream a large directory tree before probing finishes,
cancel obsolete work when the selected roots change, reject late worker
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

The complete ownership split is:

| State | Authority | Lifetime | Contents | Reconnect behavior |
| --- | --- | --- | --- | --- |
| Durable facts | Core `DurableState` | Journaled across restarts | Stable media observations, full path bindings, content records, native analyses, verdicts, runs, outputs, imported provenance | Included in `AppSnapshot` |
| Analysis standing model | Core `AppState.analysis` | Process-local, not journaled | Current generation id, activity, public row facts, future scan/applicability facts | Shell sends one complete `AnalysisDelta::Reset` immediately after `AppSnapshot` |
| Analysis execution | Engine generation registry | Current generation only | Untouched native `PathBuf`s, row allocation, cancellation source, pending work, permits, child processes | Not replayed; a process restart has no generation |
| Analysis reconnect mirror | Shell `StreamState.analysis` | Shell process | Exact fold of core Analysis deltas | Source of the complete Reset; never independently mutated |
| Analysis presentation | UI Analysis store | Webview lifetime | Normalized generated rows plus current activity | Snapshot clears it; the following Reset replaces it |
| Interaction state | UI components/store | Webview lifetime | Expansion, selection, sorting, columns, scroll | Reconciled by `(generation, row_id)`; never sent to core |

No projection or renderer performs filesystem access. Durable facts enter only
through durable commands; a row is a projection, not another durable fact.

The reducer allocates a monotonically increasing `AnalysisGenerationId` with
`begin_analysis_generation` when new roots or an explicit rescan supersede the
current generation. Callers never choose a generation. Every engine batch,
completion, failure, and row-targeted command carries the generation.
`apply_analysis_mutation` rejects a non-next Reset and every live mutation
that does not name the current generation. Cancellation is an optimization;
this reducer gate is the correctness boundary. #55 connects these primitives
to discovery commands and the driver registry.

| Event | Core transition | Engine action | Late/stale behavior |
| --- | --- | --- | --- |
| Select roots / explicit rescan | Allocate next id; Reset to `Discovering` and no rows | Create native registry and cancel the prior registry | Prior-generation work may finish but mutation is rejected |
| Discovery batch | Upsert rows for current id | Retain native paths under the same row ids | Unknown/stale generation is rejected |
| Discovery complete | `Discovering` to `Discovered` | Stop discovery workers | Repeated stale completion is rejected |
| Start Basic Scan | `Discovered`/`Ready` to `BasicScanning` | Acquire bounded probe permits | Vendor-busy or missing-tool request is rejected before transition |
| Scan batch | Upsert facts for current id after any source durable deltas | Retain/cancel supervised processes | Unknown/stale generation is rejected |
| Scan complete | `BasicScanning` to `Ready` | Release permits and child handles | Prior-generation completion is rejected |
| Cancel | Current activity to `Cancelled` | Signal generation cancellation and terminate supervised children | A later batch cannot change the cancelled/new generation |
| Webview reconnect | No core transition | None | Shell replays durable Snapshot, then complete Analysis Reset under one lock |
| Process restart | `AnalysisSnapshot::default()` | Registry starts empty | Durable facts survive; the directory tree is intentionally rediscovered |

Normal operation uses bounded `AnalysisDelta` batches. The driver and shell
fold those deltas into standing Analysis state. On subscription, the shell
emits the durable `AppSnapshot` first and then one Analysis replacement delta
containing the complete current ephemeral snapshot. No live delta can
interleave with that replay because subscription already holds the stream
lock. A process restart starts with no Analysis generation and discovers
again; journal replay never reconstructs the tree.

All Analysis deltas use the driver's post-durable position. Discovery-only
updates have no durable deltas, so the same ordering is harmless there. This
extends the existing `SessionAggregates` rule: a consumer never sees a
projection of a fact before the fact itself.

Starting a new generation cancels the prior driver-local generation and
immediately replaces its public state. Late results remain harmless even if
the underlying OS operation cannot be interrupted.

| Requested work | Conversion active | Basic Scan active | Vendor install/check active |
| --- | --- | --- | --- |
| Discovery | Allowed | Allowed for the same current generation | Allowed; it needs no media tool |
| Basic Scan | Allowed under its independent bounded permit pool | Idempotent/rejected if already active | Rejected |
| Conversion | Existing single conversion worker continues | Allowed; it never borrows Analysis state or permits | Existing vendor/session policy applies |
| Vendor install/check | Existing conversion policy applies | Rejected until scan stops/cancels | Serialized by existing vendor activity |

Current Analysis level and historical achievement are separate fields derived
by `assess_analysis_levels`; neither is persisted as a mutable flag.

| Current facts for the freshly selected `ContentKey` | Applicable level | Historical achievement |
| --- | --- | --- |
| No stable observation | `Discovered` | None, or the mapped parked import status |
| Stable record, no reusable native analysis/verdict | `Scanned` | At least `Scanned` |
| Adopted/parked imported `Analyzed` | Unchanged (`Discovered` or `Scanned`) | `Analyzed` |
| Adopted/parked imported `NotWorthwhile` | Unchanged | `Analyzed` |
| Adopted/parked imported `Converted` without an applicable content verdict | Unchanged | `Converted` |
| Native analysis satisfying the current reuse contract | `Analyzed` | `Analyzed` |
| Native or adopted `NotWorthwhile` verdict | Does not establish reusable `Analyzed` | `Analyzed`; its separate floor policy may still skip a conversion |
| Applicable Converted/Remuxed content verdict | `Converted` | `Converted` |

Historical achievement is the maximum of native analyses, verdicts, adopted
provenance, and any still-parked path summary. Imported analysis is never
inserted into `FileRecord.analyses`.

Native analysis reuse is exact except for the documented target relation:

| Input to reuse decision | Rule |
| --- | --- |
| Content identity | The record must be the one selected by the row's freshly observed probable `ContentKey` |
| Requested target | A result at or above the new requested target may satisfy it |
| Fallback result | A result below the requested target is reusable only for the identical requested target, floor, and step |
| Preset / maximum encoded percent | Exact match in `AnalysisProfile` |
| Sample count / sample duration / thorough mode | Exact match in `AnalysisProfile` |
| Actual decode mode | Exact decoder-granular match; software, CUVID, and QSV are distinct |
| ab-av1 / FFmpeg / encoder revision | Every revision must match exactly |
| Decode preference | Not separately compared; the actual decode mode above is the measurement identity |
| Overwrite/output settings | Do not affect a CRF-search measurement |
| `AnalysisIntent::Refresh` | Explicitly bypasses reuse even when the applicable level is `Analyzed` |

`AnalysisLevelAssessment` is the foundation contract; #57 adds it and the
prediction/confidence fields to streamed rows after Basic Scan facts land.

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

Implementation references:

* Rust's [`std::fs::read_dir`](https://doc.rust-lang.org/std/fs/fn.read_dir.html)
  documentation specifies that entry order is platform/filesystem dependent,
  may change between calls, and must be explicitly sorted for reproducible
  output. It also notes that advancing the iterator can independently fail.
* [`walkdir::WalkDir`](https://docs.rs/walkdir/latest/walkdir/struct.WalkDir.html)
  demonstrates the standard iterator-of-results error model and link-following
  controls, but its depth-first traversal does not provide this ADR's
  breadth-first row-allocation contract.
* [`ignore::WalkBuilder`](https://docs.rs/ignore/latest/ignore/struct.WalkBuilder.html)
  is useful prior art for non-followed links and iterator/visitor traversal.
  Its ignore-file, hidden-file, and glob semantics are intentionally not part
  of Analysis discovery.
* [ripgrep's deterministic-output discussion](https://github.com/BurntSushi/ripgrep/blob/master/FAQ.md#how-can-i-get-results-in-a-consistent-order)
  documents that its sorted output disables parallel traversal. Level 0 makes
  the same ordering-over-parallelism tradeoff because stable row IDs and
  batches are part of the public contract.
