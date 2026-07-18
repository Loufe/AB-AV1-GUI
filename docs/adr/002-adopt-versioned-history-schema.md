---
status: accepted
date: 2026-07-18
---

# Adopt Versioned History Schema (v2)

## Context and Problem Statement

`conversion_history.json` has no schema version: migrations are detected by key sniffing (`_load_from_disk` checks for an `audio_codec` key and directs users to a migration script that no longer exists). The record schema carries debt that every consumer — and the planned Rust port's DTO layer (issue #33) — must compensate for: a legacy combined-time field, status/level double-encoding with fallback heuristics, and fields whose validity depends on `status` in undocumented ways. A 2026-07-18 audit of the production file (5,286 records, uniform keyset, no NaN/Infinity values) quantified the debt and confirmed a one-time rewrite is low-risk.

## Decision Drivers

* 484 of 656 CONVERTED records hold only the legacy `conversion_time_sec`; time estimation reads it via a fallback chain (`models.total_time_sec` → `estimation.py`), so the field cannot simply be deleted without degrading estimates for 74% of conversion history
* The `get_analysis_level()` heuristics ("old records before ANALYZED status existed", `models.py`) match zero production records — dead compensation code
* `NOT_WORTHWHILE` has no representable analysis level: a completed (failed) CRF search reports level SCANNED
* The Rust port needs a settled format for its day-one round-trip test; every heuristic kept in Python becomes a permanent legacy branch in the Rust DTO
* User data justifies a conversion step despite the zero-backwards-compatibility policy; code-level compatibility shims remain prohibited

## Considered Options

* **Option A: Keep the schema, document the quirks** — no migration, heuristics stay
* **Option B: Versioned container + cleaned records via a one-time rewrite** — `{"schema_version": 2, "records": [...]}`; legacy data folded forward; all compensation code deleted
* **Option C: Move to a new store (SQLite / JSONL)** — already rejected in issue #22 scoping; JSONL is the Rust port's decision (issue #33), not this one

## Decision Outcome

Chosen option: **Option B**. The v2 schema:

* **Container**: top-level object `{"schema_version": 2, "records": [...]}`. The loader accepts v2 only; a bare array or unknown version gets one clear error naming the rewrite tool. Key-sniffing migration detection is deleted.
* **Legacy time fold**: for records with `conversion_time_sec` set, the rewrite computes `encoding_time_sec = conversion_time_sec − (crf_search_time_sec or 0)` and drops the field; `total_time_sec`'s fallback and `estimation.py`'s fallback branch are deleted.
* **Analysis level derives purely from status**: the field-sniffing fallbacks in `get_analysis_level()` are deleted. `NOT_WORTHWHILE` maps to level 2 (ANALYZED) — the CRF search ran to completion; its result is the negative verdict (`vmaf_target_attempted`, `min_vmaf_attempted`, `skip_reason`).
* **Path re-keying and duplicate merge** per ADR-001: drive-letter records re-hash under their UNC spelling; duplicate groups merge, keeping the highest-status record (ties broken by `last_updated`); the `duplicate_of` field is dropped. Anonymized records that cannot be re-keyed are dropped and self-heal on next scan.
* **Non-finite floats**: dropped field-by-field with a logged warning at load (audit found zero occurrences; the policy exists so the Rust port's `serde_json`, which rejects `NaN`/`Infinity`, inherits clean data).
* **Rewrite tool**: `tools/migrate_history_v2.py`, run once; writes `conversion_history.json.bak` first, then replaces atomically.

### Consequences

* Good: every schema consumer, including the future Rust DTO, targets one self-describing format with no legacy branches
* Good: time estimation keeps its full 656-conversion signal through the fold instead of losing 484 records
* Good: format changes become an explicit version bump instead of key sniffing
* Bad: a one-time manual migration step (mitigated by the `.bak` and a clear startup error)
* Bad: the folded `encoding_time_sec` slightly overstates pure encoding time for legacy records whose search time was never recorded separately — accepted, since estimation already consumed the combined value

## More Information

Companion decision: [ADR-001](001-replace-alias-records-with-path-normalization.md) (the re-keying this rewrite performs). `docs/HISTORY_FORMAT.md` is updated when the implementation lands. The estimation percentile cache moving out of `HistoryIndex` (issue #22) is an implementation detail riding the same change, not part of this decision. Audit details: issue #22 discussion, 2026-07-18.
