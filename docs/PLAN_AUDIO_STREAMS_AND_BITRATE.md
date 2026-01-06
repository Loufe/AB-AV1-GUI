# Plan: Audio Stream Info & Bitrate Fix

## Overview

Fix missing bitrate fields in history records and add detailed audio stream information (codec, language, title, channels) to support proper display in the History tab.

**Two bitrate values tracked:**
1. `bitrate_kbps` - Overall content bitrate (video + audio). From ffprobe `format.bit_rate`.
2. `audio_streams[].bitrate_kbps` - Per-audio-stream bitrate (NEW)

**Derived value (not stored):**
- Video bitrate = `bitrate_kbps - sum(audio_streams[].bitrate_kbps)` when needed

## Execution Order

1. **Implement** - Complete Steps 1-8 (code changes)
2. **Migrate** - Run migration script on existing history (Step 9)
3. **Test** - Verify everything works

The migration script imports `extract_video_metadata()` which must return `audio_streams`, so implementation comes first. Migration creates a backup, so recovery is safe if issues arise.

## Current State

### Problem 1: Bitrate Not Saved by Worker
- `folder_analysis.py:373` sets `bitrate_kbps=meta.bitrate_kbps` during basic scan ✓
- `worker.py:_create_file_record()` does NOT include `bitrate_kbps` when creating records ✗
- Result: Files converted without prior Analysis tab scanning have no bitrate in history

### Problem 2: Only First Audio Stream Saved
- `VideoMetadata` has `audio_codec` (first stream only) and `audio_stream_count` (just a count)
- `FileRecord` has only `audio_codec` (first stream)
- No language, title, or channel info stored
- Cannot display "AAC, AC3" or "English (AAC), Commentary (AC3)"

## Proposed Data Model Changes

### New AudioStreamInfo Dataclass (src/models.py)

```python
@dataclass
class AudioStreamInfo:
    """Information about a single audio stream."""
    codec: str                        # e.g., "aac", "ac3", "opus"
    language: str | None = None       # e.g., "eng", "jpn", None
    title: str | None = None          # e.g., "English 5.1", "Commentary"
    channels: int | None = None       # e.g., 2 (stereo), 6 (5.1)
    bitrate_kbps: float | None = None # e.g., 128.0, 640.0

    def to_dict(self) -> dict:
        """Convert to JSON-serializable dict."""
        return {
            "codec": self.codec,
            "language": self.language,
            "title": self.title,
            "channels": self.channels,
            "bitrate_kbps": self.bitrate_kbps,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "AudioStreamInfo":
        """Create from dict (for JSON deserialization)."""
        return cls(
            codec=d.get("codec", "unknown"),
            language=d.get("language"),
            title=d.get("title"),
            channels=d.get("channels"),
            bitrate_kbps=d.get("bitrate_kbps"),
        )
```

### Update VideoMetadata (src/models.py)

Add field after existing audio fields (~line 80):
```python
# Detailed audio stream info (all streams)
audio_streams: list[AudioStreamInfo] = field(default_factory=list)
```

Remove these fields (replaced by `audio_streams`):
- `audio_codec` (line 64) - use `audio_streams[0].codec if audio_streams else None`
- `audio_stream_count` (line 76) - use `len(audio_streams)`

Keep these fields (still useful):
- `audio_channels` - first stream channels (commonly accessed)
- `audio_sample_rate` - first stream sample rate
- `audio_bitrate_kbps` - first stream bitrate
- `total_audio_bitrate_kbps` - sum across all streams (used for size estimation)
- `bitrate_kbps` - overall file bitrate

### Update FileRecord (src/models.py)

Add field after `bitrate_kbps` (~line 318):
```python
# Detailed audio stream info (JSON-serializable)
audio_streams: list[dict] = field(default_factory=list)
```

Remove:
- `audio_codec` (line 315) - use `audio_streams[0].get("codec") if audio_streams else None`

**Note:** Using `list[dict]` for direct JSON serialization. Each dict follows AudioStreamInfo structure.

## Implementation Steps

### Step 1: Update models.py

