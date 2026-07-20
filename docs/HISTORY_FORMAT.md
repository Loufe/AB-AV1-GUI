# History File Format

This document describes the structure of `conversion_history.json`, the persistent storage for file metadata and conversion history.

## Overview

The history file is a JSON object mapping path hashes to `FileRecord` objects. It serves multiple purposes:

- **Cache**: Skip ffprobe for files with unchanged size/mtime
- **Estimation**: Use past conversions to predict future encoding times
- **Status tracking**: Remember which files are converted, analyzed, or not worthwhile
- **Privacy**: Optionally store only hashes, not actual paths

## File Location

Default: `conversion_history.json` in the application directory.

## Top-Level Structure

The JSON file is a **versioned container** (schema v2, ADR-002) holding an array of FileRecord objects:

```json
{
  "schema_version": 2,
  "records": [
    { "path_hash": "a1b2c3d4e5f6...", "status": "scanned", ... },
    { "path_hash": "f6e5d4c3b2a1...", "status": "converted", ... }
  ]
}
```

The loader accepts only `schema_version` 2 and raises on the legacy unversioned array with instructions to run `tools/migrate_history_v2.py` — no key sniffing. At runtime, `HistoryIndex` loads the records into a dictionary keyed by `path_hash` for O(1) lookups.

`path_hash` is BLAKE2b (16-byte digest) of the normalized path, truncated to 16 hex characters. Normalization: absolute path, mapped network drives resolved to their UNC spelling, lowercased on Windows, backslashes to forward slashes.

## FileRecord Fields

### Identity

| Field | Type | Description |
|-------|------|-------------|
| `path_hash` | string | BLAKE2b hash of normalized path (primary key) |
| `original_path` | string\|null | Full path if anonymization OFF, else null |
| `status` | string | One of: `scanned`, `analyzed`, `not_worthwhile`, `converted` |

### Cache Validation

| Field | Type | Description |
|-------|------|-------------|
| `file_size_bytes` | int | File size for change detection |
| `file_mtime` | float | Modification time for change detection |

Cache is valid if both size AND mtime match the current file (mtime has 1-second tolerance due to JSON precision loss).

### Duplicate Detection (ADR-001, ADR-002)

Used to recognize the same physical file accessed via different paths.
See [ADR-001](adr/001-use-metadata-for-duplicate-detection.md) for the metadata cascade and
[ADR-002](adr/002-adopt-versioned-history-schema-v2.md) for the read-time resolution model.

| Field | Type | Description |
|-------|------|-------------|
| `filename_hash` | string\|null | BLAKE2b hash of the basename (includes extension); enables matching when `original_path` is null (anonymized) |

Duplicates are never persisted: every record is canonical for its own path. When display or
the worker needs a verdict, `find_better_duplicate()` resolves it at read time.

### Video Metadata (Layer 1 - ffprobe)

Populated during "Basic Scan" or first analysis.

| Field | Type | Description |
|-------|------|-------------|
| `duration_sec` | float\|null | Video duration in seconds |
| `video_codec` | string\|null | e.g., "h264", "hevc", "av1" |
| `audio_streams` | array | List of audio stream objects (see below) |
| `width` | int\|null | Video width in pixels |
| `height` | int\|null | Video height in pixels |
| `bitrate_kbps` | float\|null | Overall bitrate |

#### Audio Stream Object

Each entry in `audio_streams` contains:

| Field | Type | Description |
|-------|------|-------------|
| `codec` | string | Audio codec, e.g., "aac", "opus", "ac3" |
| `channels` | int\|null | Number of audio channels |
| `bitrate_kbps` | float\|null | Stream bitrate in kbps |
| `language` | string\|null | Language code, e.g., "eng", "jpn" |

### Estimation (Layer 1)

| Field | Type | Description |
|-------|------|-------------|
| `estimated_reduction_percent` | float\|null | Rough size reduction estimate |
| `estimated_from_similar` | int\|null | Count of similar files used |

These are rough estimates shown with "~" prefix in the UI.

### VMAF Analysis (Layer 2 - CRF Search)

Populated after running CRF search (either standalone ANALYZE or as part of CONVERT).

| Field | Type | Description |
|-------|------|-------------|
| `vmaf_target_when_analyzed` | int\|null | VMAF target achieved |
| `preset_when_analyzed` | int\|null | SVT-AV1 preset used |
| `best_crf` | float\|null | Optimal CRF value found (fractional since ab-av1 0.11) |
| `best_vmaf_achieved` | float\|null | VMAF score at best CRF |
| `predicted_output_size` | int\|null | Predicted output bytes |
| `predicted_size_reduction` | float\|null | Accurate reduction % |
| `crf_search_time_sec` | float\|null | How long CRF search took |

