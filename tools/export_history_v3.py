#!/usr/bin/env python3
"""Export conversion history for import into CRFty (the V3 rewrite).

Reads the schema-v2 ``conversion_history.json`` and writes the versioned
exchange file CRFty imports (``docs/HISTORY_IMPORT.md`` on the ``rewrite``
branch, ``import_version`` 1). All legacy interpretation happens here — the
V3 app knows nothing about this application's formats:

- paths come from ``original_path`` with mapped network drives resolved to
  their UNC spelling, matching how V3 canonicalizes queued files
- floats become integers: CRF x 1000, VMAF x 100, seconds to milliseconds,
  mtime to a nanoseconds decimal string
- ISO timestamps (naive local time) become milliseconds since the Unix epoch
- fields that are missing, non-finite, or out of the import schema's range
  are omitted rather than guessed; only ``path`` and ``status`` are required
- anonymized records (no ``original_path``) are dropped with a report: a
  hashed path can never match a real file

Run this on the machine that produced the history file: drive mappings and
the local timezone are read from the running system. A legacy unversioned
array must first be migrated with ``tools/migrate_history_v2.py``.

Usage:
    python tools/export_history_v3.py [history-json] [-o output-json]

The default output is ``crfty_history_import.json`` beside the history file.
Point CRFty's Settings -> History -> "Import history" at the written file.

Privacy note: this tool prints record counts only, never paths or filenames
from the history contents.
"""

import argparse
import datetime
import json
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from src.config import HISTORY_SCHEMA_VERSION
from src.history_index import get_history_path
from src.platform_utils import resolve_mapped_drive_path

IMPORT_VERSION = 1
DEFAULT_OUTPUT_NAME = "crfty_history_import.json"

_STATUSES = ("scanned", "analyzed", "not_worthwhile", "converted")

# JSON-safe integer ceiling: the import schema requires 64-bit values to fit
# in a double's exact-integer range (modified_ns is a string and is exempt).
_MAX_SAFE_INT = 2**53 - 1

_MAX_VMAF_HUNDREDTHS = 10_000
_MAX_TARGET = 100

_NS_PER_SECOND = 1_000_000_000
_MS_PER_SECOND = 1_000


def _safe_int(value, maximum: int = _MAX_SAFE_INT) -> int | None:
    """An int within [0, maximum], or None. Floats are accepted when finite."""
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, float):
        if not math.isfinite(value):
            return None
        value = round(value)
    if not isinstance(value, int):
        return None
    if value < 0 or value > maximum:
        return None
    return value