1. Add `AudioStreamInfo` dataclass before `VideoMetadata` class
2. Add `audio_streams: list[AudioStreamInfo] = field(default_factory=list)` to `VideoMetadata`
3. Remove `audio_codec: str | None = None` from `VideoMetadata` (line 64)
4. Remove `audio_stream_count: int = 0` from `VideoMetadata` (line 76)
5. Add `audio_streams: list[dict] = field(default_factory=list)` to `FileRecord`
6. Remove `audio_codec: str | None = None` from `FileRecord` (line 315)

### Step 2: Update video_metadata.py

Modify `extract_video_metadata()` to collect all audio streams.

#### 2a: Add import
```python
from src.models import AudioStreamInfo
```

#### 2b: Initialize before loop
```python
audio_streams: list[AudioStreamInfo] = []
```

Note: Keep `audio_stream_count` as a LOCAL counter variable (not returned).

#### 2c: Extract audio streams with metadata

Replace current audio handling (~lines 79-101):
```python
elif codec_type == "audio":
    audio_stream_count += 1

    # Extract per-stream bitrate
    try:
        stream_bitrate = int(stream.get("bit_rate", 0)) / 1000 or None
    except (ValueError, TypeError):
        stream_bitrate = None

    stream_info = AudioStreamInfo(
        codec=stream.get("codec_name", "unknown"),
        language=stream.get("tags", {}).get("language"),
        title=stream.get("tags", {}).get("title"),
        channels=stream.get("channels"),
        bitrate_kbps=stream_bitrate,
    )
    audio_streams.append(stream_info)

    # Sum bitrates for total
    if stream_bitrate and stream_bitrate > 0:
        total_audio_bitrate_kbps += stream_bitrate
        has_any_audio_bitrate = True

    # First stream info for convenience fields
    if audio_stream_count == 1:
        audio_channels = stream.get("channels")
        try:
            audio_sample_rate = int(stream.get("sample_rate", 0)) or None
        except (ValueError, TypeError):
            audio_sample_rate = None
        audio_bitrate_kbps = stream_bitrate
```

#### 2d: Update return statement

Remove from return:
```python
audio_codec=audio_codec,           # DELETE
audio_stream_count=audio_stream_count,  # DELETE
```

Update:
```python
has_audio=len(audio_streams) > 0,  # Was: audio_stream_count > 0
```

Add:
```python
audio_streams=audio_streams,
```

### Step 3: Update folder_analysis.py

#### 3a: Update `_create_scanned_record()` (~line 362)

Remove:
```python
audio_codec=meta.audio_codec,  # DELETE
```

Add:
```python
audio_streams=[s.to_dict() for s in meta.audio_streams],
```

#### 3b: Update `_update_existing_record_metadata()` (~line 404)

Remove:
```python
audio_codec=meta.audio_codec if meta.audio_codec else existing.audio_codec,  # DELETE
```

Add:
```python
audio_streams=[s.to_dict() for s in meta.audio_streams] if meta.audio_streams else existing.audio_streams,
```

### Step 4: Update worker.py

#### 4a: Update `_create_file_record()` signature (~line 57)

Add parameters:
```python
bitrate_kbps: float | None = None,
audio_streams: list[dict] | None = None,
```

Remove from FileRecord creation:
```python
audio_codec=input_acodec.lower() if input_acodec != "?" else None,  # DELETE (line 116)
```

Add to FileRecord creation:
```python
bitrate_kbps=bitrate_kbps,
audio_streams=audio_streams or [],
```

#### 4b: Update metadata extraction (~line 457)

Initialize before the conditional:
```python
audio_streams_dicts: list[dict] = []
input_bitrate_kbps: float | None = None
```

Inside `if video_info:` block, after `meta = extract_video_metadata(video_info)`:
```python
# Get first audio codec for display (replacing meta.audio_codec)
input_acodec = (meta.audio_streams[0].codec if meta.audio_streams else "?").upper()

# Extract for history record
audio_streams_dicts = [s.to_dict() for s in meta.audio_streams]
input_bitrate_kbps = meta.bitrate_kbps
```

#### 4c: Update all `_create_file_record()` calls

There are 4 call sites that need `bitrate_kbps` and `audio_streams` added:

