# src/conversion_engine/scanner.py
"""
Contains functions to scan video files and determine if conversion is needed.
"""

import logging
import os
from pathlib import Path
from typing import Any

from src.config import MIN_RESOLUTION_HEIGHT, MIN_RESOLUTION_WIDTH  # Import resolution constants

# Project imports
from src.utils import anonymize_filename, get_video_info  # No GUI needed here

logger = logging.getLogger(__name__)


def find_video_files(folder_path: str, extensions: list[str]) -> list[str]:
    """Find all video files in a folder matching the given extensions.

    Uses single-pass os.walk() with case-insensitive extension matching.

    Args:
        folder_path: Path to folder to scan
        extensions: List of file extensions to match (e.g., ["mp4", "mkv"])

    Returns:
        Sorted list of absolute file paths
    """
    # Build set of lowercase extensions for fast lookup
    ext_set = {ext.lower() for ext in extensions}

    files = []
    for dirpath, _dirnames, filenames in os.walk(folder_path):
        for filename in filenames:
            # Case-insensitive extension check
            ext = filename.rsplit(".", 1)[-1].lower() if "." in filename else ""
            if ext in ext_set:
                files.append(os.path.join(dirpath, filename))

    return sorted(files)


def scan_video_needs_conversion(
    input_video_path: str, output_path: str, overwrite: bool = False, video_info_cache: dict[str, Any] | None = None
) -> tuple[bool, str, dict[str, Any] | None]:
    """Scan a video file to determine if it needs conversion, using a cache.

    Args:
        input_video_path: Absolute path to the video file to scan.
        output_path: Absolute path to the output file (pre-calculated by caller).
        overwrite: Whether to overwrite existing output files.
        video_info_cache: Dictionary to use for caching ffprobe results.

    Returns:
        Tuple of (needs_conversion, reason, video_info) where:
        - needs_conversion is a boolean.
        - reason is a string explaining why conversion is needed or not needed.
        - video_info is the dictionary obtained from get_video_info (or cache), or None if failed.
    """
    anonymized_input = anonymize_filename(input_video_path)
    anonymized_output = anonymize_filename(output_path)
    video_info = None  # Initialize video_info

    # Convert output_path to Path object for existence checks
    input_path_obj = Path(input_video_path).resolve()
    output_path_obj = Path(output_path).resolve()

    # --- Check Output Existence ---
    # Only skip here if the output exists AND it's NOT an in-place conversion (which needs codec check)
    if output_path_obj.exists() and not overwrite and input_path_obj != output_path_obj:
        reason = "Output file exists"
        logger.info(f"Skipping {anonymized_input} - {reason}: {anonymized_output}")
        return False, reason, None

    # --- Video Info Caching & Retrieval ---
    cache_was_hit = False
    if video_info_cache is not None and input_video_path in video_info_cache:
        video_info = video_info_cache[input_video_path]
        cache_was_hit = True
        logger.debug(f"Cache hit for {anonymized_input}")
    else:
        try:
            video_info = get_video_info(input_video_path)
            if video_info and video_info_cache is not None:
                video_info_cache[input_video_path] = video_info  # Store only on success
                logger.debug(f"Cache miss, stored info for {anonymized_input}")
            elif not video_info:
                logger.warning(f"get_video_info failed for {anonymized_input} (cache hit: {cache_was_hit})")
            # else: video_info is now populated or None if failed
        except Exception as e:
            logger.error(f"Error getting video info for {anonymized_input}: {e}", exc_info=True)
            video_info = None  # Ensure video_info is None on error

    # --- Analyze Video Info (if retrieved successfully) ---
    if not video_info:
        # If analysis failed (either initially or during cache retrieval)
        reason = "Analysis failed"
        logger.warning(f"Cannot analyze {anonymized_input} - will attempt conversion.")
        return True, reason, None  # Assume conversion needed if analysis fails

    try:
        is_already_av1 = False
        video_stream_found = False
        width = 0
        height = 0
        # Check container from path, not ffprobe (ffprobe 'format_name' can be just 'matroska')
        is_mkv_container = input_video_path.lower().endswith(".mkv")

        for stream in video_info.get("streams", []):
            if stream.get("codec_type") == "video":
                video_stream_found = True
                codec_name = stream.get("codec_name", "").lower()
                if codec_name == "av1":
                    is_already_av1 = True
                # Get resolution from video stream
                width = stream.get("width", 0)
                height = stream.get("height", 0)
                # We get the first video stream and break to avoid multiple streams
                break

        if not video_stream_found:
            reason = "No video stream found"
            logger.warning(f"No video stream in {anonymized_input} - skipping.")
            return False, reason, video_info  # Return info even if skipped

        # Check resolution before other checks
        if width < MIN_RESOLUTION_WIDTH or height < MIN_RESOLUTION_HEIGHT:
            reason = (
                f"Below minimum resolution ({width}x{height}) - needs at least "
                f"{MIN_RESOLUTION_WIDTH}x{MIN_RESOLUTION_HEIGHT}"
            )
            logger.info(f"Skipping {anonymized_input} - {reason}")
            return False, reason, video_info

        # Decision Logic:
        # Skip if: Already AV1 AND already in MKV container (regardless of overwrite or in-place)
        # Convert if: Not AV1 OR (AV1 but NOT in MKV container).
        # The suffix logic in process_video handles the specific case of
        # non-AV1 MKV input + no overwrite + output=input folder.
        if is_already_av1 and is_mkv_container:
            reason = "Already AV1/MKV"
            logger.info(f"Skipping {anonymized_input} - {reason}")
            return False, reason, video_info  # Return info even if skipped
        # Determine specific reason for conversion
        if not is_already_av1:
            reason = "Needs conversion (codec is not AV1)"
        elif not is_mkv_container:
            reason = "Needs conversion (AV1 but not MKV container)"
        else:
            reason = "Needs conversion"  # Fallback reason

        logger.info(f"Conversion needed for {anonymized_input} - Reason: {reason}")
        return True, reason, video_info

    except Exception:
        logger.exception(f"Error checking video stream info for {anonymized_input}")
        # If an unexpected error occurs during stream check, assume conversion might be needed
        return True, "Error during stream check", video_info
