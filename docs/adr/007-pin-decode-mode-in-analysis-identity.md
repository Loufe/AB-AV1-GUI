---
status: accepted
date: 2026-07-20
---

# Pin the Actual Decode Mode in the Analysis Identity

## Context and Problem Statement

A CRF search measures VMAF against decoded frames. Hardware decoders (Cuvid,
QSV) and software decoding can produce different decoded frames for the same
bitstream, so their measurements are not interchangeable. `AnalysisProfile` is
the exact-match cache key for durable analyses (`FileRecord.analyses`), and the
question is whether the decode mode belongs inside that identity — and if so,
at what granularity — given that decode availability is resolved per machine
and per file, and that the hardware→software retry ladder makes the mode a run
can *actually* use diverge from the mode its spec requested.

## Decision Drivers

* A reused analysis must describe measurements the current execution would
  reproduce
* Decode resolution is machine- and file-dependent; specs must stay honest
  about what was requested versus what ran
* The retry ladder records results under a profile the spec did not request
* Issue #33 §7 already settled the direction in prose; the rewrite needs it
  pinned as a record

## Considered Options

* Keep the actual `DecodeMode` (decoder-granular) in `AnalysisProfile`
* Key by a coarse hardware/software bit only
* Exclude decode mode from the identity and treat measurements as universal

## Decision Outcome

Chosen option: **keep the actual `DecodeMode` in `AnalysisProfile`**, because
it is the only option under which a cache hit is a claim the current execution
can reproduce.

Mechanics, fixed by this record:

* `AnalysisProfile.decode_mode` carries the mode the search actually ran with.
  `select_analysis` is an exact-profile lookup, so an analysis recorded under
  `Hardware(H264Cuvid)` is not returned for a software execution — and, being
  decoder-granular, not for a `Hardware(H264Qsv)` execution either; those
  re-search. This is the accepted cost of honesty: switching GPUs re-analyzes.
* Requested-versus-actual provenance needs no extra type: the request lives in
  `ExecutionSettings.decode_preference`, the search's actual mode in
  `spec.execution.profile.decode_mode` (and, post-ladder, in the recorded
  result's profile), and encode divergence in the terminal evidence's
  `encode_decode` field.
* The hardware→software retry ladder records its fallback result under the
  software-decode variant of the prepared profile. The durable gate for
  `AnalysisRecorded` — `permitted_profiles(&ExecutionSettings)` — accepts
  exactly the prepared profile plus that variant, at both live apply and
  journal replay. The `JobSpec` is never rewritten.

### Consequences

* Good: A cache hit always describes measurements the execution reproduces
* Good: The ladder's divergent results stay durable without lying about mode
* Bad: Changing decoder hardware (Cuvid ↔ QSV) invalidates cached analyses
  and re-searches, even when scores would likely match
