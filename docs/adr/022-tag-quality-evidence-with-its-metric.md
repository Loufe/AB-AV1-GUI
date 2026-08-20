---
status: accepted
date: 2026-08-20
---

# Tag quality evidence with its metric

## Context and problem statement

Every durable quality value is VMAF-typed: `VmafTarget` and `VmafScore` thread the analysis map key, verdicts, `StatFact`, the statistics payload, and import. The pinned ab-av1 interface already exposes a `min_xpsnr` argument the adapter sets to none, and telemetry classifies XPSNR sample work, so a second metric sits one flag away. Upstream keeps receiving requests for more metrics (alexheretic/ab-av1#167, alexheretic/ab-av1#205, alexheretic/ab-av1#331, alexheretic/ab-av1#343). The journal is append-only and permanent, so widening bare VMAF types after the first release is a schema migration; before it, the same change is a refactor plus fixture regeneration.

## Decision drivers

* A measurement under one metric must never answer a request under another; the units are not comparable
* The journal outlives type changes, and the zero-backwards-compatibility window closes at the first release
* No second metric has a consumer today, and speculative features are prohibited
* A search runs under exactly one quality flag in the pinned interface

## Considered options

* Tag every durable target and score with a metric enum, VMAF the sole initial variant
* Keep bare VMAF types and migrate when a second metric is wanted
* Model each measurement as a map of scores per metric

## Decision outcome

Chosen option: **tag every durable target and score with a metric enum, VMAF the sole initial variant**, because it buys schema room at refactor cost today instead of migration cost later while building nothing speculative.

Mechanics, fixed by this record:

* The metric joins the analysis identity alongside decode mode (ADR-007), so an analysis recorded under one metric is never returned for a request under another.
* Verdict-carried targets and scores are self-describing; no consumer infers the metric from context.
* Statistics aggregate per metric and never blend scores across metrics.
* Imported V2 evidence is tagged VMAF, because V2 recorded nothing else.
* Each measurement carries one tagged score, matching the one-flag interface; a dual-metric future adds tagged scores rather than reshaping them.
* No second metric variant, code path, or UI exists until a consumer asks; the tag is schema insurance only.

### Consequences

* Good: Adding a metric later is an additive enum change, not a journal migration
* Good: Cross-metric blending is unrepresentable in aggregates and reuse
* Bad: Every durable quality type, both golden fixture sets, and the generated bindings change now

## More information

Shipped position and blast radius: `docs/design/history-consumer-matrix.md`. Analysis identity precedent: ADR-007. Journal permanence: ADR-004, ADR-009.