def _scaled_int(value, scale: int, maximum: int = _MAX_SAFE_INT) -> int | None:
    """``round(value * scale)`` within [0, maximum], or None."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return _safe_int(round(value * scale), maximum)


def _timestamp_ms(value) -> int | None:
    """ISO timestamp (naive = local time) to Unix milliseconds, or None."""
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.datetime.fromisoformat(value)
    except ValueError:
        return None
    milliseconds = round(parsed.timestamp() * _MS_PER_SECOND)
    return milliseconds if milliseconds >= 0 else None


def _set(record: dict, field: str, value: int | None) -> None:
    """Store an optional field; missing or out-of-schema data is omitted."""
    if value is not None:
        record[field] = value


def export_records(records: list[dict], resolve_path=resolve_mapped_drive_path) -> tuple[list[dict], dict[str, int]]:
    """Transform schema-v2 record dicts into import-schema record dicts.

    Pure JSON-level transform (no I/O). See the module docstring for the
    rules.

    Args:
        records: Record dicts from the schema-v2 container.
        resolve_path: Callable(path) -> path with mapped drives resolved;
            injectable so tests can simulate drive mappings.

    Returns:
        (exported record list, stats counters).
    """
    stats = {
        "total_in": len(records),
        "dropped_anonymized": 0,
        "dropped_unknown_status": 0,
        "missing_decided_at": 0,
        "total_out": 0,
    }

    exported: list[dict] = []
    for source in records:
        original_path = source.get("original_path")
        if not isinstance(original_path, str) or not original_path:
            stats["dropped_anonymized"] += 1
            continue
        status = source.get("status")
        if status not in _STATUSES:
            stats["dropped_unknown_status"] += 1
            continue

        record: dict = {"path": resolve_path(original_path), "status": status}

        _set(record, "size", _safe_int(source.get("file_size_bytes")))
        mtime = source.get("file_mtime")
        modified_ns = _scaled_int(mtime, _NS_PER_SECOND, maximum=sys.maxsize)
        if modified_ns is not None:
            record["modified_ns"] = str(modified_ns)

        codec = source.get("video_codec")
        if isinstance(codec, str) and codec:
            record["video_codec"] = codec
        _set(record, "width", _safe_int(source.get("width")))
        _set(record, "height", _safe_int(source.get("height")))
        _set(record, "duration_ms", _scaled_int(source.get("duration_sec"), _MS_PER_SECOND))

        if status == "converted":
            _set(record, "output_size", _safe_int(source.get("output_size_bytes")))
            _set(record, "encoding_time_ms", _scaled_int(source.get("encoding_time_sec"), _MS_PER_SECOND))
            crf = source.get("final_crf")
            crf = crf if crf is not None else source.get("best_crf")
            vmaf = source.get("final_vmaf")
            vmaf = vmaf if vmaf is not None else source.get("best_vmaf_achieved")
            target = source.get("vmaf_target_used")
            target = target if target is not None else source.get("vmaf_target_when_analyzed")
        elif status == "analyzed":
            crf = source.get("best_crf")
            vmaf = source.get("best_vmaf_achieved")
            target = source.get("vmaf_target_when_analyzed")
        else:
            crf = vmaf = target = None

        _set(record, "crf_thousandths", _scaled_int(crf, 1000))
        _set(record, "vmaf_hundredths", _scaled_int(vmaf, 100, maximum=_MAX_VMAF_HUNDREDTHS))
        _set(record, "target", _safe_int(target, maximum=_MAX_TARGET))

        if status == "not_worthwhile":
            _set(record, "requested_target", _safe_int(source.get("vmaf_target_attempted"), maximum=_MAX_TARGET))
            _set(record, "floor_target", _safe_int(source.get("min_vmaf_attempted"), maximum=_MAX_TARGET))

        decided_at = _timestamp_ms(source.get("last_updated"))
        if decided_at is None:
            decided_at = _timestamp_ms(source.get("first_seen"))
        if decided_at is None:
            # The import treats a missing decided_at as the import instant.
            stats["missing_decided_at"] += 1
        else:
            record["decided_at_ms"] = decided_at

        exported.append(record)

    exported.sort(key=lambda record: record["path"])
    stats["total_out"] = len(exported)
    return exported, stats


def main() -> int:
    parser = argparse.ArgumentParser(description="Export conversion history for import into CRFty (V3).")
    parser.add_argument(
        "path", nargs="?", default=None, help="History file to export (default: the app's conversion_history.json)"
    )
    parser.add_argument(
        "-o",
        "--output",
        default=None,
        help=f"Where to write the import file (default: {DEFAULT_OUTPUT_NAME} beside the history file)",
    )
    args = parser.parse_args()
    history_path = args.path or get_history_path()

    if not os.path.exists(history_path):
        print(f"No history file at {history_path}; nothing to export.")
        return 1

    with open(history_path, encoding="utf-8") as f:
        data = json.load(f)

    if isinstance(data, list):
        print("Legacy unversioned history; run tools/migrate_history_v2.py first, then re-run this export.")
        return 1
    if not isinstance(data, dict) or data.get("schema_version") != HISTORY_SCHEMA_VERSION:
        version = data.get("schema_version") if isinstance(data, dict) else None
        print(f"Unsupported container (schema_version {version!r}); refusing to export it.")
        return 1
    records = data.get("records")
    if not isinstance(records, list):
        print("Unrecognized history file structure; refusing to export it.")
        return 1

    exported, stats = export_records(records)
    payload = {"import_version": IMPORT_VERSION, "records": exported}

    output_path = args.output or os.path.join(os.path.dirname(os.path.abspath(history_path)), DEFAULT_OUTPUT_NAME)
    temporary = output_path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as f:
        json.dump(payload, f, ensure_ascii=False, separators=(",", ":"))
    os.replace(temporary, output_path)

    print(f"Exported {stats['total_out']} of {stats['total_in']} records to {output_path}")
    for key in ("dropped_anonymized", "dropped_unknown_status", "missing_decided_at"):
        if stats[key]:
            print(f"  {key}: {stats[key]}")
    print("Import via CRFty: Settings -> History -> Import history.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
