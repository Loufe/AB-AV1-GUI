---
status: accepted
date: 2026-07-18
---

# Replace Alias Records with Path Normalization

## Context and Problem Statement

The original duplicate-detection decision (replaced by this record; see git history) handled "same physical file, different filesystem path" with a metadata cascade (size + duration + filename) that writes *alias records*: copies of the source's verdict re-keyed to the duplicate path, linked by `duplicate_of`. Auditing the production history (5,286 records) showed the mechanism has never fired — zero alias records exist — while nine duplicate groups matching the full cascade sit undetected, because detection only runs on cache miss and never revisits existing valid records. All nine trace to one cause: the library is recorded under two spellings of the same NAS (5,084 UNC-path records vs 176 mapped-drive `B:` records), which `normalize_path()` treats as distinct files.

The copy-based design also carries a latent staleness class: aliases hold copied verdicts with no back-reference, so re-analyzing, demoting, or deleting a source silently leaves stale copies behind (`history_index.py`, issue #22).

## Decision Drivers

* A verdict reached under one path spelling must hold under any other spelling of the same file — re-running a CRF search costs ~1 minute, a redundant encode costs hours
* `NOT_WORTHWHILE` is invisible in the file itself, so it can only be protected by history — losing it causes repeated wasted analysis
* No network I/O during identity decisions (the original decision's driver; offline NAS must not freeze scans)
* Invariants should be structural, not maintained by discipline — a copy that must be cascade-updated by every future writer is the opposite
* No survey precedent: Stash, Unmanic's metadata store, FileBot, and digiKam all key content identity to one canonical record referenced by many paths; Tdarr solves the multi-path case with configured path translation; none copy verdicts per path

## Considered Options

* **Option A: Keep copy-based aliases, add integrity cascades** — reverse index from source to aliases, rewrite copies on every source change/delete
* **Option B: Convert aliases to read-time references** — alias stores only identity + `duplicate_of`; status and results resolve from the source at lookup
* **Option C: Normalize path spellings at hash time and remove the alias mechanism entirely** — defer exact content identity to the partial-hash tier planned in issue #28

## Decision Outcome

Chosen option: **Option C**, because the audit shows the entire observed duplicate class is a path-spelling artifact, which normalization eliminates structurally — the two spellings hash to one record, so there is nothing to alias, cascade, or resolve. Options A and B both preserve machinery whose only remaining target (true content copies) is better served by the exact fingerprint tier in #28, and A additionally institutionalizes the invariant-by-discipline pattern this codebase is moving away from.

`normalize_path()` (`src/privacy.py`) additionally resolves mapped drive letters to their UNC targets before hashing, using `WNetGetConnectionW` — a local mapping-table lookup that works with the share offline, so the no-network-I/O driver is preserved. Results are cached per drive letter for the process lifetime.

Removed outright: `create_alias_record`, `find_better_duplicate`, the size index, the worker's duplicate short-circuit (`_find_duplicate_verdict`, `_persist_duplicate_alias`), the scanner's duplicate branch, the `duplicate_of` field, alias exclusions in the accessors, and the uncalled `HistoryIndex.delete()`.

The one-time history rewrite (ADR-002) re-keys drive-letter records to their UNC hash and merges duplicate groups, keeping the highest-status record per file.

### Consequences

* Good: the dominant duplicate class becomes unrepresentable instead of detected-and-compensated; the nine live duplicate groups merge into their decided records
* Good: the alias staleness bug class, the metadata-cascade false-positive risk accepted by ADR-001, and the detection coverage gap all disappear with the code that carried them
* Good: shrinks the eventual Rust port (issue #33) — no alias invariants to model or property-test
* Bad: until #28 lands, a *true content copy* (distinct file, same content) of a decided file is re-analyzed once (~1 minute); a redundant encode remains impossible because converted content is caught by the already-AV1 check
* Bad: anonymized records (26 of 5,286) store no path and cannot be re-keyed; they are dropped by the rewrite and self-heal on next scan

## More Information

Replaces the earlier metadata-only duplicate-detection ADR (deleted; see git history). Companion schema rewrite: [ADR-002](002-adopt-versioned-history-schema.md). Issue #28 tracks the partial-content-hash tier (OpenSubtitles-style size + head/tail checksum, the pattern proven by Stash/FileBot); under it, a moved or copied file re-attaches to its record by *re-keying or reference*, never by copying verdicts. Audit and ecosystem survey: issue #22 discussion, 2026-07-18.
