---
status: accepted
date: 2026-07-21
---

# Project Imported History Separately

## Context and Problem Statement

ADR-012 established pure History, Statistics, and estimation projections, but
its proposed imported-history seam was incomplete. Imported records exist
before they acquire a content identity, several normalized paths can later
resolve to one content key, and imported analysis facts cannot safely become
reusable v3 analysis results because they lack a trusted profile, decode mode,
and tool revisions. Forcing these facts through `StatFact` would either invent
media facts or lose the path-level guards required to prevent re-import after
adoption and restart.

## Decision Drivers

* Preserve every consumed import-path key across adoption and restart
* Keep one content record when several imported paths identify the same bytes
* Surface honest sparse records without fabricating absent metadata
* Preserve exact Statistics totals across one-to-one adoption
* Define deterministic collision and codec tie rules
* Keep imported analysis display-only until a fresh v3 analysis exists
* Keep projections pure and native run totals native
* Make every new path-bearing durable field visible to privacy scrubbing (#51)

## Considered Options

* Project parked and adopted imported records through a dedicated accumulator,
  retain one deterministic summary on the content record, and guard every
  adopted path globally
* Store all imported summaries on each content record
* Convert imported records into native runs, analyses, and `StatFact`s
* Remove imported facts from Statistics and History at adoption

## Decision Outcome

Chosen option: **a dedicated imported-history projection path with a complete
global consumed-key set**, because it preserves sparse source facts and all
re-import guards while retaining the one-record-per-content invariant.

`DurableState.parked` contains unresolved `ImportedHistoryRecord`s by
`ImportPath`. Every successful adoption moves its path into
`DurableState.adopted_imports`; retirement removes a parked record without
consuming its path. `FileRecord.imported` retains one `ImportedProvenance` for
display and projection continuity. When several paths resolve to one content,
the retained summary is selected by newest `decided_at`, then status strength
(`Converted` > `NotWorthwhile` > `Analyzed` > `Scanned`), then the
lexicographically smaller normalized path. All paths still enter the global
guard set.

`ParkedAdopted` carries the imported facts and replay validation requires them
to equal the currently parked value. A fold could clone prior state before
removal, but carrying the facts makes the journal transition self-describing
and auditable. A native verdict always wins over an imported verdict.

Statistics has a private accumulator shared by native and imported inputs,
not a fabricated `StatFact`. Parked and adopted `Converted` records contribute
the facts they actually carry; both sizes are still required for size totals,
reduction bins, and cumulative savings. A known codec counts even when sizes
are absent. Imported `NotWorthwhile` contributes only its count, while
`Scanned` and `Analyzed` do not affect Statistics. Imported records never
increment native `RunTotals`. Codec counts sort by count descending, then the
typed codec's canonical ascending order; this deliberately replaces ADR-012's
equal-count content-insertion ordering.

History emits sparse rows keyed by either `Content(ContentKey)` or
`Parked(ImportPath)`. Parked `Scanned` records stay hidden; other parked and
adopted statuses remain visible with absent fields represented as `None`.
Imported `Analyzed` is historical display only: it is not inserted into
`FileRecord.analyses`, so the current Analysis view remains SCANNED and a
future Convert performs a fresh CRF search (#42).

Parked records never affect time estimation because they have no content
identity. Once adopted, a `Converted` verdict can supply an encoding-rate
sample through the ordinary `StatFact` path when codec, dimensions, duration,
and encoding time are sufficient. Imported `Analyzed` remains excluded.

One-to-one adoption into otherwise undecided content preserves Statistics
exactly. Native-verdict collisions and many-paths-to-one-content collisions
intentionally collapse to the single content record; their deterministic
outcome is tested instead of requiring impossible total preservation.

### Consequences

* Good: Re-import deduplication survives every many-path collision and restart
* Good: History and Statistics show imported facts before and after adoption
  without invented runs, analyses, containers, or audio metadata
* Good: The journal can validate exactly which historical facts moved
* Good: Collision and codec ordering no longer depend on input iteration order
* Bad: Imported data has a separate Statistics ingestion path alongside
  native `StatFact`s
* Bad: `FileRecord.imported` is a deterministic summary, not a complete audit
  list; the complete consumed-path history lives in `adopted_imports`
* Bad: Imported analyzed history may say Analyzed while the current Analysis
  model correctly offers only reusable SCANNED-level data

## More Information

This ADR supersedes ADR-012 in full while retaining its pure-projection,
ephemeral-Statistics, and Rust-oracle/TypeScript-fixture decisions. It corrects
the imported-history seam and makes the codec tie change explicit.

Path-bearing privacy surfaces for #51 are `DurableState.parked` keys,
`DurableState.adopted_imports`, `ImportedProvenance.import_path`,
`HistoryRowKey::Parked`, and journal deltas carrying import paths. See issues
#39, #42, #51, and #52, plus ADR-004 and ADR-012.
