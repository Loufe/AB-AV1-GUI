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
import time
from typing import Any

from src.utils import get_video_info, load_history

logger = logging.getLogger(__name__)


def find_similar_file_in_history(current_file_info: dict, tolerance: dict | None = None) -> dict | None:
    """Find a similar file in conversion history based on codec, duration, and size.

    Args:
        current_file_info: Dict with 'codec', 'duration', 'size' keys
        tolerance: Dict with tolerance values. Defaults to {'duration': 0.2, 'size': 0.3}
    Returns:
        Dict with historical processing data or None if no match found
    """
    if tolerance is None:
        tolerance = {"duration": 0.2, "size": 0.3}  # 20% duration, 30% size tolerance

    history = load_history()
    if not history:
        return None

    best_match = None
    best_score = float("inf")

    current_codec = current_file_info.get("codec")
    current_duration = current_file_info.get("duration", 0)
    current_size = current_file_info.get("size", 0)

    for record in history:
        # Check if same codec
        hist_codec = record.get("input_codec") or record.get("input_vcodec")
        if hist_codec != current_codec:
            continue

        # Get metrics for comparison
        hist_duration = record.get("duration_sec", 0)
        hist_size = record.get("input_size_mb", 0) * (1024**2)  # Convert to bytes
        hist_time = record.get("time_sec", 0)

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


def estimate_processing_speed_from_history() -> float:
    """Calculate average processing speed (bytes/second) from historical data.

    Returns:
        Average processing speed in bytes/second or 0 if no history
    """
    history = load_history()
    if not history:
        return 0

    speeds = []
    for record in history:
        input_size = record.get("input_size_mb", 0) * (1024**2)  # Convert to bytes
        time_sec = record.get("time_sec", 0)
        if input_size > 0 and time_sec > 0:
            speeds.append(input_size / time_sec)

    return sum(speeds) / len(speeds) if speeds else 0


def estimate_current_file_eta(gui: Any) -> float:
    """Estimate time remaining for the current file being processed.

    Args:
        gui: The main GUI instance with conversion state

    Returns:
        Estimated seconds remaining for current file, or 0 if not processing
    """
    if not getattr(gui, "conversion_running", False):
        return 0

    # Check for stored AB-AV1 ETA first (most accurate)
    if hasattr(gui, "last_eta_seconds") and hasattr(gui, "last_eta_timestamp"):
        elapsed_since_update = time.time() - gui.last_eta_timestamp
        return max(0, gui.last_eta_seconds - elapsed_since_update)

    # Fallback to calculation based on progress
    encoding_prog = getattr(gui, "last_encoding_progress", 0)
    if encoding_prog > 0 and hasattr(gui, "current_file_encoding_start_time") and gui.current_file_encoding_start_time:
        elapsed_encoding_time = time.time() - gui.current_file_encoding_start_time
        if elapsed_encoding_time > 1:
            total_encoding_time_est = (elapsed_encoding_time / encoding_prog) * 100
            current_eta = total_encoding_time_est - elapsed_encoding_time
            logger.debug(f"Using progress-based ETA: {current_eta}s for current file")
            return max(0, current_eta)

    return 0


def get_file_processing_estimate(file_path: str, history: list | None = None) -> float:
    """Estimate processing time for a single file based on historical data.

    Args:
        file_path: Path to the file to estimate
        history: Optional pre-loaded history list (for efficiency)

    Returns:
        Estimated processing time in seconds, or 0 if can't estimate
    """
    if history is None:
        history = load_history()

    # Get file info
    file_info = get_video_info(file_path)
    if not file_info:
        return 0

    # Extract codec, duration, and size
    file_codec = None
    file_duration = 0
    file_size = file_info.get("file_size", 0)

    for stream in file_info.get("streams", []):
        if stream.get("codec_type") == "video":
            file_codec = stream.get("codec_name")
            break

    if "format" in file_info and "duration" in file_info["format"]:
        try:
            file_duration = float(file_info["format"]["duration"])
        except (ValueError, KeyError, TypeError):
            file_duration = 0

    # First, try to find a similar file in history
    similar_file = find_similar_file_in_history({"codec": file_codec, "duration": file_duration, "size": file_size})

    if similar_file:
        return similar_file.get("time_sec", 0)

    # Use average processing speed as fallback
    avg_speed = estimate_processing_speed_from_history()
    if avg_speed > 0 and file_size > 0:
        return file_size / avg_speed

    # If no historical data, use rough estimate of 1 GB per hour
    if file_size > 0:
        rough_speed = (1024**3) / 3600  # 1 GB per hour
        return file_size / rough_speed

    # Default fallback - assume 30 minutes if no size available
    return 1800


def estimate_pending_files_eta(gui: Any, pending_files: list) -> float:
    """Estimate total time needed for all pending files.

    Args:
        gui: The main GUI instance
        pending_files: List of file paths waiting to be processed

    Returns:
        Estimated total seconds for all pending files
    """
    if not pending_files:
        return 0

    total_time = 0
    current_path = getattr(gui, "current_file_path", None)

    # Normalize current path for comparison
    if current_path:
        current_path = os.path.normpath(current_path)

    # Check if current file is already being encoded (don't count it twice)
    current_file_handled = hasattr(gui, "current_file_encoding_start_time") and gui.current_file_encoding_start_time

    for file_path in pending_files:
        # Normalize path for comparison
        normalized_file_path = os.path.normpath(file_path)

        # Skip if this is the current file and it's already being encoded
        if normalized_file_path == current_path and current_file_handled:
            continue

        # Estimate time for this file
        file_estimate = get_file_processing_estimate(file_path)
        total_time += file_estimate

    return total_time


def estimate_remaining_time(gui: Any, current_file_info: dict | None = None) -> float:
    """Estimate total remaining time for all queued files.

    Args:
        gui: The main GUI instance
        current_file_info: Dict with current file info if available (optional, currently unused)
    Returns:
        Estimated remaining time in seconds
    """
    if not getattr(gui, "conversion_running", False):
        return 0

    remaining_time = 0

    # Add ETA for current file if it's being processed
    current_file_eta = estimate_current_file_eta(gui)
    if current_file_eta > 0:
        remaining_time += current_file_eta

    # Add ETA for pending files
    pending_files = getattr(gui, "pending_files", [])
    pending_eta = estimate_pending_files_eta(gui, pending_files)
    remaining_time += pending_eta

    return remaining_time
