---
status: accepted
date: 2026-07-20
---

# Adopt Versioned History Schema v2 and Drop Alias Records

## Context and Problem Statement

`conversion_history.json` is a bare JSON array with no version field; format changes are detected by key sniffing. `duplicate_of` alias records are full copies of a source record's verdict with no back-reference integrity: re-analyzing or deleting a source leaves stale or dangling aliases (issue #22). The legacy `conversion_time_sec` field survives only through fallbacks, and mapped-drive vs UNC spellings of the same path hash to different keys, creating duplicate records. The V3 rewrite (issue #33 §10) adopts this file one time into its Rust journal and needs a single well-defined input format.

## Decision Drivers

* The Rust importer (#39) must read exactly one declared format, not sniff keys
* Alias copies are unsound (no cascade on source delete/re-analyze) — and an audit of the real history file found **zero** alias records, so the machinery has no data to serve
* Path-spelling duplicates (`B:\x.mp4` vs `\\NAS\share\x.mp4`) split one file's facts across two records
* Zero-backwards-compatibility policy: user data may be migrated once by script; code keeps no compatibility layers

## Considered Options

* **Alias records as read-time references** — keep `duplicate_of` but store only the pointer
* **Drop alias records entirely; resolve duplicates at read time** — no persisted duplicate state
* **Auto-migrate inside the app's load path** vs **standalone one-time migration script**

## Decision Outcome

Chosen: **versioned container, no alias records, standalone migration script**, because aliases are unused in real data and read-time resolution makes staleness structurally impossible.

Schema v2:

1. **Container**: `{"schema_version": 2, "records": [...]}`. The loader accepts only `schema_version == 2` and raises with migration instructions on the legacy array form. No key sniffing.
2. **Alias machinery dropped**: the `duplicate_of` field, `create_alias_record`, and both alias-persisting call sites are deleted. Duplicate detection (ADR-001's metadata cascade) remains, evaluated at read/decision time: the Analysis display resolves a decided verdict from another path when rendering, and the worker's short-circuit skips a decided duplicate before processing. Nothing about a duplicate is ever persisted; every record is canonical.
3. **`conversion_time_sec` folded into `encoding_time_sec`** during migration (only where `encoding_time_sec` is absent), then the field and its fallbacks (`total_time_sec`, `get_analysis_level` best-crf sniffing) are deleted. Migration also normalizes pre-`ANALYZED`-era records (status `scanned` with Layer-2 results) to status `analyzed`.
4. **Drive-letter paths re-keyed under UNC**: `normalize_path()` resolves mapped network drives to their UNC form (local mapping table only — `WNetGetConnectionW`, no network I/O) before hashing, and the migration re-keys records with a stored `original_path` under the new hash, rewriting `original_path` to the UNC spelling. Records whose new keys collide (the same file recorded under both spellings) are merged: highest status wins, ties broken by `last_updated`, earliest `first_seen` preserved. Anonymized records (no `original_path`) cannot be re-keyed and keep their stored hash.

Migration runs as `python tools/migrate_history_v2.py` (idempotent, atomic write, keeps a `.v1.bak` backup); the app raises at load on an unmigrated file, matching the earlier `migrate_audio_streams` precedent. It must run on the machine holding the drive mappings.

### Consequences

* Good: stale/dangling aliases become unrepresentable; every record is canonical
* Good: the Rust importer reads one declared format; parked legacy records key on the exact hasher defined here
* Good: one file reached via mapped drive and UNC is one record
* Bad: a duplicate path is no longer filtered at enqueue time (no alias to find); the worker's short-circuit skips it during the run instead
* Bad: a CONVERT of an ANALYZED-elsewhere duplicate re-runs the CRF search (~1 min) instead of reusing the alias's copied Layer-2 data — accepted since duplicates are absent from real data
* Bad: with duplicate paths now stored as canonical SCANNED records in the size index, ADR-001's step-3 uniqueness fallback (renamed copies, no filename match) can no longer fire once both copies are scanned; steps 1–2 (filename match) are unaffected

## More Information

Amends ADR-001 (detection cascade unchanged; alias persistence removed). Decision recorded in issue #33 §10 as the Python-side prerequisite for the V3 history adoption (#39, #22). The path hasher frozen here — absolute-normalized path, mapped drives resolved to UNC, lowercased on Windows, backslashes to slashes, BLAKE2b (16-byte digest) truncated to 16 hex chars — is what the V3 migration module reimplements to match parked records.
