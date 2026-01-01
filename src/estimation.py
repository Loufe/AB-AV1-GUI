# src/estimation.py
"""
Time estimation functions for conversion progress.

Provides functions to estimate remaining time based on:
- Historical conversion data
- Current file progress
- Pending files in queue
"""

import logging
import os
import statistics
import time
from typing import Any

from src.config import MIN_SAMPLES_FOR_ESTIMATE
from src.history_index import get_history_index
from src.models import FileRecord, TimeEstimate
from src.utils import get_video_info

logger = logging.getLogger(__name__)


def find_similar_file_in_history(current_file_info: dict, tolerance: dict | None = None) -> FileRecord | None:
    """Find a similar file in conversion history based on codec, duration, and size.

    Args:
        current_file_info: Dict with 'codec', 'duration', 'size' keys
        tolerance: Dict with tolerance values. Defaults to {'duration': 0.2, 'size': 0.3}
    Returns:
        FileRecord with historical processing data or None if no match found
    """
    if tolerance is None:
        tolerance = {"duration": 0.2, "size": 0.3}  # 20% duration, 30% size tolerance

    index = get_history_index()
    converted_records = index.get_converted_records()
    if not converted_records:
        return None

    best_match = None
    best_score = float("inf")

    current_codec = current_file_info.get("codec")
    current_duration = current_file_info.get("duration", 0)
    current_size = current_file_info.get("size", 0)

    for record in converted_records:
        # Check if same codec
        if record.video_codec != current_codec:
            continue

        # Get metrics for comparison
        hist_duration = record.duration_sec or 0
        hist_size = record.file_size_bytes
        hist_time = record.conversion_time_sec or 0

        if not (hist_duration and hist_size and hist_time):
            continue

        # Check if within tolerance
        if current_duration > 0 and hist_duration > 0:
            duration_diff = abs(hist_duration - current_duration) / hist_duration
        else:
            duration_diff = 1
        size_diff = abs(hist_size - current_size) / hist_size if current_size > 0 and hist_size > 0 else 1

        if duration_diff <= tolerance["duration"] and size_diff <= tolerance["size"]:
            # Score based on similarity (lower is better)
            score = duration_diff + size_diff
            if score < best_score:
                best_score = score
                best_match = record

    return best_match


def get_encoding_rates(codec: str | None = None) -> list[float]:
    """Get historical encoding rates (conversion_time / duration), optionally filtered by codec.

    Encoding rate represents how long encoding takes relative to video duration.
    A rate of 1.5 means encoding takes 1.5x the video's duration.

    Args:
        codec: If provided, only return rates for files with this video codec.

    Returns:
        List of encoding rates from historical conversions.
    """
    index = get_history_index()
    converted_records = index.get_converted_records()

    rates = []
    for record in converted_records:
        duration = record.duration_sec or 0
        conv_time = record.conversion_time_sec or 0
        if duration > 0 and conv_time > 0 and (codec is None or record.video_codec == codec):
            rates.append(conv_time / duration)

    return rates


def compute_percentiles(values: list[float]) -> dict[str, float] | None:
    """Compute P25, P50, P75 percentiles from a list of values.

    Args:
        values: List of numeric values.

    Returns:
        Dict with 'p25', 'p50', 'p75' keys, or None if insufficient data.
    """
    if len(values) < MIN_SAMPLES_FOR_ESTIMATE:
        return None
    quantiles = statistics.quantiles(values, n=4)
    return {"p25": quantiles[0], "p50": quantiles[1], "p75": quantiles[2]}


