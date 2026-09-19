---
status: accepted
date: 2026-09-18
---

# Record one observation per terminal run

## Context and problem statement

History is the evidence Statistics and estimation are computed from, and the alpha requires durable observations for terminal work, retained prediction and measurement pairs, and sparse evidence for failed, stopped, and incomplete runs. The Python application kept one record per path and rewrote it on every terminal, so a failed retry, a source that changed between two conversions, and a prediction later contradicted by its encode all left no trace. V3 today keys durable facts by content and projects one History row per content, collapsing the runs behind it, while the estimator walks conversion runs directly and selects its own candidates. The question is what the unit of History is, who may change it, and who decides which units a consumer may use.

## Decision drivers

* A prediction and the measurement that tests it must both survive, even after a later run on the same file predicts again
* A retry, a changed source, and an interrupted run must keep their own outcome rather than being replaced by a later success
* Failed, stopped, and incomplete work must retain sparse evidence behind default-off filters
* Consumers must not each decide for themselves which records count; the seam between History and estimation must be one-directional
* The file-level view users know from the Python application must remain derivable

## Considered options

* One record per file, overwritten by the latest terminal, as the Python application did
* One immutable observation per terminal run, with the file's standing derived from its observations
* One immutable observation per run plus a separately stored per-file summary kept in step by the writer

## Decision outcome

Chosen option: **one immutable observation per terminal run**, because it is the only option under which prediction pairs, retries, changed sources, and interrupted work are representable without a second write path, and the per-file view costs a derivation rather than a stored duplicate.

Mechanics, fixed by this record:

* An observation describes one run that reached a terminal outcome: converted, remuxed, not worthwhile, analyzed, failed, stopped, or incomplete. Skipped work and reservation-only failures produce no observation; their reasons stay on the queue item.
* A native observation is identified by its run identifier, which is never reused. A translated observation is identified by its import origin and the origin's record key, never by a readable path, so pathless evidence remains identifiable and re-import stays idempotent.
* An observation names exactly one source by content key. Several observations may name the same content; the file's current standing is the latest decisive observation for that content and is derived on request, never stored or overwritten.
* Once recorded an observation does not change. A later run on the same file is a new observation. Privacy scrub removes readable paths from an observation and nothing else.
* Predictions and measurements are distinct typed facts on the observation; neither replaces or reconciles the other. Absent facts are absent, and an unknown decision time is represented as unknown rather than filled in.
* Three things are called attempts and are kept distinct. Quality-target fallback attempts belong to the analysis result inside one run. The hardware-to-software decode retry belongs to the run and is visible in its encode decode mode. A queue retry mints a new run and therefore a new observation, with lineage derived by content key and run order.
* History owns eligibility. Each consumer obtains facts through a typed accessor that yields a value only when the observation's outcome, source assessment, and provenance qualify for that purpose. Estimation owns weighting: how much an eligible observation counts for a given prediction, including by toolchain, environment, and throttling provenance. Neither reaches into the other's decision.
* Aggregates that count files dedupe by content key, since one file may honestly hold several converted observations after its source changes.

### Consequences

* Good: Prediction pairs, retries, changed sources, and interruptions are first-class evidence rather than casualties of an overwrite
* Good: The estimator stops selecting its own candidates, so eligibility is decided once and tested once
* Good: The per-file view is a pure derivation and cannot disagree with the observations beneath it
* Bad: Storage holds every terminal run, so browsing and aggregation are over observations rather than files and must be bounded by the storage selection
* Bad: Statistics that the Python application counted per file must dedupe explicitly, and the parity fixtures change to say so

## More information

The observation contract, the required and optional facts per outcome, and the eligibility rules are in `docs/HISTORY.md`. The seam this record fixes is described in `docs/design/estimation.md`. Source assessment per phase, which the eligibility accessors consume, is the [source-continuity contract](../design/source-continuity.md). Content identity is ADR-019 and metric tagging of quality facts is ADR-022. ADR-015's separate imported projection is replaced when translated observations share this model in durable storage; until then it describes the shipped parked path.
