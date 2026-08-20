# History consumer matrix

Status: working design note; the proposals below are recorded as decided in `docs/PLAN.md` (the metric tag as ADR-022), and parity rows await distillation into `docs/HISTORY.md`

## Purpose and boundary

This note records the V2-to-V3 consumer matrix for History and Statistics: each user-visible result, the evidence it requires, and the stage where that evidence becomes available. It restates the decided consumer floor, records the field dispositions that fall out, and derives the enriched import scope as their delta.

It selects no storage engine, designs no estimator (`docs/design/estimation.md`), sets no budgets (`docs/design/history-collection-budgets.md`), and decides no presentation (D11 in `docs/design/ui-verdicts.md`). V2 facts are read from source on `main`; V3 facts are read from source on the current tree.

## Evidence labels

- **decided**: recorded in `docs/PLAN.md`, or a product decision restated here as its durable home.
- **read from source**: established by reading first-party code.
- **derived**: computed from cited facts, with the derivation stated.

## The decided consumer floor

Every V2 statistic survives as a V3 consumer, with richer evidence where collection allows (decided). The floor also names required evidence beyond survival: per-run CRF-search time, distinct first-seen and last-updated timestamps, the not-worthwhile reason, preset, bitrate, and per-stream audio evidence including bitrate (decided; the audio requirement realizes verdict E5).

Three floor items already stand (read from source): search time persists as the analyzing span in each run's `phase_spans`, preset is part of the `AnalysisProfile` keying every recorded analysis, and the not-worthwhile reason survives as the typed verdict below. The remaining items are the gaps in their own section.

## Availability stages

Evidence enters durable state at four fold events, and each matrix row names the earliest stage its evidence exists (read from source).

| Stage | Fold event | V2 equivalent |
| --- | --- | --- |
| Scan | `MediaObserved` writes `VideoMeta` onto the content record | SCANNED |
| Search | `AnalysisRecorded` files an `AnalysisResult` under its profile and target | ANALYZED |
| Terminal | `ItemFinished` writes the verdict, run outcome, and phase spans | CONVERTED, NOT_WORTHWHILE |
| Import | `ParkedAdopted` attaches an imported summary | n/a |

Scanned-only content and skipped runs get no History entry, and queue eligibility and analysis reuse never read History (`docs/HISTORY.md`).

## Statistics parity

Read from source: every V2 Statistics aggregate has a wired V3 home in `StatisticsPayload`, computed in `crfty-core/src/projection.rs` and rendered by the Statistics view.

| V2 result (`src/gui/tabs/statistics_tab.py`) | Required evidence | Stage | V3 home |
| --- | --- | --- | --- |
| Total files converted | converted verdicts | Terminal | `converted_files` |
| Average, min, max VMAF | verdict-carried score | Terminal | `vmaf` spread |
| Average, min, max CRF | verdict-carried CRF | Terminal | `crf` spread |
| Average, min, max size reduction | joined input and output sizes | Terminal | `reduction_percent` spread |
| Size reduction histogram | joined sizes | Terminal | `reduction_bins` |
| Total space saved | joined sizes | Terminal | `total_saved_bytes` |
| Throughput in gigabytes per hour | input bytes, analyzing and encoding spans | Terminal | `gigabytes_per_hour` |
| Source codec distribution | observed codec | Scan + Terminal | `codecs` |
| Cumulative space saved by day | joined sizes, terminal timestamp | Terminal | `cumulative_savings` |
| History range | terminal timestamps | Terminal | `first_epoch_day`, `last_epoch_day` |

V3 exceeds the floor with aggregates V2 never had: remux savings, grew count, the not-worthwhile count, and terminal run totals including stopped, skipped, and failed (read from source).

Two semantics differ deliberately. V2 keyed its date axis on `first_seen`, which its own worker overwrote on every terminal write, so the values coincided with conversion dates by accident (read from source). V3 keys on the terminal timestamp, keeping the intentional semantic. A converted fact missing either size still counts as converted while contributing nothing to savings, because absence and zero are different claims (`docs/HISTORY.md`).

## History browsing parity

Read from source: the V3 history view covers every V2 History tab column and adds container, encode time, and file actions.

| V2 column (`src/gui/tabs/history_tab.py`) | Required evidence | Stage | V3 home |
| --- | --- | --- | --- |
| Name | path binding, run input, or import path | Scan | row key joins |
| Date | terminal timestamp | Terminal | `happened_at` |
| Status | verdict, run outcome, or import status | Terminal | `status` |
| Resolution, Codec, Duration, Audio | observed metadata | Scan | `VideoMeta` fields |
| Bitrate | size, duration, per-stream audio evidence | Scan | derived in views |
| Input, Output, Reduction | joined sizes | Terminal | size fields |
| VMAF, CRF | verdict-carried values | Terminal | `vmaf`, `crf` |

V2's status filtering survives as a consumer, and failed and stopped rows sit behind default-off filters, excluded from Statistics aggregates (decided). Scrub reporting needs only record counts and imposes no field (read from source).

Bitrate is the one V2 column with no V3 field, and it needs none: `VideoMeta` records that bitrate derives in views (read from source). With per-stream audio bitrates present, the video-stream rate is the remainder of container bytes over duration, which is also the E5 size basis (derived).

## Rows retained for estimation

Estimation consumers and prediction goals are owned by `docs/design/estimation.md`; this matrix records only the evidence rows History retains for them.

