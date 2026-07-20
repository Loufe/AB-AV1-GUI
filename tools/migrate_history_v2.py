#!/usr/bin/env python3
"""One-time migration of conversion_history.json to schema v2 (ADR-002).

Transforms the legacy unversioned JSON array into the versioned container
``{"schema_version": 2, "records": [...]}``:

- drops alias records (``duplicate_of`` set) and the field itself (ADR-001)
- folds the legacy combined time: ``encoding_time_sec = conversion_time_sec -
  (crf_search_time_sec or 0)``, then drops ``conversion_time_sec``
- normalizes pre-ANALYZED-era records (status ``scanned`` with Layer-2 results)
  to status ``analyzed``
- re-keys records with a stored ``original_path`` under the current path hasher,
  which resolves mapped network drives to their UNC spelling (ADR-001)
- merges records that collide on the same new key (highest status wins,
  ties broken by last_updated; earliest first_seen preserved)
- drops anonymized records (no ``original_path``): they cannot be re-keyed and
  self-heal on the next scan (ADR-001)
- scrubs non-finite floats field-by-field with a warning
- drops keys that are no longer FileRecord fields

Run this on the machine that produced the history file: path hashing is
platform-dependent (Windows lowercasing, live drive mappings). The original
file is preserved as ``<file>.bak``. Idempotent: an already-migrated file is
left untouched.

Usage:
    python tools/migrate_history_v2.py [path-to-history-json]

Privacy note: this tool prints record counts only, never paths or filenames
from the history contents.
"""

import argparse
import dataclasses
import json
import math
import os
import sys
from collections.abc import Callable

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from src.config import HISTORY_SCHEMA_VERSION
from src.history_index import compute_path_hash, get_history_path
from src.models import FileRecord
from src.platform_utils import resolve_mapped_drive_path

# Higher number = better status; mirrors find_better_duplicate's priority order.
_STATUS_PRIORITY = {"converted": 4, "not_worthwhile": 3, "analyzed": 2, "scanned": 1}

_KNOWN_FIELDS = {f.name for f in dataclasses.fields(FileRecord)}


def _scrub_nonfinite(value: dict | list) -> int:
    """Replace non-finite floats (NaN/Infinity) with None, recursively.

    serde_json in the Rust port rejects non-finite JSON, so v2 guarantees clean
    data (ADR-002; the production audit found zero occurrences).

    Returns:
        Number of values scrubbed.
    """
    scrubbed = 0
    items = value.items() if isinstance(value, dict) else enumerate(value)
    for key, item in items:
        if isinstance(item, float) and not math.isfinite(item):
            value[key] = None
            scrubbed += 1
        elif isinstance(item, dict | list):
            scrubbed += _scrub_nonfinite(item)
    return scrubbed


def _default_rekey(original_path: str) -> tuple[str, str]:
    """Re-key a stored path under the current hasher (mapped drives -> UNC).

    Returns:
        (new_path_hash, new_original_path) - the path keeps its stored spelling
        except that a mapped-drive prefix is rewritten to its UNC root.
    """
    resolved = resolve_mapped_drive_path(original_path)
    return compute_path_hash(resolved), resolved


