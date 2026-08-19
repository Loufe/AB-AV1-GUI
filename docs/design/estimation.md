# Estimation

Status: working design note; recorded verdicts are authoritative and survive, everything else is open

## Purpose and boundary

Estimation predicts work that has not happened: how long a job takes and how much smaller its output is. History records what happened and what each record claims; estimation is a model over that record. History's contract is truthfulness and provenance. Estimation's contract is calibration and honest uncertainty.

This document covers the model, its evidence seam, how uncertainty is expressed, and how an estimator is judged. Which observations exist, which collectors run, how records are stored, and how views render are all outside it.

## Evidence labels

Claims carry a label. Unlabeled prose is framing, not evidence.

- **read from source**: established by reading first-party or pinned code.
- **measured**: produced by a recorded measurement run.
- **derived**: computed from measured values, with the derivation stated.
- **carried reasoning**: an argument that has not been tested.
- **unverified**: carried from earlier investigation and not reproduced.

## Recorded verdicts

Labels are stable and cited bare.

**E1, no fabricated estimate.** An estimate is emitted only when evidence supports it. Where it does not, the value is absent, and absence is a display state rather than a failure. V2's terminal fallback to a hardcoded reduction constant is prohibited and must not reappear in any form, including as a seeded default, a placeholder, or a clamped floor. The boundary of this verdict is open; see sufficiency below.

**E2, point uncertainty.** An estimate reaches the user as a point value through the three-state presentation ramp in `docs/design/ui-verdicts.md`. No interval is shown; the ramp's states, not a number, carry confidence.

**E3, estimator family.** The estimator family is the kernel-weighted quantile successor recorded under Shipped position, built first rather than after a shipped median ladder. The ladder survives only as the evaluation comparator, and adoption still passes the evaluation contract below.

**E4, imported evidence.** Observations imported from the V2 history are admissible as a cold-start prior in a toolchain-unversioned quality class, down-weighted until native evidence dominates. They never gain native standing, because the producing toolchain, effective preset, and decode mode cannot be established after the fact.

## The seam with History

Estimation reads History as evidence and never writes to it. Three parts of that seam are unsettled.

**Read shape.** `EstimationModel::from_state` walks every content record and every conversion run on each estimation round (read from source). That is affordable against an in-memory journal but unproven against an indexed store holding a large history. The alternative is estimation describing a cohort and the store answering a query, which makes estimator cohorts part of the storage workload. The choice constrains storage engine selection and therefore precedes it.

**Admissibility authority.** `EstimationModel::from_state` reaches past `StatFact` into `state.conversion_runs` to harvest analyze rates from analyzed-only runs (read from source). That is the estimator judging which History records are eligible. The clean rule is that History decides what an observation is and what it claims, and estimation decides which observations are useful for a given prediction.

**The prediction and measurement pair.** `docs/HISTORY.md` requires that a prediction is never overwritten or reconciled when its outcome arrives. That recording rule belongs to History. The backtest consuming those pairs belongs here, and it is the substrate every evaluation claim below depends on.

## Consumers

Read from source, across `main` and the current tree.

| Estimate | Stage | Decision it supports | V2 | V3 today |
| --- | --- | --- | --- | --- |
| Per-file savings | after basic scan | convert this file | peer mean, then global mean, then constant | absent |
| Folder savings rollup | after basic scan | convert this folder | aggregate of the above | absent |
| Per-file time | after basic scan | scheduling | P50 with a P25 to P75 range | implemented, unwired |
| Per-file time | after analyze | scheduling | ab-av1 prediction | implemented, unwired |
| Queue total time | queued | when work finishes | present | absent |
| Predicted output size | after analyze | expectation setting | ab-av1 prediction | recorded; surfaced only after the fact |
| Live ETA | during a run | when work finishes | progress velocity | shipped |

The live ETA runs a different mechanism on different evidence yet answers the same user question as the pre-run estimate. Whether the two may disagree, and by how much, is unsettled.

ab-av1's three predictions cross the IPC boundary and are consumed unevenly (read from source). `predicted_percent_basis_points` reaches the user in exactly one place: the explanation attached to a not-worthwhile outcome, which reports what the highest-saving attempt measured. That is retrospective justification of a decision already taken, not a forward-looking estimate. `predicted_size` and `predicted_duration_ms` reach no consumer at all. The most trustworthy estimate available, the encoder's own measurement of the file in front of the user, is therefore the one least surfaced.

