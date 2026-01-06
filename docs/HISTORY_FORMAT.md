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

The JSON file is an **array** of FileRecord objects:

```json
[
  { "path_hash": "a1b2c3d4e5f6...", "status": "scanned", ... },
  { "path_hash": "f6e5d4c3b2a1...", "status": "converted", ... }
]
```

**Note**: The file format is an array for simplicity. At runtime, `HistoryIndex` loads this into a dictionary keyed by `path_hash` for O(1) lookups.

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
| `best_crf` | int\|null | Optimal CRF value found |
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
| `final_crf` | int\|null | CRF used for encoding |
| `final_vmaf` | float\|null | Actual VMAF achieved |
| `vmaf_target_used` | int\|null | Target (may differ from requested due to fallback) |
| `output_audio_codec` | string\|null | Audio codec in output |

### Legacy Fields

| Field | Type | Description |
|-------|------|-------------|
| `conversion_time_sec` | float\|null | Old combined time (pre-split) |

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
| 2 | ANALYZED | Has best_crf AND best_vmaf_achieved |
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

## Implementation

- **Dataclass**: `FileRecord` in `src/models.py`
- **Index**: `HistoryIndex` in `src/history_index.py` provides O(1) lookups
- **Persistence**: JSON with atomic writes via `os.replace()`
