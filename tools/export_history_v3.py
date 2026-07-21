#!/usr/bin/env python3
"""Export a V2 (Python app) conversion history for import into CRFty.

Reads the V2 app's schema-v2 ``conversion_history.json`` and writes the
versioned exchange file CRFty imports (``docs/HISTORY_IMPORT.md``,
``import_version`` 1). All legacy interpretation happens here — the app
itself knows nothing about the V2 formats:

- paths come from ``original_path`` with mapped network drives resolved to
  their UNC spelling, matching how CRFty canonicalizes queued files
- floats become integers: CRF x 1000, VMAF x 100, seconds to milliseconds,
  mtime to a nanoseconds decimal string
- ISO timestamps (naive local time) become milliseconds since the Unix epoch
- fields that are missing, non-finite, or out of the import schema's range
  are omitted rather than guessed; only ``path`` and ``status`` are required
- anonymized records (no ``original_path``) are dropped with a report: a
  hashed path can never match a real file

Run this on the machine that produced the history file: drive mappings and
the local timezone are read from the running system. A legacy unversioned
array must first be migrated with the V2 app's ``tools/migrate_history_v2.py``.

Usage:
    python tools/export_history_v3.py <history-json> [-o output-json]

The default output is ``crfty_history_import.json`` beside the history file.
Point CRFty's Settings -> History -> "Import history" at the written file.

Standalone by design: stdlib only, no imports from either codebase, so it
runs anywhere a Python 3 interpreter exists. Tests live beside it
(``uvx pytest tools/test_export_history_v3.py``).

Privacy note: this tool prints record counts only, never paths or filenames
from the history contents.
"""

import argparse
import ctypes
import datetime
import json
import math
import os
import sys

# The V2 history container this script reads: {"schema_version": 2, "records": [...]}.
_V2_SCHEMA_VERSION = 2

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

# --- Mapped network drive resolution (inlined from the V2 app) ---

# Cache of drive letter ("b:") -> UNC root (r"\\server\share") or None if not
# a mapped network drive. Mappings survive for the process lifetime.
_drive_unc_cache: dict[str, str | None] = {}

_WNET_BUFFER_CHARS = 1024


def _query_drive_unc(drive: str) -> str | None:
    """Look up the UNC root for a drive letter via WNetGetConnectionW.

    Reads the local drive-mapping table only - no network I/O, so it cannot
    block on an offline share.

    Args:
        drive: Drive spec like "B:" (no trailing separator).

    Returns:
        UNC root like r"\\\\server\\share", or None if the drive is not a
        mapped network drive or the lookup fails.
    """
    if sys.platform != "win32":  # Callers guard too; repeated so type-checkers narrow windll
        return None
    buffer = ctypes.create_unicode_buffer(_WNET_BUFFER_CHARS)
    length = ctypes.c_ulong(_WNET_BUFFER_CHARS)
    try:
        result = ctypes.windll.mpr.WNetGetConnectionW(drive, buffer, ctypes.byref(length))
    except OSError as error:
        print(f"Warning: drive mapping lookup failed for {drive} ({error}); leaving paths as-is.", file=sys.stderr)
        return None
    if result != 0:  # ERROR_NOT_CONNECTED, ERROR_BAD_DEVICE, etc. - a local drive
        return None
    return buffer.value or None


def resolve_mapped_drive_path(path: str) -> str:
    """Rewrite a mapped-network-drive path to its UNC spelling.

    ``B:\\videos\\x.mp4`` becomes ``\\\\server\\share\\videos\\x.mp4`` when B:
    is a mapped network drive, so the exported path matches the spelling CRFty
    sees for queued files. Local drives, UNC paths, and all non-Windows paths
    pass through unchanged.

    Args:
        path: Absolute or relative path.

    Returns:
        The path with its drive prefix replaced by the UNC root, or the input
        unchanged.
    """
    if sys.platform != "win32":
        return path
    if len(path) < 2 or path[1] != ":" or not path[0].isalpha():
        return path
    drive = path[:2].lower()
    if drive not in _drive_unc_cache:
        _drive_unc_cache[drive] = _query_drive_unc(drive.upper())
    unc_root = _drive_unc_cache[drive]
    if unc_root is None:
        return path
    return unc_root.rstrip("\\") + path[2:]


# --- Export transform ---


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
    parser = argparse.ArgumentParser(description="Export a V2 conversion history for import into CRFty.")
    parser.add_argument("path", help="The V2 app's conversion_history.json to export")
    parser.add_argument(
        "-o",
        "--output",
        default=None,
        help=f"Where to write the import file (default: {DEFAULT_OUTPUT_NAME} beside the history file)",
    )
    args = parser.parse_args()
    history_path = args.path

    if not os.path.exists(history_path):
        print(f"No history file at {history_path}; nothing to export.")
        return 1

    with open(history_path, encoding="utf-8") as f:
        data = json.load(f)

    if isinstance(data, list):
        print("Legacy unversioned history; run the V2 app's tools/migrate_history_v2.py first, then re-run this export.")
        return 1
    if not isinstance(data, dict) or data.get("schema_version") != _V2_SCHEMA_VERSION:
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
