---
status: accepted
date: 2026-01-06
---

# Use Metadata-Only Approach for Duplicate File Detection

## Context and Problem Statement

The application needs to detect when the same physical video file is accessed via different filesystem paths (e.g., `B:\video.mp4` vs `\\NAS\video.mp4`, or mapped network drives vs UNC paths). Currently, each path creates a separate history record, causing a file marked NOT_WORTHWHILE via one path to show as SCANNED with conversion estimates via another.

## Decision Drivers

* Network paths may be inaccessible at scan time (offline NAS, unmapped drives)
* `os.path.samefile()` can timeout on network paths, blocking the UI
* Detection must work for anonymized records where `original_path` is null
* Industry tools (Plex, Jellyfin) successfully use metadata-only approaches

## Considered Options

* **Option A: `os.path.samefile()` with heuristic fallback** - Use OS-level filesystem check as primary method, fall back to metadata when paths inaccessible
* **Option B: Metadata-only comparison** - Use size + duration + filename matching without any filesystem calls

## Decision Outcome

Chosen option: **Option B (Metadata-only)**, because:

1. **Reliability**: No risk of timeout on network paths - the application remains responsive regardless of network state
2. **Simplicity**: Single code path instead of branching between filesystem and heuristic methods
3. **Industry validation**: Production tools (Plex Dupefinder, Jellyfin Duplicate Finder) use metadata comparison successfully:
   - Plex Dupefinder: bitrate, duration, resolution, size, codecs ([source](https://laster13.github.io/ssdv2_docs/en/Applications/Plex-Dupefinder/))
   - Jellyfin: IMDB ID + metadata ([source](https://github.com/MadFra/Jellyfin-Duplicate-Finder))
4. **Works with anonymization**: Filename hash can be stored even when `original_path` is null

### Consequences

* Good: No UI freezes from network timeouts
* Good: Consistent behavior regardless of path accessibility
* Good: Works with privacy/anonymization enabled
* Bad: Theoretical false positives if two different files have identical size + duration + filename (extremely unlikely for video files)

## More Information

The detection uses a 3-step cascade:
1. Size + duration + filename literal (when `original_path` available)
2. Size + duration + filename_hash (for anonymized records)
3. Size + duration uniqueness (fallback for legacy records without hash)

Duration tolerance of 0.1 seconds accounts for rounding differences between code paths (folder_analysis stores raw ffprobe values, worker.py rounds to 1 decimal).

See ADR-001 implementation in `src/history_index.py:find_better_duplicate()`.
