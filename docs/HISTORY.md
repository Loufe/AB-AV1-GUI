# History

History is the durable record of what conversion work actually happened, and the evidence that Statistics and estimates are computed from.

This document states only the rules that are settled and verified against the shipped core today. It is not yet the full logical contract, and the storage model behind it is not decided here.

The [source-continuity contract](design/source-continuity.md) defines required alpha changes to evidence eligibility and retained artifacts. The shipped model does not yet classify source mutation across search and conversion. Its completion sizes and fallback projections must not be read as proof that the source stayed unchanged during processing.

## History reports outcomes; it does not decide them

A History entry describes work that reached a terminal outcome: a conversion, a remux, a not-worthwhile judgment, a completed analysis, or a failed, stopped, or incomplete run. Content that was only scanned has nothing to report and gets no entry. A skipped run decided nothing and gets no entry either; its reason belongs to the queue item that was skipped.

Nothing is stored under the name History. History rows and Statistics are context-free projections of durable state, recomputed on request with no clock, no filesystem, and no cache, per ADR-015, project imported history separately.

Facts flow one way. The analysis level a file stands at, the analysis it may reuse, and its eligibility for the queue are all decided from current freshness-checked evidence about the file on disk, never from History:

- Analysis level is split at the type level. An Analysis row's `AnalysisRowStatus` reports `applicable`, meaning what a claim would actually reuse under the current tools and execution settings, alongside `historical`, meaning the highest tier known to have been reached. Historical achievement, including an imported Converted or Analyzed summary, raises only `historical` (`docs/ANALYSIS.md`, Row contract).
- A historical analysis is not a reusable analysis. Analysis identity is profile-exact per ADR-007, pin decode mode in analysis identity, so only a native search recorded under a permitted profile can be selected for reuse. An imported Analyzed fact is display-only and never enters `FileRecord.analyses`.
- Queue eligibility reads the path binding, the live destructive identity, timestamp reliability, and the standing verdict, per ADR-013, filter queue adds at enqueue. It consults neither History rows, nor Statistics, nor the parked import inbox.

Estimation is the one sanctioned consumer in the other direction: completed phase spans and settled sizes feed the cohorts behind size and time predictions. It reads History and never writes to it; its model, evidence seam, and open questions are in `docs/design/estimation.md`.

One seam does run from historical records into current state, and it is deliberately narrow. An imported record adopts onto a content record only when a fresh observation confirms the file at its recorded path. Confirmation requires either matching size and modification time or the replace-mode case where the file at that path is already the AV1 output. Adoption is the one-time route for V2 history and is governed by ADR-015; a record that no longer describes the file retires and decides nothing.

## One immutable observation per terminal run

The unit of History is the observation, fixed by ADR-024 and implemented as `crfty_core::Observation`. One observation describes one run that reached a terminal outcome. Its identity is the run identifier, which is never reused. It names at most one source by content key, and once recorded it does not change: a retry, a changed source, or a later success on the same file is a new observation. Today the observation is derived from the run, record, and output ledgers on request; the same type becomes the stored unit once History has its own storage.

Each outcome carries only the evidence it can honestly hold. Everything below is typed, and a shape outside this table does not deserialize.

| Outcome | Always present | Present when known |
| --- | --- | --- |
| Analyzed | Search evidence: the analysis with its predictions | Search duration, source assessment |
| Converted | Search evidence and encode evidence; a live encode carries both sizes and its decode mode | Encode duration, recovered sizes, output content key, assessments |
| Remuxed | Remux evidence; a live remux carries both sizes | Recovered sizes, output content key, assessment |
| Not worthwhile | Requested target, floor, and at least one fallback attempt | Each attempt's last measurement |
| Failed | Failure kind, message, and bounded diagnostic | |
| Stopped, Incomplete | Nothing beyond the run facts | |

Every observation also carries its operation, the toolchain revisions of its execution profile, and, when the source was identified, the source's content key and media facts. Start and finish instants are optional, and an unknown instant stays unknown rather than being filled in from another clock.

Three different things are called attempts, and the contract keeps them apart. Quality-target fallback attempts belong to one run's search: the analysis lists the targets that failed, and a not-worthwhile outcome lists every attempt. The hardware-to-software decode retry belongs to the encode: the live measurement records the decode mode the encode actually ran with, which the search profile may not match. A queue retry mints a new run and therefore a new observation; lineage is derived from the content key and run order and never stored.

A file's standing is the latest decisive observation for its content, where converted, remuxed, and not-worthwhile outcomes decide and the others do not. Standing is computed on request and never written back. Aggregates that count files dedupe decisive observations by content key, because one file may honestly hold several conversions after its source changed.

### Eligibility is decided once

Consumers never read run fields directly. Each obtains facts through a typed accessor on the observation that yields a value only when the observation qualifies for that purpose, and estimation decides weighting over the observations History admits. Failed, stopped, and incomplete observations are browsable evidence and yield nothing from any accessor.

