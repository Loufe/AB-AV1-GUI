---
status: accepted
date: 2026-08-20
---

# Name the quality metric once per record

## Context and problem statement

Quality targets and scores thread the analysis map key, verdicts, `StatFact`, observations, the statistics payload, and import. Their units must remain explicit in durable evidence. The pinned ab-av1 interface exposes a `min_xpsnr` argument the adapter sets to none, and telemetry classifies XPSNR sample work, so a second metric sits one flag away. The journal outlives the code that records it; a metric named in durable evidence lets a later metric variant describe its units without replacing the durable quality types.

## Decision drivers

* A measurement under one metric must never answer a request under another; the units are not comparable
* A record mixing metrics must be unrepresentable, not rejected by validation
* The journal outlives type changes, and the zero-backwards-compatibility window closes at the first release
* No second metric has a consumer today, and speculative features are prohibited
* A search runs under exactly one quality flag in the pinned interface

## Considered options

* Name the metric once per record, with bare values beneath it, VMAF the sole initial variant
* Tag every target and score with its own metric
* Keep bare VMAF types and migrate when a second metric is wanted
* Model each measurement as a map of scores per metric

## Decision outcome

Chosen option: **name the metric once per record**, because it buys schema room at refactor cost today, and a record cannot hold values in two metrics. Tagging every value made mixed-metric records representable and needed a consistency check at each entry point.

Mechanics, fixed by this record:

* `QualityTarget` and `QualityScore` are bare values whose units come from the enclosing record's `QualityMetric`, which owns their range checks and normalization.
* The metric is a field of `AnalysisProfile`, part of the analysis identity alongside decode mode (ADR-007). Every target and score in an analysis or execution is in the profile's metric, and reuse requires profile equality, so a result recorded under one metric is never returned for a request under another.
* Verdicts and native observations that carry quality values name their metric once. A value read outside its record travels as a `MetricScore`.
* V2-origin evidence (parked imports and translated observations) is VMAF by origin: `ImportOrigin::quality_metric` states it once, and readers tag those values with it.
* Statistics aggregate per metric and never blend scores across metrics.
* No second metric variant, code path, or UI exists until a consumer asks. Adapter boundaries match on the metric exhaustively, so adding a variant fails to compile at each place that must handle it.

### Consequences

* Good: Adding a metric later is an additive enum change, not a journal migration
* Good: Cross-metric blending is unrepresentable in a record, in reuse, and in aggregates
* Bad: A dual-metric measurement would reshape `SearchMeasurement`; no consumer needs one, and the one-flag interface cannot produce one
* Bad: Every durable quality type, the golden fixture sets, and the generated bindings change now

## More information

Shipped position and blast radius: `docs/design/history-consumer-matrix.md`. Analysis identity precedent: ADR-007. Journal permanence: ADR-004, ADR-009.
