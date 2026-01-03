# ab-av1 Output Parsing

This document explains how the application parses output from the `ab-av1` tool to extract progress and statistics.

## Overview

ab-av1 conversion has two distinct phases with different output formats:

1. **CRF Search** (quality detection) - ab-av1's structured output
2. **Encoding** - FFmpeg's progress output passed through ab-av1

The parser (`src/ab_av1/parser.py`) handles both phases with different regex patterns.

## Phase 1: CRF Search

During CRF search, ab-av1 samples the video at various CRF values to find one meeting the VMAF target.

### Output Format

```
crf 30 VMAF 96.5
crf 32 VMAF 94.2
crf 31 VMAF 95.1
Best CRF: 31
predicted video stream size 450MB (65%)
```

### Patterns

| Pattern | Purpose | Example Match |
|---------|---------|---------------|
| `crf\s+(\d+)\s+VMAF\s+(\d+\.?\d*)` | Extract CRF/VMAF pairs | `crf 31 VMAF 95.1` |
| `Best\s+CRF:\s+(\d+)` | Final CRF selection | `Best CRF: 31` |
| `predicted video stream size.*?\((\d+\.?\d*)\s*%\)` | Size prediction | `(65%)` |

### Progress Heuristic

CRF search progress is estimated since ab-av1 doesn't report it directly:
- Each CRF/VMAF pair increments progress by 10% (capped at 90%)
- "Best CRF" line sets progress to 95%
- Phase transition to encoding sets quality progress to 100%

## Phase Transition

The parser detects encoding start via:

```regex
ab_av1::command::encode\]\s*encoding|Starting encoding
```

When matched:
- `stats["phase"]` changes from `"crf-search"` to `"encoding"`
- Quality progress set to 100%
- Encoding progress reset to 0%

## Phase 2: Encoding

During encoding, FFmpeg writes progress to stderr. ab-av1 passes this through, sometimes with its own summary lines.

### FFmpeg Progress Format

```
frame=12345 fps=45.2 q=-1.0 size=524288kB time=00:15:30.50 bitrate=4500.0kbits/s speed=1.85x
```

### Patterns (Priority Order)

1. **ab-av1 structured progress** (most reliable when present):
   ```
   [sample_encode] 45.2%, 30 fps, eta 5m 30s
   command::encode] 45%, 30 fps, eta 5m 30s
   ```

2. **FFmpeg time-based** (most common, needs duration):
   ```regex
   time=\s*(\d+):(\d+):(\d+\.\d+)
   ```
   Progress calculated as: `(parsed_seconds / total_duration) * 100`

3. **Generic percentage** (fallback):
   ```regex
   (\d+)\s*%
   ```

4. **ab-av1 summary line** (ETA only):
   ```
   ⠖ 00:00:37 Encoding -------- (encoding, eta 0s)
   ```

### FFmpeg Field Extraction

| Field | Pattern | Example |
|-------|---------|---------|
| Frame | `frame=\s*(\d+)` | `frame=12345` |
| FPS | `fps=\s*(\d+\.?\d*)` | `fps=45.2` |
| Time | `time=\s*(\d+):(\d+):(\d+\.\d+)` | `time=00:15:30.50` |
| Speed | `speed=\s*(\d+\.?\d*)x` | `speed=1.85x` |
| Size | `size=\s*(\d+)([kKmMgG]?[bB])` | `size=524288kB` |

## Progress Update Throttling

To avoid UI spam, encoding progress updates are throttled:
- Only sent when progress increases by at least 0.1%
- ETA updates only sent when ETA text actually changes

## Error Detection

The parser watches for error indicators:

```regex
error|failed|invalid
Failed\s+to\s+find\s+a\s+suitable\s+crf
```

Error handling (like VMAF fallback) is done in the wrapper, not the parser.

## Stats Dictionary

The parser maintains and updates a stats dictionary:

| Key | Type | Description |
|-----|------|-------------|
| `phase` | str | `"crf-search"` or `"encoding"` |
| `progress_quality` | float | 0-100, CRF search progress |
| `progress_encoding` | float | 0-100, encoding progress |
| `crf` | int | Current/best CRF value |
| `vmaf` | float | Current/best VMAF score |
| `size_reduction` | float | Predicted size reduction % |
| `eta_text` | str | ETA string from ab-av1 |
| `total_duration_seconds` | float | Video duration (for progress calc) |
| `last_ffmpeg_fps` | float | Last parsed FPS value |

## Callback Flow

```
ab-av1 stdout/stderr
    ↓
AbAv1Parser.parse_line()
    ↓
ProgressEvent dataclass
    ↓
file_info_callback(basename, "progress", event)
    ↓
GUI update via update_ui_safely()
```

## Buffering Issues

FFmpeg's progress output is subject to buffering, which can cause:
- Delayed progress updates
- Multiple progress lines arriving at once
- Gaps in progress reporting

The environment variables in `wrapper.py` attempt to minimize buffering, but it's not fully controllable.
