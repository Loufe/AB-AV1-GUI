---
status: accepted
date: 2026-08-16
---

# Key Durable Facts by Sampled Content Identity

## Context and Problem Statement

Analyses and verdicts are expensive to produce and they describe content, not locations, but the Python application keyed them by path hash: renaming or moving a file discarded its history, and two copies of the same video were searched twice. V3 needs one durable key for reusable facts, and choosing it means deciding what the key is derived from, how much of a multi-gigabyte file may be read to compute it, and what the resulting equality is allowed to authorize.

## Decision Drivers

* A move or a rename must not discard an analysis or a verdict
* Content already judged once must not be searched again, wherever it now lives
* Identity cost must be bounded and independent of file size
* Durable facts must survive reinstall and data-directory relocation
* No destructive action may rest on a probabilistic claim

## Considered Options

* Key durable facts by path, as the Python application did
* Key by a full-file hash, with collision confirmation
* Key by a sampled hash over a probe-derived header plus fixed regions of the file
* Key by a sampled hash salted per install

## Decision Outcome

Chosen option: **a sampled content hash**, because it makes a moved file the same file at bounded cost, and because the facts it keys are advisory rather than destructive, which is what makes probabilistic equality an honest fit rather than a compromise.

Mechanics, fixed by this record:

* `ContentKey` is BLAKE2b with a 16-byte output over the domain separator `ck1`, then the file length, the probe-reported duration in milliseconds, the length-prefixed codec name, the width and the height, then content bytes. Its text form is `ck1:` followed by the digest in hex.
* A file of 4 MiB or less is hashed whole. A larger file contributes its first 256 KiB, 64 KiB at each quarter with the offset rounded down to a 4 KiB boundary, and its last 256 KiB, so any large file is identified by about 704 KiB of reads.
* Sampling is guarded on both sides: filesystem identity is captured before the first read and again after the last, and a mismatch fails the operation instead of producing a key.
* Durable facts live in `records`, a map from `ContentKey` to `FileRecord` (media metadata, the analysis index, the standing verdict, imported provenance). Locations live in `paths`, a map from `PathHash` to a binding of full filesystem identity plus the `ContentKey` it resolved to.
* The two hash spaces are distinct newtypes over distinct domain separators, `ck1` for content and `ph2` for canonical paths, so a location key and a content key cannot be interchanged or compared.
* Content equality may reuse an analysis and may skip queued work with a visible typed reason (`SkipReason::ProbableDuplicate`). It authorizes nothing destructive: promotion, overwrite, and original retirement each revalidate exact filesystem identity (file id, size, modification time) immediately before acting, and any mismatch aborts the step.
* The digest carries no per-install or per-machine salt, so the same bytes yield the same key on every machine and after any reinstall.

### Consequences

* Good: A moved, renamed, or copied file keeps its analyses and its verdict, and a duplicate costs a skip rather than a search
* Good: Identity cost is bounded, so a very large file is keyed as cheaply as a small one
* Good: A restored or migrated data directory still matches the media it describes
* Bad: Equality is probabilistic, because files differing only outside the sampled regions collide, so every destructive path must carry its own exact-identity check
* Bad: The header includes probe-reported duration, codec, and dimensions, so a probe reporting different values for unchanged bytes yields a different key and a re-analysis
* Bad: The key is deterministic and unsalted, so anyone holding a history file plus candidate media can test which media it describes; it identifies content rather than location, and it crosses IPC as the record map key, so it is not treated as a secret

## More Information

Implementation: the digest and its stability guards are in `crates/crfty-engine/src/media.rs`; the key and identity types are in `crates/crfty-core/src/output.rs` and `crates/crfty-core/src/media.rs`; the state shape is `DurableState` in `crates/crfty-core/src/state.rs`; the duplicate rule is in `crates/crfty-core/src/policy.rs`; destructive revalidation is in `crates/crfty-engine/src/output.rs`.

The construction is pinned by golden fixtures (`ck1_matches_independent_golden_fixtures`), so changing the digest is a deliberate act with a visible diff rather than an accident.

Related: ADR-004 (the journal these records persist to), ADR-007 (what a reusable analysis must match beyond content), ADR-015 (the projections that read these records), and ADR-017 (row identity in Analysis, which restates the probabilistic limit for large files).