def migrate_records(
    records: list[dict], rekey: Callable[[str], tuple[str, str]] = _default_rekey
) -> tuple[list[dict], dict[str, int]]:
    """Transform legacy record dicts into schema-v2 record dicts.

    Pure JSON-level transform (no I/O). See the module docstring for the rules.

    Args:
        records: Record dicts from the legacy array file.
        rekey: Callable(original_path) -> (new_path_hash, new_original_path);
            injectable so tests can simulate drive mappings.

    Returns:
        (migrated record list, stats counters).
    """
    stats = {
        "total_in": len(records),
        "dropped_aliases": 0,
        "dropped_anonymized": 0,
        "folded_times": 0,
        "normalized_statuses": 0,
        "rekeyed": 0,
        "merged": 0,
        "scrubbed_nonfinite": 0,
        "dropped_unknown_keys": 0,
        "total_out": 0,
    }

    by_hash: dict[str, dict] = {}
    for original in records:
        if original.get("audio_codec") is not None and "audio_streams" not in original:
            raise ValueError("History contains pre-audio_streams records; this format predates supported migration.")
        if original.get("duplicate_of"):
            stats["dropped_aliases"] += 1
            continue
        if not original.get("original_path"):
            # Anonymized records cannot be re-keyed under the normalized hasher;
            # they are dropped and self-heal on the next scan (ADR-001)
            stats["dropped_anonymized"] += 1
            continue

        record = {k: v for k, v in original.items() if k in _KNOWN_FIELDS or k == "conversion_time_sec"}
        dropped = len(original) - len(record) - ("duplicate_of" in original)
        stats["dropped_unknown_keys"] += max(0, dropped)

        stats["scrubbed_nonfinite"] += _scrub_nonfinite(record)

        legacy_time = record.pop("conversion_time_sec", None)
        if legacy_time is not None and record.get("encoding_time_sec") is None:
            # The legacy field held search + encode combined; recover the encode
            # share where the search time was recorded separately (ADR-002)
            record["encoding_time_sec"] = max(0.0, legacy_time - (record.get("crf_search_time_sec") or 0))
            stats["folded_times"] += 1

        if (
            record.get("status") == "scanned"
            and record.get("best_crf") is not None
            and record.get("best_vmaf_achieved") is not None
        ):
            record["status"] = "analyzed"
            stats["normalized_statuses"] += 1

        new_hash, new_path = rekey(record["original_path"])
        if new_hash != record.get("path_hash") or new_path != record["original_path"]:
            stats["rekeyed"] += 1
        record["path_hash"] = new_hash
        record["original_path"] = new_path

        key = record["path_hash"]
        existing = by_hash.get(key)
        if existing is None:
            by_hash[key] = record
            continue

        # Collision: the same file recorded under two spellings. Keep the better
        # verdict; preserve the earliest first_seen across both.
        stats["merged"] += 1
        winner, loser = existing, record
        winner_rank = (_STATUS_PRIORITY.get(existing.get("status"), 0), existing.get("last_updated") or "")
        loser_rank = (_STATUS_PRIORITY.get(record.get("status"), 0), record.get("last_updated") or "")
        if loser_rank > winner_rank:
            winner, loser = record, existing
        seen_stamps = [s for s in (winner.get("first_seen"), loser.get("first_seen")) if s]
        if seen_stamps:
            winner["first_seen"] = min(seen_stamps)
        by_hash[key] = winner

    migrated = list(by_hash.values())
    stats["total_out"] = len(migrated)
    return migrated, stats


def main() -> int:
    parser = argparse.ArgumentParser(description="Migrate conversion_history.json to schema v2 (ADR-002).")
    parser.add_argument(
        "path", nargs="?", default=None, help="History file to migrate (default: the app's conversion_history.json)"
    )
    args = parser.parse_args()
    history_path = args.path or get_history_path()

    if not os.path.exists(history_path):
        print(f"No history file at {history_path}; nothing to migrate.")
        return 0

    with open(history_path, encoding="utf-8") as f:
        data = json.load(f)

    if isinstance(data, dict):
        if data.get("schema_version") == HISTORY_SCHEMA_VERSION:
            print(f"Already schema v{HISTORY_SCHEMA_VERSION}; nothing to do.")
            return 0
        print(f"Unsupported container (schema_version {data.get('schema_version')!r}); refusing to touch it.")
        return 1
    if not isinstance(data, list):
        print("Unrecognized history file structure; refusing to touch it.")
        return 1

    migrated, stats = migrate_records(data)

    backup_path = history_path + ".bak"
    if os.path.exists(backup_path):
        print(f"Backup already exists, refusing to overwrite it: {backup_path}")
        return 1
    os.rename(history_path, backup_path)

    temp_path = history_path + ".tmp"
    with open(temp_path, "w", encoding="utf-8") as f:
        # allow_nan=False backstops the scrub: v2 must never contain non-finite JSON
        json.dump({"schema_version": HISTORY_SCHEMA_VERSION, "records": migrated}, f, indent=2, allow_nan=False)
    os.replace(temp_path, history_path)

    print(f"Migrated {history_path} to schema v{HISTORY_SCHEMA_VERSION} (backup: {backup_path})")
    for name, value in stats.items():
        print(f"  {name}: {value}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
