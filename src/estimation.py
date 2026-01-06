# src/estimation.py
"""
Time estimation functions for conversion progress.

Predictors: duration, resolution (bucketed), codec.
NOT file size - size correlates with bitrate, not encoding complexity.

See docs/TIME_ESTIMATION.md for full explanation.
"""

import logging
import os
import statistics
import time
from collections import defaultdict
from typing import Any

from src.config import MIN_SAMPLES_FOR_ESTIMATE, MIN_SAMPLES_HIGH_CONFIDENCE
from src.history_index import get_history_index
from src.models import OperationType, TimeEstimate
from src.utils import get_video_info
from src.video_metadata import extract_video_metadata

logger = logging.getLogger(__name__)


# =============================================================================
# Resolution Bucketing
# =============================================================================


def get_resolution_bucket(width: int | None, height: int | None) -> str:
    """Categorize resolution into standard buckets for rate grouping.

    Args:
        width: Video width in pixels.
        height: Video height in pixels.

    Returns:
        Resolution bucket string: "4k", "1440p", "1080p", "720p", "sd", or "unknown".
    """
    if not width or not height:
        return "unknown"
    pixels = width * height
    if pixels >= 3840 * 2160:  # 8.3M+ pixels
        return "4k"
    if pixels >= 2560 * 1440:  # 3.7M+ pixels
        return "1440p"
    if pixels >= 1920 * 1080:  # 2.1M+ pixels
        return "1080p"
    if pixels >= 1280 * 720:  # 0.9M+ pixels
        return "720p"
    return "sd"


# =============================================================================
# Encoding Rate Computation
# =============================================================================


def compute_grouped_encoding_rates(
    operation_type: OperationType | None = None,
) -> dict[tuple[str | None, str | None], list[float]]:
    """Compute encoding rates grouped by (codec, resolution_bucket).

    Encoding rate = time / duration (how long the operation takes relative to video length).

    Args:
        operation_type: If ANALYZE, use crf_search_time_sec only (much faster).
                       If CONVERT or None, use full encoding time.

    Returns:
        Dict mapping (codec, resolution_bucket) to list of rates.
        Special keys:
        - (codec, None): All resolutions for that codec (fallback)
        - (None, None): All files globally (final fallback)
    """
    index = get_history_index()
    converted_records = index.get_converted_records()

    rates: dict[tuple[str | None, str | None], list[float]] = defaultdict(list)
    for record in converted_records:
        duration = record.duration_sec or 0

        # Select appropriate time field based on operation type
        if operation_type == OperationType.ANALYZE:
            # ANALYZE only does CRF search, not full encoding
            conv_time = record.crf_search_time_sec or 0
        else:
            # CONVERT does full encoding - prefer encoding_time_sec, fall back to total_time_sec
            conv_time = record.encoding_time_sec if record.encoding_time_sec else (record.total_time_sec or 0)

        if duration > 0 and conv_time > 0:
            rate = conv_time / duration
            res_bucket = get_resolution_bucket(record.width, record.height)

            # Add to specific (codec, resolution) group
            rates[(record.video_codec, res_bucket)].append(rate)
            # Add to codec-only group for fallback
            rates[(record.video_codec, None)].append(rate)
            # Add to global for final fallback
            rates[(None, None)].append(rate)

    return rates


def compute_grouped_percentiles(operation_type: OperationType | None = None) -> dict:
    """Compute percentiles for all rate groups. Cached until converted records change.

    Results are cached in HistoryIndex and automatically invalidated when
    a CONVERTED record is added or modified.
    """
    index = get_history_index()

    # Check cache first
    cached = index.get_cached_percentiles(operation_type)
    if cached is not None:
        return cached

    # Compute and cache
    grouped_rates = compute_grouped_encoding_rates(operation_type)
    result = {key: compute_percentiles(rates) for key, rates in grouped_rates.items()}
    index.cache_percentiles(operation_type, result)
    return result