1. **ANALYZE success** (~line 598):
```python
bitrate_kbps=input_bitrate_kbps,
audio_streams=audio_streams_dicts,
```

2. **ANALYZE not worthwhile** (~line 639):
```python
bitrate_kbps=input_bitrate_kbps,
audio_streams=audio_streams_dicts,
```

3. **CONVERT success** (~line 762):
```python
bitrate_kbps=input_bitrate_kbps,
audio_streams=audio_streams_dicts,
```

4. **NOT_WORTHWHILE from CONVERT** (~line 801):
```python
bitrate_kbps=input_bitrate_kbps,
audio_streams=audio_streams_dicts,
```

### Step 5: Update history_tab.py

Update `compute_history_display_values()` (~line 259):

Replace:
```python
audio = (record.audio_codec or "—").upper()
```

With:
```python
# Audio column - show codecs or count
if record.audio_streams:
    if len(record.audio_streams) == 1:
        audio = record.audio_streams[0].get("codec", "—").upper()
    elif len(record.audio_streams) <= 3:
        # Show all codecs: "AAC, AC3"
        codecs = [s.get("codec", "?").upper() for s in record.audio_streams]
        audio = ", ".join(codecs)
    else:
        # Too many - show count
        audio = f"{len(record.audio_streams)} audio"
else:
    audio = "—"
```

### Step 6: Update tree_display.py

#### 6a: Update `format_stream_display()` signature (~line 203)

Change from:
```python
def format_stream_display(
    video_codec: str | None = None,
    audio_codec: str | None = None,
    audio_stream_count: int = 1,
    subtitle_stream_count: int = 0,
) -> str:
```

To:
```python
def format_stream_display(
    video_codec: str | None = None,
    audio_streams: list[dict] | None = None,
    subtitle_stream_count: int = 0,
) -> str:
```

#### 6b: Update audio formatting logic (~line 229)

Replace:
```python
if audio_stream_count > 1:
    audio = f"{audio_stream_count} audio"
elif audio_codec:
    audio = audio_codec.upper()
else:
    audio = "—"
```

With:
```python
if audio_streams:
    if len(audio_streams) == 1:
        audio = audio_streams[0].get("codec", "—").upper()
    elif len(audio_streams) <= 3:
        codecs = [s.get("codec", "?").upper() for s in audio_streams]
        audio = ", ".join(codecs)
    else:
        audio = f"{len(audio_streams)} audio"
else:
    audio = "—"
```

#### 6c: Update callers

**tree_display.py:155** - Update call in `format_analysis_values()`:
```python
# Change from:
format_str = format_stream_display(record.video_codec, record.audio_codec)
# To:
format_str = format_stream_display(record.video_codec, record.audio_streams)
```

**queue_tree.py:148** - Update call in `_get_streams_display()`:
```python
# Change from:
return format_stream_display(record.video_codec, record.audio_codec)
# To:
return format_stream_display(record.video_codec, record.audio_streams)
```

### Step 7: Update analysis_scanner.py

Review `analysis_scanner.py` for any direct record creation. Based on codebase grep, this file calls functions in `folder_analysis.py` which we're updating in Step 3. No direct changes needed unless there are inline record creations.

Verify: Search for `FileRecord(` in analysis_scanner.py - if none found, no changes needed.

### Step 8: Update HISTORY_FORMAT.md

Add documentation for the new `audio_streams` field:

```markdown
### audio_streams (list of objects)

Detailed information about each audio stream in the source file.

Each object contains:
- `codec` (string): Audio codec name, e.g., "aac", "ac3", "opus"
- `language` (string|null): Language tag, e.g., "eng", "jpn"
- `title` (string|null): Stream title, e.g., "English 5.1", "Commentary"
- `channels` (int|null): Number of audio channels, e.g., 2, 6
- `bitrate_kbps` (float|null): Stream bitrate in kbps

Example:
```json
"audio_streams": [
  {"codec": "aac", "language": "eng", "title": null, "channels": 2, "bitrate_kbps": 128.0},
  {"codec": "ac3", "language": "eng", "title": "Commentary", "channels": 6, "bitrate_kbps": 640.0}
]
```
```