| Accessor | Yields when |
| --- | --- |
| Decisive fact | Converted, remuxed, or not worthwhile with an identified source; no assessment required |
| Reduction fact | Converted or remuxed with both sizes measured and the producing phase assessed as matched |
| Analyze rate sample | Analyzed or converted with a positive search duration, a positive source duration, and the search assessed as matched |
| Convert rate sample | Converted with a positive encode duration, a positive source duration, and the encode assessed as matched |
| Prediction pair | Converted with measured sizes and encode duration, and both search and encode assessed as matched |

Source assessment uses the classes of the [source-continuity contract](design/source-continuity.md). The shipped engine does not yet produce one, so every recorded observation is unassessed, and unassessed evidence does not qualify for aggregates or prediction pairs. Statistics and estimation still read the older per-content projections until they move onto these accessors.

## Interrupted runs and preparation failures

Stopped means a user requested Force Stop and cleanup allowed that outcome to be recorded. Incomplete means a prepared run had no recorded terminal when startup recovery examined it. This can follow a crash, a persistence failure, or ordinary shutdown during active work. The recovery timestamp records when the outcome was decided, not when processing ended; recovery adds no measured phase durations.

Output evidence takes precedence: a successfully settled output reports recovered conversion or remux, and a conflict reports failure. Other prepared work reports Incomplete only once output is absent or safely abandoned. Missing tools leave unsettled output deferred. Saved analysis survives an incomplete run, but an analysis attached from an earlier run cannot prove that this run completed. An existing standing verdict still takes precedence in the content History row.

A reservation has no prepared run. Startup returns it to its original queue position without creating an outcome. A known preparation rejection instead records a failed queue item with its reason. That failure remains visible until explicit queue removal or retry, but it has no independent History observation or run-based Statistics entry. A durable identity high-water mark prevents ID reuse; it does not retain the failure as a separate observation.

## Predictions and measurements are separate facts

A quality search produces predictions. `SearchMeasurement` carries `predicted_size`, `predicted_percent_basis_points`, and `predicted_duration_ms` next to the CRF and VMAF score it settled on, and the observation holds it as search evidence.

A finished encode produces measurements. `CompletionEvidence::LiveEncode` carries input and output sizes verified against the settled output transaction, and the observation holds them as encode evidence.

Both persist, and neither replaces the other. A prediction is never overwritten, nulled out, or reconciled when its outcome arrives, because holding the pair is the only way to learn what the prediction was worth.

No consumer may present a prediction as a measurement. The distinction lives in the field names and the types rather than in a convention readers are asked to remember.

## Sizes are measured or absent

Byte sizes reach History from verified identities only. An observation takes its sizes from live completion evidence, or for a crash-recovered success from the promoted artifact identity in the output transaction, and otherwise holds none. The older per-content projections additionally fall back to the summary from an adopted import and to the record's inspected input size while the output stays unknown.

Human-readable FFmpeg output is not a size source. The stream summaries FFmpeg prints when an encode ends are display text rounded to a unit FFmpeg chose, and the engine does not treat human-oriented process output as an application contract. Such a figure must never be stored as a size, and a value derived from one must never be presented as a measurement.

An unknown size is absent, not zero. Absence and zero are different claims, and a size that was never established must not be allowed to become the second one. Absence is carried through the projections intact: a converted fact whose sizes are not both known still counts as converted, and contributes nothing to output-size reduction totals, reduction bins, or the cumulative series.

## Pathless is pseudonymous, not anonymous

A History record stripped of readable paths remains linkable. The identities that survive are pseudonyms rather than erasures: a content key is a BLAKE2b digest over sampled file bytes plus the media header, and a path hash is a BLAKE2b digest over the canonicalized path. Anyone holding the same file recomputes its content key exactly, and anyone holding a list of candidate paths can recompute path hashes and match them.

Exports and disclosures must say so plainly. A bundle without readable paths is described as pseudonymous, never as anonymous and never as de-identified. Its technical facts, exact tool revisions and resolutions and durations and sizes and timings, are ordinary one at a time and can be distinctive in combination, so the linkability a bundle admits belongs in its own disclosure rather than in a footnote elsewhere.

## Scrub is a logical transform, not erasure

Scrubbing removes readable path data from the current records and leaves everything else standing. Pseudonymous identity and every technical and statistical fact survive it, because the goal is to drop the readable path without weakening the evidence. ADR-015 enumerates the path-bearing durable surfaces a scrub has to cover.

Scrubbing is idempotent. Scrubbing an already-scrubbed store changes nothing, and a scrubbed store is a valid store rather than a degraded one.

Scrubbing is not erasure, and must never be described to users as if it were. State persists through an append-only journal (ADR-004, persist state in an append-only journal) compacted by atomic replacement with a snapshot head line (ADR-009, compact the journal into a snapshot head line). Nothing below the application is asked to forget bytes it already wrote, including the filesystem and any underlying backup, snapshot, or replica. A scrub changes the records the application serves from that point forward. It offers no physical-erasure guarantee.

Log scrubbing is a different mechanism under a different contract (ADR-014, scrub paths inside the log sink) and is not governed by this rule.

The `privacy.anonymize_history` setting is persisted and exposed in the Settings view, but no engine or core code reads it: today it toggles nothing. Wiring it to the transform described above is outstanding work, and the setting must not be presented to users as protection it does not yet provide.