def compute_percentiles(values: list[float]) -> dict[str, float] | None:
    """Compute P25, P50, P75 percentiles from a list of values.

    Args:
        values: List of numeric values.

    Returns:
        Dict with 'p25', 'p50', 'p75', 'count' keys, or None if insufficient data.
    """
    if len(values) < MIN_SAMPLES_FOR_ESTIMATE:
        return None
    quantiles = statistics.quantiles(values, n=4)
    return {"p25": quantiles[0], "p50": quantiles[1], "p75": quantiles[2], "count": len(values)}


def estimate_file_time(
    file_path: str | None = None,
    *,
    codec: str | None = None,
    duration: float | None = None,
    width: int | None = None,
    height: int | None = None,
    operation_type: OperationType | None = None,
    grouped_percentiles: dict | None = None,
) -> TimeEstimate:
    """Estimate processing time using resolution and codec-based percentiles.

    Uses a tiered approach based on available historical data:
    1. (codec, resolution) specific rates -> high confidence (10+ samples) or medium (5-9)
    2. Codec-only rates (all resolutions) -> medium confidence
    3. Global rates (all files) -> low confidence
    4. Insufficient data -> none confidence

    Args:
        file_path: Path to file (will extract metadata via ffprobe if not provided).
        codec: Video codec (e.g., 'h264'). Extracted from file if not provided.
        duration: Video duration in seconds. Extracted from file if not provided.
        width: Video width in pixels. Extracted from file if not provided.
        height: Video height in pixels. Extracted from file if not provided.
        operation_type: If ANALYZE, estimates CRF search time only (no encoding).
                       If CONVERT or None, estimates full encoding time (CRF search + encode).
        grouped_percentiles: Pre-computed percentiles from compute_grouped_percentiles().
                            If provided, skips percentile computation (for batch operations).

    Returns:
        TimeEstimate with min/max range, best guess, and confidence level.
    """
    # Check history index first for cached metadata (avoids ffprobe subprocess)
    if file_path and (codec is None or duration is None or width is None or height is None):
        index = get_history_index()
        record = index.lookup_file(file_path)
        if record:
            if codec is None:
                codec = record.video_codec
            if duration is None:
                duration = record.duration_sec
            if width is None:
                width = record.width
            if height is None:
                height = record.height

    # Fall back to ffprobe only if still missing required data
    if file_path and (codec is None or duration is None or width is None or height is None):
        file_info = get_video_info(file_path)
        if file_info:
            meta = extract_video_metadata(file_info)
            if codec is None:
                codec = meta.video_codec
            if duration is None:
                duration = meta.duration_sec or 0
            if width is None:
                width = meta.width
            if height is None:
                height = meta.height

    # Validate we have minimum required data
    if not duration or duration <= 0:
        return TimeEstimate(0, 0, 0, "none", "no_duration")

    # Use pre-computed percentiles if provided, otherwise compute
    if grouped_percentiles is None:
        grouped_percentiles = compute_grouped_percentiles(operation_type)
    res_bucket = get_resolution_bucket(width, height)

    # Tier 1: (codec, resolution) specific
    if codec and res_bucket != "unknown":
        key = (codec, res_bucket)
        stats = grouped_percentiles.get(key)
        if stats:
            confidence = "high" if stats.get("count", 0) >= MIN_SAMPLES_HIGH_CONFIDENCE else "medium"
            return TimeEstimate(
                min_seconds=duration * stats["p25"],
                max_seconds=duration * stats["p75"],
                best_seconds=duration * stats["p50"],
                confidence=confidence,
                source=f"{codec}:{res_bucket}",
            )

    # Tier 2: Codec-only (all resolutions)
    if codec:
        key = (codec, None)
        stats = grouped_percentiles.get(key)
        if stats:
            return TimeEstimate(
                min_seconds=duration * stats["p25"],
                max_seconds=duration * stats["p75"],
                best_seconds=duration * stats["p50"],
                confidence="medium",
                source=f"codec:{codec}",
            )

    # Tier 3: Global
    stats = grouped_percentiles.get((None, None))
    if stats:
        return TimeEstimate(
            min_seconds=duration * stats["p25"],
            max_seconds=duration * stats["p75"],
            best_seconds=duration * stats["p50"],
            confidence="low",
            source="global",
        )

    # Tier 4: Insufficient data
    return TimeEstimate(0, 0, 0, "none", "insufficient_data")