## File Summary

| File | Changes |
|------|---------|
| `src/models.py` | Add `AudioStreamInfo`, add `audio_streams` to `VideoMetadata` and `FileRecord`, remove `audio_codec` and `audio_stream_count` |
| `src/video_metadata.py` | Extract all audio streams, remove `audio_codec`/`audio_stream_count` from return |
| `src/folder_analysis.py` | Pass `audio_streams` when creating/updating records, remove `audio_codec` |
| `src/conversion_engine/worker.py` | Add `bitrate_kbps` and `audio_streams` to `_create_file_record()`, update all call sites |
| `src/gui/tabs/history_tab.py` | Display audio streams properly |
| `src/gui/tree_display.py` | Update `format_stream_display()` signature and logic |
| `src/gui/queue_tree.py` | Update call to `format_stream_display()` |
| `docs/HISTORY_FORMAT.md` | Document new field |

## Migration

### Why Migration Script is Required

When we remove `audio_codec` from `FileRecord`, existing JSON records that contain it will fail deserialization:
```python
record = FileRecord(**record_dict)  # TypeError: unexpected keyword argument 'audio_codec'
```

The migration script transforms old records to the new format by:
1. Adding `audio_streams` (populated via ffprobe)
2. Deleting `audio_codec` (prevents deserialization failure)

### Migration Notes

- **Run AFTER implementing Steps 1-8** - script imports the new code
- Creates timestamped backup before any changes
- Skips anonymized records (no path to ffprobe)
- Skips files that no longer exist on disk
- Also backfills missing `bitrate_kbps` while running ffprobe

## Step 9: Migration Script

Create `tools/migrate_audio_streams.py`:

