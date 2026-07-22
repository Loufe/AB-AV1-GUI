# Analysis Pipeline

The Analysis view is a generation-scoped, ephemeral tree over durable media
facts. Discovery streams names without probing. A user-requested Basic Scan
then consumes the discovered file rows from the engine's native-path registry;
it never traverses the roots again and no projection or renderer touches the
filesystem.

## Basic Scan

`analysis_basic_scan(generation)` changes a `Discovered` or `Ready` generation
to `BasicScanning`. The engine snapshots the current ffprobe executable and
runs a fixed native-thread pool:

* worker count is `available_parallelism`, clamped to four through eight and
  then to the number of files;
* the job channel holds at most twice the worker count;
* each ffprobe has a 30-second deadline, a 1 MiB JSON head, and a 4 KiB
  diagnostic tail;
* cancellation drops unscheduled jobs, signals every running process group or
  Windows job, drains both pipes, and joins every worker before the generation
  runtime moves on;
* one file's failure becomes typed row state and does not fail its siblings.

Every worker first submits current path hash, destructive identity, timestamp
reliability, and import-path candidates to the core reducer. Core owns the
freshness decision. A reliable exact cache hit publishes the cached media facts
without another durable write. Unknown, coarse, recent, touched, replaced, or
unbound files go through supervised ffprobe and cancellable content sampling.
Identity is checked before probing, after probing, and after sampling; a change
at either boundary publishes no observation.

The successful observation command is atomic at the reducer boundary:

1. write `MediaObserved` when the path binding or content metadata changed;
2. resolve matching parked imports using the same helper as queue preparation;
3. write adoption or retirement deltas;
4. publish the row update after those durable deltas.

This order makes replay and the UI stream agree. Imported `Analyzed` facts stay
display-only provenance and never enter the reusable native analysis index.
When an observed artifact is a settled native output, imported provenance is
attached to the source-content relationship so its native verdict continues to
outrank the import and statistics do not count a second conversion.

Replace-mode recognition follows ADR-017. With a reliable timestamp, an exact
settled destructive identity is recognized before the stale source-path binding
and needs no ffprobe. Unknown/coarse/recent timestamps reobserve and recognize
the output only after stable probable-content comparison. A parked import also
forces that observation when the fast path has no trustworthy output metadata;
the implementation never substitutes source metadata for output metadata.

## Failure and privacy contract

File failures distinguish missing/unavailable input, timeout, tool rejection,
invalid or truncated JSON, process-supervision failure, change after probe, and
change during sampling. ffprobe diagnostics are bounded before they enter core
and the input path and filename are replaced with `[input]`. Starting a new
generation or cancelling the current one remains the primary cleanup action;
the reducer's generation check is the final correctness gate for any late
result.

## Implementation references

The design deliberately follows established bounded-work patterns:

* [Git's racy-stat documentation](https://git-scm.com/docs/racy-git) explains
  why matching size and timestamp cannot be trusted on coarse or recently
  written files and why ambiguous metadata falls back to content work.
* [Syncthing's scanner block queue](https://github.com/syncthing/syncthing/blob/5fa3d9f418c8bc2413d063970d501483939d325e/lib/scanner/blockqueue.go)
  uses fixed hashing workers, cancellation, and file checks around hashing;
  CRFty applies the same shape to probe plus sampled identity.
* Rust's [`sync_channel`](https://doc.rust-lang.org/std/sync/mpsc/fn.sync_channel.html)
  supplies backpressure rather than allowing an unbounded list of pending
  paths, and [`available_parallelism`](https://doc.rust-lang.org/std/thread/fn.available_parallelism.html)
  supplies the conservative host parallelism hint.
* [The Rust Book's thread-pool chapter](https://doc.rust-lang.org/stable/book/ch21-02-multithreaded.html)
  demonstrates fixed workers consuming shared queued jobs and joining them at
  shutdown.
* [ffprobe's JSON output documentation](https://ffmpeg.org/ffprobe.html)
  defines the machine-readable contract used here; human-oriented output is
  never parsed as media data.

Avoid unbounded task spawning, detached children or reader threads, size-only
cache hits, fake metadata for sparse imports, filesystem access in projections,
and row updates emitted before their durable observation/adoption facts.
