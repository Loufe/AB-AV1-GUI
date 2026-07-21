---
status: superseded by 015
date: 2026-07-20
---

# Derive History and Statistics as Pure Projections

## Context and Problem Statement

V3 already journals every raw fact — file records with verdicts, conversion
runs with phase spans, the output-transaction ledger — but had no History
view, no Statistics aggregates, and no conversion-time estimates (#40). The
Python V2 app derived these ad hoc from its history file, with semantics that
were partly deliberate and partly accidents of loose `None` handling. V3
needs one tested definition of these read models, a way to publish them that
respects the single ordered stream (ADR-006: commands return acks only), and
a seam so parked legacy records (#39) feed the same numbers later without
rework.

## Decision Drivers

* One oracle per derivation — no second implementation that can drift
  silently
* Determinism: no clock, timezone, filesystem, or cache inside crfty-core
* The stream stays the only event path; commands stay ack-only (ADR-006)
* Aggregates must never enter the journal — they are derivable, and stored
  copies rot (ADR-004)
* Legacy adoption (#39) must feed statistics and estimation without identity
  adoption or new plumbing
* Python parity where V2 semantics were intentional; documented divergence
  where they were bugs
* No new dependencies

## Considered Options

* Pure projection functions in crfty-core; Statistics answered as a
  request-driven ephemeral delta; History rows mirrored in TypeScript against
  exported golden fixtures
* Materialize aggregates in `DurableState` and update them in the fold
* Return the Statistics payload directly from the command (bypassing the
  stream)
* Compute everything in the frontend only, with no Rust definition
* Ship History rows over the wire alongside the snapshot

## Decision Outcome

Chosen option: **pure projections with a request-driven Statistics
ephemeral**, because it keeps derived data out of the journal, keeps the
stream as the one ordered event path, and puts the single tested definition
where determinism is enforced.

All logic is context-free functions in `crfty-core/src/projection.rs` and
`estimation.rs`. `collect_stat_facts` flattens each content with a standing
verdict into a `StatFact` — sizes joined in live-evidence →
settled-transaction → metadata order, phase times, codec, resolution, VMAF,
CRF, completion date. Statistics, History, and estimation consume facts and
state; nothing is cached or stored. `StatFact` is the #39 seam: parked legacy
records map into the same struct when they land, which is how their history
reaches Statistics and estimation without identity adoption.

Statistics crosses IPC vendor-style: `RequestStatistics { utc_offset_minutes }`
is validated and acknowledged by the reducer, which computes the payload
synchronously (pure and sub-millisecond at this data size — no worker) and
emits `EphemeralDelta::Statistics`. It is never journaled and never replayed
on subscribe; the UI re-requests when its inputs change. The caller-supplied
UTC offset keeps local-day bucketing deterministic and the core clock-free.

History rows do not cross IPC: the snapshot already ships full
`DurableState`, so `history_rows` is the Rust oracle and the frontend derives
rows with a TypeScript mirror, proven equal by golden fixtures exported from
the Rust side (`export_projection_fixtures` → `projection-fixtures.json`),
the same contract as the fold mirror. A verdict wins row status; without one,
the latest failed or stopped run reports with its reason; analyses alone
report as Analyzed; scanned-only content gets no row.

Estimation keeps V2's shape with a typed basis: a caller-selected fresh
analysis prediction (Convert only; freshness is policy's judgment, not
estimation's) is exact; otherwise a historical ladder — (codec, resolution
bucket) → codec → global, first group with ≥5 rate samples, exclusive
(type-6) quartiles of phase-time/duration rates scaled by the video's
duration; ≥10 samples in the top tier upgrades confidence. Analyze estimates
learn from every analyzing span (converted, not-worthwhile, and
analyzed-only runs); convert estimates from encoding spans.

### Deliberate divergences from V2 (Python)

* Savings totals and the cumulative chart share one rule: a fact contributes
  only when both input and output sizes are known. V2 was asymmetric, so its
  chart and its total disagreed.
* Negative savings are represented: totals and the cumulative series can dip;
  the histogram carries a separate `grew_count` instead of clamping growth
  into the 0–10% bin.
* Dates come from run completion (`finished_at`), not first-seen scan time,
  which V2 wrongly used for chart attribution.
* Facts dedupe by content key with the latest verdict winning — a converted
  copy no longer counts twice.
* Codec grouping uses the typed `VideoCodec` enum, not raw ffprobe strings.
* Remuxed outcomes are counted and summed separately, never blended into
  conversion savings/VMAF/CRF aggregates.
* Failed and stopped runs appear as History rows with their failure facts;
  V2 could not represent failures at all.
* Analyze-rate samples come from all analyzing spans, including the CRF
  search inside converted runs, not only analyze-operation records.

Parity quirks carried over intact: exclusive quantile math (pinned fixture
`[1,2,3,4,5] → 1.5/3.0/4.5`), histogram bin ownership by floor with ≥100%
clamped into the last bin, total time as analyze + encode phase spans, and
throughput as input GiB per total hour.

### Consequences

* Good: One tested definition per derivation; the TS mirrors cannot drift
  unnoticed past the CI freshness gates
* Good: Nothing derived is persisted — schema changes to aggregates are free
  under the zero-backcompat policy
* Good: #39 lands by mapping parked records into `StatFact`, touching no
  wire or reducer code
* Bad: Statistics recomputes a full scan per request — acceptable now,
  and the request-driven shape leaves room for caching behind the same
  command if profiling ever demands it
* Bad: The History mirror is duplicated logic; fixtures prove agreement on
  covered scenarios, not all reachable states
* Bad: `StatisticsPayload` carries floats, so the ephemeral delta family
  loses `Eq` and compares via `PartialEq` only

## More Information

Post-V3 note: the percentile ladder discards cross-group information (a
hard sample-count cliff at each tier). A kernel-weighted quantile estimator —
weighting samples by similarity across codec/resolution instead of hard
buckets — is the recorded successor, to be validated by backtesting predicted
vs. actual durations over accumulated history before replacing the ladder.

See issue #40 (requirements), #39 (parked records), #33 §14 (mirror-and-
fixtures contract), ADR-002, ADR-004, ADR-006.
