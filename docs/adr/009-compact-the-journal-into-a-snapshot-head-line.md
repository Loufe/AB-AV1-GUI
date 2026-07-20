---
status: accepted
date: 2026-07-20
---

# Compact the Journal Into a Snapshot Head Line

## Context and Problem Statement

The append-only journal (ADR-004) grows without bound: long-lived installs
accumulate dead upserts and startup replay cost. Compaction must rewrite live
state through a crash-safe writer barrier without breaking replay identity,
recovery semantics, or the runtime-id derivation that depends on sequence
numbering. The journal format also had no room to evolve: the version rode
inside each delta envelope, so a record of any future shape failed as a parse
error — indistinguishable from corruption — instead of "unsupported schema".

## Decision Drivers

* A compacted journal must replay to exactly the state the old one folded to
* Sequence numbering must continue across compactions (recovery identity,
  runtime-id derivation)
* Torn-tail and semantic-corruption detection must survive the format change
* A future schema version must fail distinctly, not as corruption
* A failed compaction must never lose data or take the driver down
* A running conversion is never interrupted (#33 §10)
* No new dependencies

## Considered Options

* Snapshot in a sidecar file, then truncate the journal
* In-place rewrite guarded by a marker record
* Every line a version-tagged record; compaction atomically replaces the file
  with a single snapshot head line
* Per-record checksums for corruption detection

## Decision Outcome

Chosen option: **version-tagged lines with an atomic snapshot-head replace**.
Every journal line is `{schema_version, record}` where the record is either
`Deltas` (a sequenced batch) or `Snapshot` (folded state stamped with app
version, timestamp, and the base sequence the next batch must carry). Replay
probes the version alone before decoding the record, so an unknown schema
reports "unsupported journal schema" instead of a parse error. A snapshot is
only legal as the first line; one appearing later is semantic corruption.

Compaction runs on the driver's idle tick — the writer barrier is implicit
because the driver is the only writer and sits between batches — and only when
the state is quiescent (idle session, no reserved or claimed queue item). The
size policy fires at a 64 MiB floor combined with a 4× dead-to-live ratio
(Redis-AOF-style), or unconditionally at a 256 MiB hard cap; durable
transforms (scrub, corruption acknowledgment, adoption) can force it. The
writer encodes the snapshot to a temp file in the journal directory, fsyncs
it, closes the old journal handle, atomically replaces the journal (with a
bounded retry for Windows sharing violations), fsyncs the parent directory
where supported, and reopens. Sequence numbering continues from the snapshot's
base sequence. On any failure the temp file is discarded, the old generation
stays authoritative, and the driver retries after a backoff — never a fatal.

A single-file replace was chosen over a sidecar-plus-truncate because it keeps
exactly one authoritative artifact with no cross-file recovery protocol.
Per-record checksums were rejected: JSON-parse and semantic-fold validation
already classify every observed failure mode, and the zero-backcompat policy
makes adding checksums later a cheap schema bump.

### Consequences

* Good: Replay of `snapshot + tail` is provably identical to the original
  journal plus tail, and restart cost is bounded
* Good: Future format changes degrade to a typed "unsupported schema" state
* Good: Crash at any point leaves either the old or the new generation intact;
  a stray temp file is inert
* Bad: Compaction waits for quiescence, so a machine that never idles between
  work can exceed the size targets until the next barrier
* Bad: The snapshot line duplicates fold logic's trust — a bug that folds bad
  state would be baked into the compacted head (mitigated by semantic replay
  validation before any compaction)

## More Information

See issue #33 section 10, issue #39, ADR-002, ADR-004, and ADR-008.
