# History File Format

This document describes the structure of `conversion_history_v2.json`, the persistent storage for file metadata and conversion history.

## Overview

The history file is a JSON object mapping path hashes to `FileRecord` objects. It serves multiple purposes:

- **Cache**: Skip ffprobe for files with unchanged size/mtime
- **Estimation**: Use past conversions to predict future encoding times
- **Status tracking**: Remember which files are converted, analyzed, or not worthwhile
- **Privacy**: Optionally store only hashes, not actual paths

## File Location

Default: `conversion_history_v2.json` in the application directory.

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
| `audio_codec` | string\|null | e.g., "aac", "opus" |
| `width` | int\|null | Video width in pixels |
| `height` | int\|null | Video height in pixels |
| `bitrate_kbps` | float\|null | Overall bitrate |

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

## Example Records

### Scanned File (Layer 1)

```json
{
  "path_hash": "a1b2c3...",
  "original_path": "/videos/movie.mp4",
  "status": "scanned",
  "file_size_bytes": 1500000000,
  "file_mtime": 1704067200.0,
  "duration_sec": 7200.0,
  "video_codec": "h264",
  "audio_codec": "aac",
  "width": 1920,
  "height": 1080,
  "bitrate_kbps": 5000.0,
  "estimated_reduction_percent": 45.0,
  "first_seen": "2024-01-01T00:00:00",
  "last_updated": "2024-01-01T00:00:00"
}
```

### Analyzed File (Layer 2)

```json
{
  "path_hash": "b2c3d4...",
  "status": "analyzed",
  "file_size_bytes": 2000000000,
  "file_mtime": 1704067200.0,
  "duration_sec": 5400.0,
  "video_codec": "hevc",
  "width": 3840,
  "height": 2160,
  "best_crf": 28,
  "best_vmaf_achieved": 95.2,
  "predicted_output_size": 800000000,
  "predicted_size_reduction": 60.0,
  "vmaf_target_when_analyzed": 95,
  "crf_search_time_sec": 45.0
}
```

### Converted File (Layer 3)

```json
{
  "path_hash": "c3d4e5...",
  "status": "converted",
  "file_size_bytes": 1000000000,
  "file_mtime": 1704067200.0,
  "duration_sec": 3600.0,
  "video_codec": "h264",
  "width": 1920,
  "height": 1080,
  "best_crf": 30,
  "best_vmaf_achieved": 95.5,
  "output_size_bytes": 450000000,
  "reduction_percent": 55.0,
  "final_crf": 30,
  "final_vmaf": 95.4,
  "vmaf_target_used": 95,
  "crf_search_time_sec": 30.0,
  "encoding_time_sec": 1800.0
}
```

### Not Worthwhile File

```json
{
  "path_hash": "d4e5f6...",
  "status": "not_worthwhile",
  "file_size_bytes": 500000000,
  "file_mtime": 1704067200.0,
  "duration_sec": 1800.0,
  "video_codec": "h264",
  "width": 1920,
  "height": 1080,
  "vmaf_target_attempted": 95,
  "min_vmaf_attempted": 90,
  "skip_reason": "Could not achieve VMAF 90 even at lowest CRF"
}
```

## Implementation

- **Dataclass**: `FileRecord` in `src/models.py`
- **Index**: `HistoryIndex` in `src/history_index.py` provides O(1) lookups
- **Persistence**: JSON with atomic writes via `os.replace()`