def estimate_current_file_eta(
    running: bool,
    last_eta_seconds: float | None,
    last_eta_timestamp: float | None,
    encoding_progress: float,
    encoding_start_time: float | None,
) -> float:
    """Estimate time remaining for the current file being processed.

    Args:
        running: Whether conversion is currently running
        last_eta_seconds: Last stored ETA from ab-av1 in seconds
        last_eta_timestamp: Timestamp when last_eta_seconds was updated
        encoding_progress: Current encoding progress (0-100)
        encoding_start_time: Timestamp when encoding phase started

    Returns:
        Estimated seconds remaining for current file, or 0 if not processing
    """
    if not running:
        return 0

    # Check for stored AB-AV1 ETA first (most accurate)
    if last_eta_seconds is not None and last_eta_timestamp is not None:
        elapsed_since_update = time.time() - last_eta_timestamp
        return max(0, last_eta_seconds - elapsed_since_update)

    # Fallback to calculation based on progress
    if encoding_progress > 0 and encoding_start_time is not None:
        elapsed_encoding_time = time.time() - encoding_start_time
        if elapsed_encoding_time > 1:
            total_encoding_time_est = (elapsed_encoding_time / encoding_progress) * 100
            current_eta = total_encoding_time_est - elapsed_encoding_time
            logger.debug(f"Using progress-based ETA: {current_eta}s for current file")
            return max(0, current_eta)

    return 0


def estimate_pending_files_eta(
    pending_files: list[str],
    current_file_path: str | None,
    current_file_encoding_started: bool,
    operation_type: OperationType | None = None,
    grouped_percentiles: dict | None = None,
) -> float:
    """Estimate total time needed for all pending files.

    Args:
        pending_files: List of file paths waiting to be processed
        current_file_path: Path of file currently being processed (if any)
        current_file_encoding_started: Whether current file has started encoding phase
        operation_type: Operation type for time estimation (ANALYZE vs CONVERT).
                       If None, defaults to CONVERT estimates.
        grouped_percentiles: Pre-computed percentiles from compute_grouped_percentiles().
                            If provided, skips percentile computation (for batch operations).

    Returns:
        Estimated total seconds for all pending files
    """
    if not pending_files:
        return 0

    # Pre-compute percentiles once for all files if not provided
    if grouped_percentiles is None:
        grouped_percentiles = compute_grouped_percentiles(operation_type)

    total_time = 0

    # Normalize current path for comparison
    normalized_current_path = os.path.normpath(current_file_path) if current_file_path else None

    for file_path in pending_files:
        # Normalize path for comparison
        normalized_file_path = os.path.normpath(file_path)

        # Skip if this is the current file and it's already being encoded
        if normalized_file_path == normalized_current_path and current_file_encoding_started:
            continue

        # Estimate time for this file
        file_estimate = estimate_file_time(
            file_path, operation_type=operation_type, grouped_percentiles=grouped_percentiles
        ).best_seconds
        total_time += file_estimate

    return total_time


def estimate_remaining_time(gui: Any) -> float:
    """Estimate total remaining time for all queued files.

    This is a GUI-aware wrapper that extracts session state and delegates
    to the decoupled estimation functions.

    Args:
        gui: The main GUI instance

    Returns:
        Estimated remaining time in seconds
    """
    if not gui.session.running:
        return 0

    remaining_time = 0

    # Add ETA for current file if it's being processed
    current_file_eta = estimate_current_file_eta(
        running=gui.session.running,
        last_eta_seconds=gui.session.last_eta_seconds,
        last_eta_timestamp=gui.session.last_eta_timestamp,
        encoding_progress=gui.session.last_encoding_progress,
        encoding_start_time=gui.session.current_file_encoding_start_time,
    )
    if current_file_eta > 0:
        remaining_time += current_file_eta

    # Add ETA for pending files
    pending_eta = estimate_pending_files_eta(
        pending_files=gui.session.pending_files,
        current_file_path=gui.session.current_file_path,
        current_file_encoding_started=gui.session.current_file_encoding_start_time is not None,
    )
    remaining_time += pending_eta

    return remaining_time