```python
#!/usr/bin/env python3
"""
One-time migration script to backfill audio_streams for existing history records.

Reads conversion_history_v2.json, runs ffprobe on files that still exist,
and updates records with detailed audio stream info.

IMPORTANT: Run this AFTER implementing the code changes (Steps 1-8).

Usage:
    python tools/migrate_audio_streams.py [--dry-run]

Options:
    --dry-run    Show what would be updated without writing changes
"""

import argparse
import json
import logging
import os
import shutil
import sys
from datetime import datetime
from pathlib import Path

# Add src to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

from src.config import get_history_v2_path
from src.utils import get_video_info
from src.video_metadata import extract_video_metadata

logging.basicConfig(level=logging.INFO, format="%(message)s")
logger = logging.getLogger(__name__)


def backup_history(history_path: Path) -> Path:
    """Create timestamped backup of history file."""
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    backup_path = history_path.with_suffix(f".backup_{timestamp}.json")
    shutil.copy2(history_path, backup_path)
    logger.info(f"Created backup: {backup_path}")
    return backup_path


def load_records(history_path: Path) -> list[dict]:
    """Load raw record dicts from history file."""
    with open(history_path, encoding="utf-8") as f:
        return json.load(f)


def save_records(history_path: Path, records: list[dict]) -> None:
    """Save records to history file with atomic write."""
    temp_path = str(history_path) + ".tmp"
    with open(temp_path, "w", encoding="utf-8") as f:
        json.dump(records, f, indent=2)
    os.replace(temp_path, history_path)


def migrate_record(record: dict) -> tuple[bool, str]:
    """
    Migrate a single record: add audio_streams, remove audio_codec.

    Returns:
        (updated: bool, reason: str)
    """
    # Always remove deprecated field to prevent deserialization failure
    had_audio_codec = "audio_codec" in record
    record.pop("audio_codec", None)

    # Skip if already has audio_streams (just needed audio_codec removal)
    if record.get("audio_streams"):
        if had_audio_codec:
            return True, "removed audio_codec (already has audio_streams)"
        return False, "already migrated"

    # Need original_path to run ffprobe
    original_path = record.get("original_path")
    if not original_path:
        # Still mark as updated if we removed audio_codec
        if had_audio_codec:
            record["audio_streams"] = []
            return True, "removed audio_codec (anonymized, no ffprobe)"
        return False, "no original_path (anonymized)"

    # Check file exists
    if not os.path.isfile(original_path):
        if had_audio_codec:
            record["audio_streams"] = []
            return True, "removed audio_codec (file not found)"
        return False, "file not found"

    # Run ffprobe
    try:
        video_info = get_video_info(original_path)
        if not video_info:
            record["audio_streams"] = []
            return True, "ffprobe failed (set empty)"

        meta = extract_video_metadata(video_info)

        # Convert to dicts and update record
        record["audio_streams"] = [s.to_dict() for s in meta.audio_streams]

        # Also update bitrate if missing
        if not record.get("bitrate_kbps") and meta.bitrate_kbps:
            record["bitrate_kbps"] = meta.bitrate_kbps

        stream_count = len(meta.audio_streams)
        return True, f"added {stream_count} audio stream(s)"

    except Exception as e:
        record["audio_streams"] = []
        return True, f"error ({e}), set empty"


def main():
    parser = argparse.ArgumentParser(description="Migrate history records to add audio_streams")
    parser.add_argument("--dry-run", action="store_true", help="Show changes without writing")
    args = parser.parse_args()

    history_path = Path(get_history_v2_path())

    if not history_path.exists():
        logger.error(f"History file not found: {history_path}")
        return 1

    # Load records
    records = load_records(history_path)
    logger.info(f"Loaded {len(records)} records")

    # Backup first (unless dry run)
    if not args.dry_run:
        backup_history(history_path)

    # Process each record
    updated_count = 0
    skipped_counts: dict[str, int] = {}

    for i, record in enumerate(records):
        updated, reason = migrate_record(record)

        if updated:
            updated_count += 1
            if args.dry_run:
                path = record.get("original_path", record.get("path_hash", "?"))
                name = os.path.basename(path) if "/" in path or "\\" in path else path[:16]
                logger.info(f"  Would update: {name} - {reason}")
        else:
            skipped_counts[reason] = skipped_counts.get(reason, 0) + 1

        # Progress indicator
        if (i + 1) % 100 == 0:
            logger.info(f"  Processed {i + 1}/{len(records)}...")

    # Summary
    logger.info("")
    logger.info("=== Summary ===")
    logger.info(f"Total records: {len(records)}")
    logger.info(f"Updated: {updated_count}")
    if skipped_counts:
        logger.info("Skipped:")
        for reason, count in sorted(skipped_counts.items()):
            logger.info(f"  {reason}: {count}")

    # Save if not dry run
    if not args.dry_run and updated_count > 0:
        save_records(history_path, records)
        logger.info(f"\nSaved {updated_count} updated records")
    elif args.dry_run:
        logger.info("\n(Dry run - no changes written)")

    return 0


if __name__ == "__main__":
    sys.exit(main())
```

### Running the Migration

```bash
# Preview changes (no backup created, no writes)
python tools/migrate_audio_streams.py --dry-run

# Run migration (creates backup, then updates)
python tools/migrate_audio_streams.py
```

The script:
1. Creates a timestamped backup before any changes
2. Removes `audio_codec` from ALL records (prevents deserialization failure)
3. Adds `audio_streams` via ffprobe for files that exist
4. Sets empty `audio_streams: []` for files that can't be probed
5. Also backfills missing `bitrate_kbps`
6. Supports `--dry-run` for preview

## Testing Checklist

1. [ ] Basic scan populates audio_streams correctly
2. [ ] Conversion populates audio_streams from extracted metadata
3. [ ] History tab shows single codec correctly (e.g., "AAC")
4. [ ] History tab shows multiple codecs correctly (e.g., "AAC, AC3")
5. [ ] History tab shows count for 4+ streams (e.g., "4 audio")
6. [ ] Bitrate column shows values for converted files
7. [ ] Existing history records load after migration
8. [ ] JSON serialization/deserialization works
9. [ ] Migration script --dry-run shows expected changes
10. [ ] Migration script creates valid backup
11. [ ] Migration script updates records correctly
12. [ ] App fails gracefully if migration not run (clear error message)

## Future Enhancements

- Subtitle stream info (similar pattern)
- Video stream info (for multi-video files)
- Tooltip showing full stream details with languages/titles
- Filter by audio codec in History tab