These are accurate predictions shown WITHOUT "~" prefix.

### Not Worthwhile (Failed CRF Search)

When VMAF target can't be met even at minimum fallback.

| Field | Type | Description |
|-------|------|-------------|
| `vmaf_target_attempted` | int\|null | Initial VMAF target |
| `min_vmaf_attempted` | int\|null | Lowest target tried (e.g., 90) |
| `skip_reason` | string\|null | Human-readable reason |

### Conversion Results (Layer 3)

Populated after successful encoding.

| Field | Type | Description |
|-------|------|-------------|
| `output_path` | string\|null | Output path (or hash if anonymized) |
| `output_size_bytes` | int\|null | Actual output file size |
| `reduction_percent` | float\|null | Actual size reduction |
| `encoding_time_sec` | float\|null | Encoding phase duration |
| `final_crf` | float\|null | CRF used for encoding (fractional since ab-av1 0.11) |
| `final_vmaf` | float\|null | Actual VMAF achieved |
| `vmaf_target_used` | int\|null | Target (may differ from requested due to fallback) |
| `output_audio_codec` | string\|null | Audio codec in output |

### Timestamps

| Field | Type | Description |
|-------|------|-------------|
| `first_seen` | string\|null | ISO timestamp when first scanned |
| `last_updated` | string\|null | ISO timestamp of last status change |

## Status Values

| Status | Meaning | Typical Fields Present |
|--------|---------|----------------------|
| `scanned` | ffprobe complete | Layer 1 metadata |
| `analyzed` | CRF search complete | Layer 1 + Layer 2 |
| `not_worthwhile` | Can't meet VMAF target | Layer 1 + skip_reason |
| `converted` | Successfully encoded | Layer 1 + Layer 2 + Layer 3 |

## Analysis Levels

The `FileRecord.get_analysis_level()` method maps status to analysis level:

| Level | Name | Criteria |
|-------|------|----------|
| 0 | DISCOVERED | No record exists |
| 1 | SCANNED | Has video_codec or duration_sec |
| 2 | ANALYZED | Status is "analyzed" |
| 3 | CONVERTED | Status is "converted" |

## Fields by Status

All records have identity fields (`path_hash`, `status`) and cache fields (`file_size_bytes`, `file_mtime`). Additional fields depend on status:

| Field Group | `scanned` | `analyzed` | `not_worthwhile` | `converted` |
|-------------|:---------:|:----------:|:----------------:|:-----------:|
| **Layer 1** (ffprobe metadata) | ✓ | ✓ | ✓ | ✓ |
| **Layer 1** (estimates) | ✓ | ✓ | — | ✓ |
| **Layer 2** (CRF search results) | — | ✓ | — | ✓ |
| **Skip reason fields** | — | — | ✓ | — |
| **Layer 3** (conversion results) | — | — | — | ✓ |

**Duplicate paths** (same physical file recorded under two paths, ADR-001/ADR-002): each path keeps its own canonical record (usually SCANNED, holding that path's probe results and cache stamps). The decided verdict is resolved at read time — the Analysis display and the worker's pre-processing short-circuit call `find_better_duplicate()` and use the source's verdict directly without persisting anything for the duplicate path, so deleting or re-deriving the source takes effect on the next read.

ANALYZED / NOT_WORTHWHILE verdicts are discarded with stale content on re-scan (cache stamps no longer match).

Canonical CONVERTED records survive a stamp mismatch only while the file is plausibly the conversion's own output (in replace mode the AV1 output sits at the input path, so changed stamps are the expected steady state there). Two checks recognize this, depending on what data is available:

- **Basic Scan** (`folder_analysis._analyze_file`): ffprobe has run, so the probed codec decides — AV1 (or an unreadable file) preserves the record with refreshed metadata; anything else cannot be our output, and the record is demoted like the other verdicts (removing that conversion from statistics, since its input no longer exists at the path).
- **Stat-only paths** (queue filter, queue reconciliation, worker duplicate short-circuit — no ffprobe allowed): `cache_helpers.converted_verdict_applies()` keeps the verdict if the stamps still match, or if the path is `.mkv` and the current size equals the recorded `output_size_bytes` (replace mode always outputs `input.with_suffix(".mkv")`, so only a `.mkv` path can hold our output). Legacy records without `output_size_bytes` are kept conservatively. Any other mismatch means the content changed and the file is re-queueable.

## Implementation

- **Dataclass**: `FileRecord` in `src/models.py`
- **Index**: `HistoryIndex` in `src/history_index.py` provides O(1) lookups
- **Persistence**: JSON with atomic writes via `os.replace()`