| Consumer | Required evidence | Stage |
| --- | --- | --- |
| Time cohorts | phase spans split by operation; observed codec, dimensions, duration | Terminal, Scan |
| E5 size basis | settled sizes; per-stream audio bitrate and duration | Terminal, Scan |
| Evaluation backtest | retained prediction and measurement pairs, never reconciled | Search, Terminal |
| E4 cold-start prior | adopted imported summaries, tagged VMAF, in the toolchain-unversioned class | Import |

## Surfaces that are not History consumers

The V2 completion summary dialog reads a per-run in-memory session accumulator, never the store, so V3 owes History no reconstruction of it (read from source).

The V2 queue operation column and skip breakdown read current freshness-checked evidence in V3, never History rows (`docs/HISTORY.md`).

## Fields the durable model does not yet hold

**Per-stream audio bitrate (decided).** `AudioStreamMeta` holds codec and channels today (read from source). The floor and E5 require bitrate per stream. It arrives by extending the field list of the single ffprobe call Basic Scan already makes, adding no process and no file read, and stays absent where the container omits it. The scan's `-show_entries` allowlist excludes bitrate today, so the fetch list grows while the invocation count does not (read from source).

**First-observed and last-updated (decided).** No durable type carries either; `Verdict.decided_at` and run timestamps exist (read from source). The content record gains a first-observed stamp folded once at its first `MediaObserved` and never overwritten, and a last-updated stamp refreshed by any fold that changes the record. The stamps must live on the record: compaction folds the journal into one snapshot line, so replay-derived stamps do not survive it (read from source). V2's writers disagreed with each other here, so there is no intentional V2 semantic to preserve (read from source).

## Metric-tagged quality evidence

**Read from source.** `VmafTarget` and `VmafScore` are bare newtypes threading the analysis map key, verdicts, `StatFact`, the statistics payload, and import. The vendored interface exposes a `min_xpsnr` argument the adapter sets to none, telemetry classifies XPSNR sample work, and a search runs under exactly one quality flag. Upstream requests for further quality metrics recur (alexheretic/ab-av1#205, alexheretic/ab-av1#331, alexheretic/ab-av1#343).

**Decided (ADR-022).** A quality metric enum, VMAF its sole initial variant, tags every durable target and score. The metric joins the analysis identity, extending the ADR-007 precedent of pinning decode mode there, so a VMAF result never answers an XPSNR request. Verdict-carried values are self-describing, statistics spreads never blend metrics, and imported V2 evidence is tagged VMAF. No XPSNR code path is wired until a consumer asks; the tag is schema insurance. Before the first journaled release the change is a refactor plus fixture regeneration; after it, a journal migration.

## The not-worthwhile row

**Read from source.** V2 persisted a free-text `skip_reason`; the typed verdict carrying the requested target and fallback floor supersedes it. The user-facing explanation composes from the highest-saving attempt's measurement and reads the live run today.

**Decided.** The verdict embeds that highest-saving measurement, metric-tagged, so adopted imports and any future run pruning keep their explanation. The embedded value is a prediction and stays typed as one, per the prediction and measurement rule in `docs/HISTORY.md`.

## Field dispositions

A field is a free option when the stage's existing probe invocation can return it without added I/O, it is observed rather than inferred, and it is identity-free; such a field may ship with a speculative browsing or statistics consumer (decided). Inferred facts rot unread: V2 recorded `output_audio_codec` by inference from a dead setting, wrongly on most records, and no consumer existed to notice (read from source).

| V2 field | Disposition (decided) |
| --- | --- |
| `filename_hash` | superseded by the sampled content key (ADR-019) |
| `predicted_output_size` | superseded by `SearchMeasurement.predicted_size` |
| `vmaf_target_attempted`, `vmaf_target_used` | superseded by the recorded requested and successful targets |
| `skip_reason` | superseded by the typed not-worthwhile verdict above |
| `estimated_reduction_percent`, `estimated_from_similar` | not evidence; estimates are recomputed projections, never stored |
| `output_audio_codec` | observed, never inferred: the converted output's own observed metadata is the truth |
| audio `channels` | already collected |
| audio `sample_rate`, `language` | collected: free, observed, identity-safe; export coarsening decides what leaves the machine (B4) |
| audio `title` | rejected: free-text container metadata carries the work identity that scrub and pseudonymity close |

## Enriched import

Import v1 carries status, sizes, modification time, codec, dimensions, duration, encode time, CRF, VMAF, targets, and one decision timestamp (read from source). V2 records hold more than the importer keeps (read from source on `main`).

The enriched scope is the delta between the floor above and import v1 (derived, decided). It adds per-run CRF-search time, first-seen and last-updated, per-stream audio evidence including bitrate, and the recorded video bitrate, an observation the view derivation cannot reconstruct where audio evidence is missing. Every imported quality value is tagged VMAF. Preset is deliberately not carried: E4 records that the effective preset cannot be established after the fact, which is why imported evidence sits in a toolchain-unversioned class.

## Open decisions

- Which folds refresh the last-updated stamp.
- How statistics spreads render when a second metric first appears.

## Distillation

Parity rows recording settled behaviour move into `docs/HISTORY.md`, frozen proposals land in `docs/PLAN.md` with an ADR for the metric-tagged schema, and this note is deleted when the freeze lands.
