# Time Estimation System

This document explains how the application predicts encoding time for video files.

## Overview

The estimation system uses historical conversion data to predict how long future encodes will take. When you convert files, the app records how long each took. These recordings are then used to estimate times for similar files.

## Predictors

The system uses three predictors, in order of importance:

### 1. Duration

The strongest predictor. Encoding time scales linearly with video length - a 2-hour video takes roughly twice as long as a 1-hour video at the same resolution.

### 2. Resolution

More pixels means more computation per frame. Resolution is bucketed into standard categories:

| Bucket | Minimum Pixels | Typical Resolution |
|--------|----------------|-------------------|
| 4k     | 8,294,400      | 3840x2160         |
| 1440p  | 3,686,400      | 2560x1440         |
| 1080p  | 2,073,600      | 1920x1080         |
| 720p   | 921,600        | 1280x720          |
| sd     | Below 720p     | 640x480, etc.     |

Bucketing allows grouping similar resolutions together (e.g., 1920x1080 and 1920x800 both fall in "1080p"), providing more historical samples per group.

### 3. Codec

Source codec affects decode speed. H.264 and HEVC decode at different rates, which slightly impacts total encoding time.

## Why File Size Is NOT a Predictor

File size was intentionally excluded. Here's why:

File size correlates with **source bitrate**, not encoding complexity. Consider two 1080p videos:
- A highly compressed streaming rip: 1 GB
- A Blu-ray remux: 30 GB

Both have the same resolution and similar frame counts. The encoder processes roughly the same number of pixels regardless of source bitrate. The 30 GB file doesn't take 30x longer to encode - it takes about the same time as the 1 GB file.

Using file size as a predictor would systematically overestimate times for high-bitrate sources and underestimate for low-bitrate sources.

## How Estimation Works

### Encoding Rate

The fundamental unit is the **encoding rate**:

```
rate = encoding_time / duration
```

A rate of 2.0 means encoding takes twice as long as playback. A 1-hour video with rate 2.0 takes 2 hours to encode.

### Grouping

Rates are grouped by `(codec, resolution_bucket)`. For example:
- `(hevc, 1080p)` - all HEVC 1080p files
- `(h264, 4k)` - all H.264 4K files

### Percentiles

For each group, the system computes percentiles from historical rates:
- **P25**: Optimistic estimate (faster than 75% of similar files)
- **P50**: Best guess (median)
- **P75**: Conservative estimate (slower than 75% of similar files)

The displayed estimate uses P50. The min/max range uses P25/P75.

### Tiered Fallback

If there isn't enough data for a specific group, the system falls back:

| Tier | Lookup Key | Min Samples | Confidence |
|------|------------|-------------|------------|
| 1    | (codec, resolution) | 10+ | high |
| 1    | (codec, resolution) | 5-9 | medium |
| 2    | (codec, any) | 5+ | medium |
| 3    | (any, any) | 5+ | low |
| 4    | insufficient data | <5 | none |

Example: Estimating a 4K HEVC file with only 3 similar files in history:
1. Try `(hevc, 4k)` - only 3 samples, insufficient
2. Fall back to `(hevc, None)` - use all HEVC files regardless of resolution
3. If still insufficient, fall back to global rates

### Confidence Display

Estimates show confidence via formatting:
- **High confidence**: No prefix (e.g., "2h 30m")
- **Medium confidence**: Single tilde (e.g., "~2h 30m")
- **Low confidence**: Double tilde (e.g., "~~2h 30m")
- **None**: Displayed as "—"

## Operation Types

The system estimates differently based on operation:

- **CONVERT**: Uses `encoding_time_sec` (full encode)
- **ANALYZE**: Uses `crf_search_time_sec` (CRF search only, much faster)

This ensures the preview dialog shows accurate estimates for whichever operation you're queueing.

## Implementation

Key functions in `src/estimation.py`:

- `get_resolution_bucket(width, height)` - Categorizes resolution
- `compute_grouped_encoding_rates(operation_type)` - Builds rate dictionary
- `compute_percentiles(values)` - Calculates P25/P50/P75
- `estimate_file_time(...)` - Main entry point, returns `TimeEstimate`

The `TimeEstimate` dataclass contains:
- `min_seconds`, `max_seconds`, `best_seconds` - The estimate range
- `confidence` - "high", "medium", "low", or "none"
- `source` - What data was used (e.g., "hevc:1080p", "codec:hevc", "global")