## Shipped position

Read from source.

- `EstimationModel`, `TimeEstimate`, `EstimateBasis`, and `HistoricalTier` are re-exported from `crfty-core` and consumed by no caller. No History-backed estimate reaches a user.
- The only forward-looking value a user sees is the active job's ETA from `crfty-engine/src/rate.rs`: a sliding-window progress velocity that stays absent through a warm-up period rather than reporting early and wrong. E1 is already the shipped behaviour there.
- No size estimator exists in any crate.
- The Analysis view's empty state tells the user that estimates appear after a basic scan. Nothing delivers them.
- The historical ladder is `(codec, resolution bucket)`, then codec, then a global pool, answering from the first tier holding three rate samples, as the median of phase time over video duration, graded `Precise` or `Estimated`.

V2 baseline, read from source on `main`.

- Time: the same rate quantity, grouped `(codec, resolution bucket)`, reported as P50 with a P25 to P75 range, over four fallback tiers with sample thresholds of ten and five, graded high, medium, low, or none. The grade reached the user as a prefix on the rendered value: no prefix for high, one tilde for medium, two tildes for low, and the absent-value placeholder when the grade was none. Uncertainty was therefore presentation, never a number, and the P25 to P75 range it was computed from was never shown.
- Size: the mean reduction percent of peers matched on codec and width, then the mean over all converted records, then a hardcoded constant. Applied to file size less an audio size derived from audio bitrate and duration.

A successor estimator was recorded when the projections were designed, and this document is its only surviving record. It is a kernel-weighted quantile estimator that weights samples by codec and resolution similarity instead of partitioning them into hard buckets. Backtesting predicted durations against actual durations is required before it replaces the ladder. Its claimed advantage is that a dissimilar sample approaches zero weight rather than gaining authority at a sample-count cliff, which is the ladder's structural defect (carried reasoning; never implemented or measured). The V2 quartile math and its sample threshold were deliberately not ported because reproducing them would have frozen accidental behaviour as specification.

## Stage leakage

A fact may train or adjust a prediction only if the same fact is available at the prediction stage in production. Terminal counters may filter, weight, or explain historical samples without becoming features of a same-run basic-scan estimate. Analysis results may improve convert estimates only once analysis has completed. This rule is what keeps an evaluation honest: violating it yields a model that scores well and cannot run.

## Evaluation

An estimator is admissible when it is measured against the one it would replace, not when it is argued for. That requires an error and bias measure, a calibration measure for whatever uncertainty it reports, a held-out split grouping by source work so segments of one title cannot land on both sides, and a threshold fixed before results are seen. Retained prediction and measurement pairs are the substrate. Collection overhead is measured separately from predictive value, because a collector that improves nothing still costs its stage.

## Open decisions

**Sufficiency.** What makes evidence admissible under E1: sample count, dispersion, relevance to the file being estimated, or a combination. The current global tier answers any file from any three conversions; that is measured on this machine and describes nothing about the file in front of it. The answer determines whether the tier survives.

**Uncertainty.** Point, interval, or absent. The recorded presentation ramp in `docs/design/ui-verdicts.md` has three states and no interval concept, so choosing an interval changes it.

**Promise level.** Whether accuracy is promised per file, per folder aggregate, or only as a bounded range. Measured evidence in `docs/design/history-content-evidence-research.md` records three files agreeing on every predictor either estimator uses whose outcomes span a factor of seven in predicted output size. That is the headroom a per-file promise must cover.

**Size estimation.** Whether a pre-analysis size estimate can exist at all under E1, what its basis is (video stream or whole file), and whether V2's audio-copy correction was intentional semantics worth keeping.

**Read shape and admissibility authority.** As above.

**Live ETA consistency.** Whether the pre-run estimate and the in-run ETA are required to agree, and what happens when they do not.

**Cold start.** What a new installation shows before any local evidence exists, given that E1 forbids inventing a starting value.

**Estimator family.** Keep the median ladder, restore a spread over it, or pursue the kernel-weighted quantile successor.

## Distillation

Settled rules move to `docs/ESTIMATION.md`, decisions become ADRs, and the verdict register survives until its content is recorded elsewhere.
