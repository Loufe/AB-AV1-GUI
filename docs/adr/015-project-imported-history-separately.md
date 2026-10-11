---
status: accepted
date: 2026-07-21
---

# Project imported history separately

## Context and problem statement

History, Statistics, and estimation are pure projections, but imported records need a dedicated projection path. Imported records exist before they acquire a content identity, several normalized paths can later resolve to one content key, and imported analysis facts cannot safely become reusable V3 analysis results because they lack a trusted profile, decode mode, and tool revisions. Treating these summaries as native observations would invent evidence and lose the distinction between imported facts and assessed native measurements.

## Decision drivers

* Preserve every consumed import-path key across adoption and restart
* Keep one content record when several imported paths identify the same bytes
* Surface honest sparse records without fabricating absent metadata
* Preserve exact Statistics totals across one-to-one adoption
* Define deterministic collision and codec tie rules
* Keep imported analysis display-only until a fresh v3 analysis exists
* Keep projections pure and native run totals native
* Make every new path-bearing durable field visible to privacy scrubbing

## Considered options

* Project parked and adopted imported records through a dedicated accumulator, retain one deterministic summary on the content record, and guard every adopted path globally
* Store all imported summaries on each content record
* Convert imported records into native runs and assessed facts
* Remove imported facts from Statistics and History at adoption

## Decision outcome

Chosen option: **a dedicated imported-history projection path with a complete global consumed-key set**, because it preserves sparse source facts and all re-import guards while retaining the one-record-per-content invariant.

`DurableState.parked` contains unresolved `ImportedHistoryRecord`s by `ImportPath`. Every successful adoption moves its path into `DurableState.adopted_imports`; retirement removes a parked record without consuming its path. `FileRecord.imported` retains one `ImportedProvenance` for display and projection continuity. When several paths resolve to one content, the retained summary is selected by newest `decided_at`, then status strength (`Converted` > `NotWorthwhile` > `Analyzed` > `Scanned`), then the lexicographically smaller normalized path. All paths still enter the global guard set.

`ParkedAdopted` carries the imported facts and replay validation requires them to equal the parked value. A fold could clone prior state before removal, but carrying the facts makes the journal transition self-describing and auditable. A native verdict always wins over an imported verdict.

Statistics has a private accumulator shared by native Statistics facts and imported facts. Parked and adopted summaries pass through the same imported-fact construction as translated observations. `Converted` records contribute the facts they actually carry; both sizes are still required for size totals, reduction bins, and cumulative output-size reduction. A known codec counts even when sizes are absent. Imported `NotWorthwhile` contributes only its count, while `Scanned` and `Analyzed` do not affect Statistics. Imported records never increment native `RunTotals`. Codec counts sort by count descending, then the typed codec's canonical ascending order, independent of content insertion order.

History emits sparse rows keyed by either `Content(ContentKey)` or `Parked(ImportPath)`. Parked `Scanned` records stay hidden; other parked and adopted statuses remain visible with absent fields represented as `None`. Imported `Analyzed` is historical display only: it is not inserted into `FileRecord.analyses`, so the current Analysis view remains SCANNED and a future Convert performs a fresh CRF search.

Neither parked nor adopted imported summaries supply native time-estimation samples. Their missing phase assessment and execution profile do not become known at adoption. Estimation reads the observation rate accessor, which admits only qualified native phases. A separately weighted imported prior belongs to the successor estimator described in `docs/design/estimation.md`.

One-to-one adoption into otherwise undecided content preserves Statistics exactly. Native facts and the retained imported summary contribute independently, even when they describe the same content record. Several imported paths resolving to one content still retain only the selected imported summary, so that collision changes totals deterministically. Operational verdict selection remains separate from Statistics eligibility.

### Consequences

* Good: Re-import deduplication survives every many-path collision and restart
* Good: History and Statistics show imported facts before and after adoption without invented runs, analyses, containers, or audio metadata
* Good: The journal can validate exactly which historical facts moved
* Good: Collision and codec ordering no longer depend on input iteration order
* Bad: Imported data has a separate Statistics ingestion path alongside assessed native facts
* Bad: `FileRecord.imported` is a deterministic summary, not a complete audit list; the complete consumed-path history lives in `adopted_imports`
* Bad: Imported analyzed history may say Analyzed while the current Analysis model correctly offers only reusable SCANNED-level data

## More information

History, Statistics, and estimation remain pure projections. Statistics stays ephemeral, and the Rust oracle remains synchronized with TypeScript through fixtures.

ADR-024 fixes one immutable observation per terminal run as the unit of History, and ADR-025 identifies translated observations by their V2 record key without adoption. This separate imported path is replaced once translated observations are stored under those records; until then it describes the shipped parked path.

The path-bearing privacy surfaces are `DurableState.parked` keys, `DurableState.adopted_imports`, `ImportedProvenance.import_path`, `HistoryRowKey::Parked`, and journal deltas carrying import paths. See ADR-004.