def estimate_file_time(
    file_path: str | None = None, *, codec: str | None = None, duration: float | None = None, size: int | None = None
) -> TimeEstimate:
    """Estimate processing time with tiered confidence levels.

    Uses a tiered approach:
    1. Similar file match (same codec, ~duration, ~size) → medium confidence
    2. Codec-specific percentiles → low confidence
    3. Global percentiles → low confidence
    4. Insufficient data → none confidence

    Args:
        file_path: Path to file (will extract codec/duration/size if not provided).
        codec: Video codec (e.g., 'h264'). Extracted from file if not provided.
        duration: Video duration in seconds. Extracted from file if not provided.
        size: File size in bytes. Extracted from file if not provided.

    Returns:
        TimeEstimate with min/max range, best guess, and confidence level.
    """
    # Extract file info if needed
    if file_path and (codec is None or duration is None or size is None):
        file_info = get_video_info(file_path)
        if file_info:
            if codec is None:
                for stream in file_info.get("streams", []):
                    if stream.get("codec_type") == "video":
                        codec = stream.get("codec_name")
                        break
            if duration is None:
                try:
                    duration = float(file_info.get("format", {}).get("duration", 0))
                except (ValueError, TypeError):
                    duration = 0
            if size is None:
                size = file_info.get("file_size", 0)

    # Validate we have minimum required data
    if not duration or duration <= 0:
        return TimeEstimate(0, 0, 0, "none", "no_duration")

    # Tier 1: Try to find a similar file (same codec, similar duration/size)
    if codec and size:
        similar = find_similar_file_in_history({"codec": codec, "duration": duration, "size": size})
        if similar and similar.conversion_time_sec:
            t = similar.conversion_time_sec
            # ±20% range for similar file match
            return TimeEstimate(
                min_seconds=t * 0.8, max_seconds=t * 1.2, best_seconds=t, confidence="medium", source="similar_file"
            )

    # Tier 2: Codec-specific percentiles
    if codec:
        codec_rates = get_encoding_rates(codec)
        stats = compute_percentiles(codec_rates)
        if stats:
            return TimeEstimate(
                min_seconds=duration * stats["p25"],
                max_seconds=duration * stats["p75"],
                best_seconds=duration * stats["p50"],
                confidence="low",
                source=f"codec:{codec}",
            )

    # Tier 3: Global percentiles
    all_rates = get_encoding_rates()
    stats = compute_percentiles(all_rates)
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


def estimate_from_progress(progress_percent: float, elapsed_seconds: float) -> TimeEstimate:
    """Estimate remaining time from in-progress encoding.

    This is the most accurate estimation method, used during active encoding.

    Args:
        progress_percent: Current encoding progress (0-100).
        elapsed_seconds: Time elapsed since encoding started.

    Returns:
        TimeEstimate with high confidence single value.
    """
    if progress_percent <= 0 or elapsed_seconds <= 0:
        return TimeEstimate(0, 0, 0, "none", "in_progress")

    total_estimated = (elapsed_seconds / progress_percent) * 100
    remaining = max(0, total_estimated - elapsed_seconds)

    return TimeEstimate(
        min_seconds=remaining, max_seconds=remaining, best_seconds=remaining, confidence="high", source="in_progress"
    )


def estimate_processing_speed_from_history() -> float:
    """Calculate median processing speed (bytes/second) from historical data.

    Returns:
        Median processing speed in bytes/second or 0 if no history.
    """
    index = get_history_index()
    converted_records = index.get_converted_records()
    if not converted_records:
        return 0

    speeds = []
    for record in converted_records:
        input_size = record.file_size_bytes
        time_sec = record.conversion_time_sec or 0
        if input_size > 0 and time_sec > 0:
            speeds.append(input_size / time_sec)

    return statistics.median(speeds) if speeds else 0


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
    pending_files: list[str], current_file_path: str | None, current_file_encoding_started: bool
) -> float:
    """Estimate total time needed for all pending files.

    Args:
        pending_files: List of file paths waiting to be processed
        current_file_path: Path of file currently being processed (if any)
        current_file_encoding_started: Whether current file has started encoding phase

    Returns:
        Estimated total seconds for all pending files
    """
    if not pending_files:
        return 0

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
        file_estimate = estimate_file_time(file_path).best_seconds
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
