# src/conversion_engine/scanner.py
"""
Contains the function to scan video files and determine if conversion is needed.
"""
import os
import logging
from pathlib import Path

# Project imports
from src.utils import get_video_info, anonymize_filename # No GUI needed here
from src.config import MIN_RESOLUTION_WIDTH, MIN_RESOLUTION_HEIGHT  # Import resolution constants

logger = logging.getLogger(__name__)

def scan_video_needs_conversion(input_video_path: str, input_base_folder: str, output_base_folder: str, overwrite: bool = False, video_info_cache: dict = None) -> tuple:
    """Scan a video file to determine if it needs conversion, using a cache.

    Args:
        input_video_path: Absolute path to the video file to scan.
        input_base_folder: Absolute path to the base input folder selected by the user.
        output_base_folder: Absolute path to the base output folder selected by the user.
        overwrite: Whether to overwrite existing output files.
        video_info_cache: Dictionary to use for caching ffprobe results.

    Returns:
        Tuple of (needs_conversion, reason, video_info) where:
        - needs_conversion is a boolean.
        - reason is a string explaining why conversion is needed or not needed.
        - video_info is the dictionary obtained from get_video_info (or cache), or None if failed.
    """
    anonymized_input = anonymize_filename(input_video_path)
    video_info = None # Initialize video_info

    try:
        input_path_obj = Path(input_video_path).resolve()
        input_folder_obj = Path(input_base_folder).resolve()
        output_folder_obj = Path(output_base_folder).resolve()

        # Determine output path relative to base folders
        relative_dir = Path(".") # Default if not relative
        try:
            # Check if input path is within the input base folder
            relative_dir = input_path_obj.parent.relative_to(input_folder_obj)
        except ValueError:
            # Handle case when input is not relative to input folder (e.g., single file dropped?)
            # In this case, output directly to the base output folder
            logger.debug(f"File {anonymized_input} is not relative to input base {input_base_folder}. Outputting directly to {output_base_folder}.")
            # relative_dir remains "."

        output_dir = output_folder_obj / relative_dir
        output_filename = input_path_obj.stem + ".mkv" # Ensure .mkv extension
        output_path = output_dir / output_filename
        anonymized_output = anonymize_filename(str(output_path))

    except Exception as e:
        logger.error(f"Error determining output path for {anonymized_input}: {e}")
        # If path calculation fails, we can't reliably check existence, assume conversion needed
        return True, "Error determining output path", None

    # --- Check Output Existence ---
    # Only skip here if the output exists AND it's NOT an in-place conversion (which needs codec check)
    if output_path.exists() and not overwrite and input_path_obj != output_path:
        reason = "Output file exists"
        logger.info(f"Skipping {anonymized_input} - {reason}: {anonymized_output}")
        return False, reason, None

    # --- Video Info Caching & Retrieval ---
    cache_was_hit = False
    if video_info_cache is not None and input_video_path in video_info_cache:
        video_info = video_info_cache[input_video_path]
        cache_was_hit = True
        logging.debug(f"Cache hit for {anonymized_input}")
    else:
        try:
            video_info = get_video_info(input_video_path)
            if video_info and video_info_cache is not None:
                video_info_cache[input_video_path] = video_info # Store only on success
                logging.debug(f"Cache miss, stored info for {anonymized_input}")
            elif not video_info:
                 logging.warning(f"get_video_info failed for {anonymized_input} (cache hit: {cache_was_hit})")
            # else: video_info is now populated or None if failed
        except Exception as e:
            logging.error(f"Error getting video info for {anonymized_input}: {e}", exc_info=True)
            video_info = None # Ensure video_info is None on error

    # --- Analyze Video Info (if retrieved successfully) ---
    if not video_info:
        # If analysis failed (either initially or during cache retrieval)
        reason = "Analysis failed"
        logger.warning(f"Cannot analyze {anonymized_input} - will attempt conversion.")
        return True, reason, None # Assume conversion needed if analysis fails

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
            return False, reason, video_info # Return info even if skipped

        # Check resolution before other checks
        if width < MIN_RESOLUTION_WIDTH and height < MIN_RESOLUTION_HEIGHT:
            reason = f"Below minimum resolution ({width}x{height}) - needs at least {MIN_RESOLUTION_WIDTH}x{MIN_RESOLUTION_HEIGHT}"
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
            return False, reason, video_info # Return info even if skipped
        else:
            # Determine specific reason for conversion
            if not is_already_av1: reason = "Needs conversion (codec is not AV1)"
            elif not is_mkv_container: reason = "Needs conversion (AV1 but not MKV container)"
            else: reason = "Needs conversion" # Fallback reason

            logger.info(f"Conversion needed for {anonymized_input} - Reason: {reason}")
            return True, reason, video_info

    except Exception as e:
        logger.error(f"Error checking video stream info for {anonymized_input}: {str(e)}", exc_info=True)
        # If an unexpected error occurs during stream check, assume conversion might be needed
        return True, f"Error during stream check: {str(e)}", video_info