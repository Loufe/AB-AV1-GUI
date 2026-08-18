# History

History is the durable record of what conversion work actually happened, and the evidence that Statistics and estimates are computed from.

This document states only the rules that are settled and verified against the shipped core today. It is not yet the full logical contract, and the storage model behind it is not decided here.

## History reports outcomes; it does not decide them

A History entry describes work that reached a terminal outcome: a conversion, a remux, a not-worthwhile judgment, a completed analysis, or a failed or stopped run. Content that was only scanned has nothing to report and gets no entry. A skipped run decided nothing and gets no entry either; its reason belongs to the queue item that was skipped.

Nothing is stored under the name History. History rows and Statistics are context-free projections of durable state, recomputed on request with no clock, no filesystem, and no cache, per ADR-015, project imported history separately.

Facts flow one way. The analysis level a file stands at, the analysis it may reuse, and its eligibility for the queue are all decided from current freshness-checked evidence about the file on disk, never from History:

- Analysis level is split at the type level. `assess_analysis_levels` returns `applicable`, meaning what the current file and execution settings can actually reuse, alongside `historical`, meaning the best tier known to have been reached. Historical achievement, including an imported Converted or Analyzed summary, raises only `historical`.
- A historical analysis is not a reusable analysis. Analysis identity is profile-exact per ADR-007, pin decode mode in analysis identity, so only a native search recorded under a permitted profile can be selected for reuse. An imported Analyzed fact is display-only and never enters `FileRecord.analyses`.
- Queue eligibility reads the path binding, the live destructive identity, timestamp reliability, and the standing verdict, per ADR-013, filter queue adds at enqueue. It consults neither History rows, nor Statistics, nor the parked import inbox.

Estimation is the one sanctioned consumer in the other direction: completed phase spans and settled sizes feed the cohorts behind size and time predictions. It reads History and never writes to it; its model, evidence seam, and open questions are in `docs/design/estimation.md`.

One seam does run from historical records into current state, and it is deliberately narrow. An imported record adopts onto a content record only when a fresh observation of the file at its recorded path confirms it, either by matching size and modification time or by the replace-mode case where the file now at that path is already the AV1 output. Adoption is the one-time route for V2 history and is governed by ADR-015; a record that no longer describes the file retires and decides nothing.

## Predictions and measurements are separate observations

A quality search produces predictions. `SearchMeasurement` carries `predicted_size`, `predicted_percent_basis_points`, and `predicted_duration_ms` next to the CRF and VMAF score it settled on.

A finished encode produces measurements. `CompletionEvidence::LiveEncode` carries input and output sizes verified against the settled output transaction.

Both persist, and neither replaces the other. A prediction is never overwritten, nulled out, or reconciled when its outcome arrives, because holding the pair is the only way to learn what the prediction was worth.

No consumer may present a prediction as a measurement. The distinction lives in the field names and the types rather than in a convention readers are asked to remember.

## Sizes are measured or absent

Byte sizes reach History from verified identities only, joined in a fixed order: live completion evidence, then the promoted artifact identity in the output transaction, then the summary an adopted import carried, and finally the record's own inspected size for the input while the output stays unknown.

Human-readable FFmpeg output is not a size source. The stream summaries FFmpeg prints when an encode ends are display text rounded to a unit FFmpeg chose, and the engine does not treat human-oriented process output as an application contract. Such a figure must never be stored as a size, and a value derived from one must never be presented as a measurement.

An unknown size is absent, not zero. Absence and zero are different claims, and a size that was never established must not be allowed to become the second one. Absence is carried through the projections intact: a converted fact whose sizes are not both known still counts as converted, and contributes nothing to savings totals, reduction bins, or the cumulative series.

## Pathless is pseudonymous, not anonymous

A History record stripped of readable paths remains linkable. The identities that survive are pseudonyms rather than erasures: a content key is a BLAKE2b digest over sampled file bytes plus the media header, and a path hash is a BLAKE2b digest over the canonicalized path. Anyone holding the same file recomputes its content key exactly, and anyone holding a list of candidate paths can recompute path hashes and match them.

Exports and disclosures must say so plainly. A bundle without readable paths is described as pseudonymous, never as anonymous and never as de-identified. Its technical facts, exact tool revisions and resolutions and durations and sizes and timings, are ordinary one at a time and can be distinctive in combination, so the linkability a bundle admits belongs in its own disclosure rather than in a footnote elsewhere.

## Scrub is a logical transform, not erasure

Scrubbing removes readable path data from the current records and leaves everything else standing. Pseudonymous identity and every technical and statistical fact survive it, because the goal is to drop the readable path without weakening the evidence. ADR-015 enumerates the path-bearing durable surfaces a scrub has to cover.

Scrubbing is idempotent. Scrubbing an already-scrubbed store changes nothing, and a scrubbed store is a valid store rather than a degraded one.

Scrubbing is not erasure, and must never be described to users as if it were. State persists through an append-only journal (ADR-004, persist state in an append-only journal) compacted by atomic replacement with a snapshot head line (ADR-009, compact the journal into a snapshot head line), and nothing below the application, not the filesystem and not any backup, snapshot, or replica underneath it, is asked to forget bytes it already wrote. A scrub changes the records the application serves from that point forward. It offers no physical-erasure guarantee.

Log scrubbing is a different mechanism under a different contract (ADR-014, scrub paths inside the log sink) and is not governed by this rule.

The `privacy.anonymize_history` setting is persisted and exposed in the Settings view, but no engine or core code reads it: today it toggles nothing. Wiring it to the transform described above is outstanding work, and the setting must not be presented to users as protection it does not yet provide.
