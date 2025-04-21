#src/video_conversion.py
"""
Video conversion functions for the AV1 Video Converter application.

This module provides functions to convert videos to AV1 format using ab-av1.
"""
import os
import sys
import time
import logging
import traceback
from pathlib import Path

# Corrected import path: Use src.ab_av1 instead of src.ab_av1_wrapper
from src.ab_av1.wrapper import AbAv1Wrapper
from src.ab_av1.exceptions import (
    InputFileError, OutputFileError, VMAFError, EncodingError, AbAv1Error
)
from src.utils import (
    log_video_properties, log_conversion_result, anonymize_filename,
    get_video_info, format_file_size, format_time
)
# Import constants from config
from src.config import (
    DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET
)

logger = logging.getLogger(__name__)


def process_video(video_path: str, input_folder: str, output_folder: str, overwrite: bool = False,
                 convert_audio: bool = True, audio_codec: str = "opus",
                 progress_callback: callable = None, file_info_callback: callable = None, pid_callback: callable = None,
                 total_duration_seconds: float = 0.0) -> tuple: # Added total_duration_seconds
    """
    Process a single video file using ab-av1 with hardcoded quality settings.

    Args:
        video_path: Path to the input video file
        input_folder: Base input folder path for calculating relative paths
        output_folder: Destination folder for converted files
        overwrite: Whether to overwrite existing output files
        convert_audio: Whether to convert audio to a different codec
        audio_codec: Target audio codec if conversion is enabled
        progress_callback: Optional callback for reporting progress (legacy, not used directly)
        file_info_callback: Optional callback for reporting file status changes
        pid_callback: Optional callback for receiving process ID
        total_duration_seconds: Total duration of the input video in seconds (for progress calc)

    Returns:
        tuple: (output_path, elapsed_time, input_size, output_size, final_crf, final_vmaf, final_vmaf_target) on success, None otherwise.
    """
    input_path = Path(video_path).resolve()
    input_folder_path = Path(input_folder).resolve()
    output_folder_path = Path(output_folder).resolve()
    # Anonymize early for logging consistency
    anonymized_input_name = anonymize_filename(str(input_path))

    logging.debug(f"Processing video: {anonymized_input_name}")

    # --- Determine Standard Output Path ---
    try:
        relative_dir = input_path.parent.relative_to(input_folder_path)
        output_dir = output_folder_path / relative_dir
    except ValueError:
        logging.warning(f"Input {anonymized_input_name} not relative to input base. Outputting to base: {output_folder_path}")
        output_dir = output_folder_path
        relative_dir = Path(".")
    except Exception as e:
         error_msg = f"Error calculating output dir: {e}"; logging.error(error_msg)
         if file_info_callback: file_info_callback(input_path.name, "failed", {"message":error_msg,"type":"output_dir_error"})
         return None

    try: output_dir.mkdir(parents=True, exist_ok=True)
    except Exception as e:
        error_msg = f"Failed create output dir '{output_dir}': {e}"; logging.error(error_msg)
        if file_info_callback: file_info_callback(input_path.name, "failed", {"message":error_msg,"type":"output_dir_error"})
        return None

    # Calculate the standard output path (without suffix yet)
    output_filename = input_path.stem + ".mkv"
    output_path = output_dir / output_filename

    # --- Initial Skip Checks (Standard Overwrite/Existence) ---
    # This check is primarily for non-in-place conversions where output exists
    if output_path.exists() and not overwrite and input_path != output_path:
        anonymized_output_name = anonymize_filename(str(output_path))
        logging.info(f"Skipping {anonymized_input_name} - output exists (standard check): {anonymized_output_name}")
        if file_info_callback: file_info_callback(input_path.name, "skipped", f"Output exists")
        return None

    # --- Get Video Info ---
    video_info = None; input_size = 0; input_vcodec = "?"; input_acodec = "?"
    is_av1 = False # Flag to check if input is AV1
    try:
        video_info = get_video_info(str(input_path))
        if not video_info:
            if file_info_callback: file_info_callback(input_path.name, "failed", {"message":f"Cannot analyze {input_path.name}", "type":"analysis_failed"})
            return None
        input_size = video_info.get('file_size', 0)
        for stream in video_info.get("streams", []):
            if stream.get("codec_type") == "video":
                input_vcodec = stream.get("codec_name", "?").upper()
                if input_vcodec == "AV1": is_av1 = True
            elif stream.get("codec_type") == "audio":
                input_acodec = stream.get("codec_name", "?").upper()

    except Exception as e:
        error_msg = f"Error analyzing {anonymized_input_name}: {e}"; logging.error(error_msg, exc_info=True)
        if file_info_callback: file_info_callback(input_path.name, "failed", {"message":error_msg, "type":"analysis_error", "details":str(e)})
        return None

    # --- Handle In-Place, No Overwrite, Non-AV1 MKV Scenario ---
    if input_path == output_path and not overwrite:
        # This means input is MKV, output folder = input folder
        if is_av1:
            # If it's already AV1, we just skip it as there's nothing to do (in-place, no overwrite)
            logging.info(f"Skipping {anonymized_input_name} - Already AV1/MKV (in-place, no overwrite)")
            if file_info_callback: file_info_callback(input_path.name, "skipped", "Already AV1/MKV (in-place, no overwrite)")
            return None
        else:
            # Input is MKV, but NOT AV1. Attempt to add suffix.
            suffixed_filename = input_path.stem + " (av1).mkv"
            suffixed_output_path = output_dir / suffixed_filename
            anonymized_suffixed_output = anonymize_filename(str(suffixed_output_path))

            if suffixed_output_path.exists():
                # If the suffixed file *also* exists, we must skip
                logging.warning(f"Skipping {anonymized_input_name} - Output with suffix '{suffixed_filename}' already exists: {anonymized_suffixed_output}")
                if file_info_callback: file_info_callback(input_path.name, "skipped", f"Output with '(av1)' suffix exists")
                return None
            else:
                # Use the suffixed path for this conversion
                output_path = suffixed_output_path
                logging.info(f"Input is non-AV1 MKV; Overwrite disabled. Using suffixed output: {anonymized_suffixed_output}")

    # --- Check again if final output path exists (covers case where suffix was added) ---
    # Note: This check might seem redundant if the suffix logic already checked,
    # but it ensures correctness if the suffix logic isn't hit.
    # We only check existence now, as the overwrite logic for the standard path was handled earlier.
    # If the path was modified by the suffix logic, its existence was checked there.
    if output_path.exists() and not overwrite:
        # This condition should primarily catch the case where the *suffixed* file already exists
        # (which was checked above) OR if somehow the initial non-in-place check was bypassed.
        # Log it for clarity.
        anonymized_output_name = anonymize_filename(str(output_path))
        logging.warning(f"Skipping {anonymized_input_name} - Final determined output path exists (post-suffix check): {anonymized_output_name}")
        if file_info_callback: file_info_callback(input_path.name, "skipped", f"Final output path exists")
        return None


    # If we got here, conversion is needed. Proceed.
    anonymized_final_output_name = anonymize_filename(str(output_path)) # Anonymize final path for logging
    logging.debug(f"Final Output Path: {anonymized_final_output_name}") # Log final path

    logging.info(f"Processing {anonymized_input_name} - Size: {format_file_size(input_size)}")
    if file_info_callback: file_info_callback(input_path.name, "file_info", {"file_size_mb": input_size / (1024**2) if input_size else 0})

    # Use constants from config
    logging.info(f"Using settings -> Preset: {DEFAULT_ENCODING_PRESET}, VMAF Target: {DEFAULT_VMAF_TARGET}")
    log_video_properties(video_info, prefix="Input")

    # --- Execute Conversion ---
    conversion_start_time = time.time(); result_stats = None; output_size = 0
    final_crf = None; final_vmaf = None; final_vmaf_target = None
    try:
        logging.info(f"Starting ab-av1 for {anonymized_input_name} -> {anonymized_final_output_name}")
        ab_av1 = AbAv1Wrapper()
        result_stats = ab_av1.auto_encode(
            input_path=str(input_path),
            output_path=str(output_path), # Use the potentially modified output_path
            file_info_callback=file_info_callback,
            pid_callback=pid_callback,
            total_duration_seconds=total_duration_seconds # Pass duration
        )
        conversion_elapsed_time = time.time() - conversion_start_time
        logging.info(f"ab-av1 finished for {anonymized_input_name} in {format_time(conversion_elapsed_time)}.")

        # --- Post-Conversion Verification & Stat Gathering ---
        if not output_path.exists(): raise OutputFileError(f"Output missing: {anonymized_final_output_name}", error_type="missing_output")
        output_size = output_path.stat().st_size
        if output_size < 1024: # Check if output is suspiciously small
             error_msg = f"Output too small ({output_size} bytes): {anonymized_final_output_name}"
             try: output_path.unlink()
             except OSError as rm_err: logging.warning(f"Cannot remove small file {anonymized_final_output_name}: {rm_err}")
             raise OutputFileError(error_msg, error_type="invalid_output")

        log_conversion_result(str(input_path), str(output_path), conversion_elapsed_time)

        final_vmaf = result_stats.get('vmaf') if result_stats else None
        final_crf = result_stats.get('crf') if result_stats else None
        final_vmaf_target = result_stats.get('vmaf_target_used') if result_stats else DEFAULT_VMAF_TARGET # Get actual target used
        logging.info(f"Conversion successful - Final VMAF: {final_vmaf if final_vmaf else 'N/A'}, Final CRF: {final_crf if final_crf else 'N/A'} (Target VMAF: {final_vmaf_target})")

        # Check VMAF achieved vs target used (use a small tolerance)
        if isinstance(final_vmaf, (int, float)) and isinstance(final_vmaf_target, (int, float)) and final_vmaf < final_vmaf_target - 1.0:
             logging.warning(f"Final VMAF {final_vmaf:.1f} is below target {final_vmaf_target} for {anonymized_input_name}")

        output_acodec = input_acodec # Default to original
        if convert_audio and input_acodec.lower() not in ['aac', 'opus']: output_acodec = audio_codec.lower() # Store lowercase

        # Return success tuple including stats for history
        return (str(output_path), conversion_elapsed_time, input_size, output_size, final_crf, final_vmaf, final_vmaf_target)

    except (InputFileError, OutputFileError, VMAFError, EncodingError, AbAv1Error) as e:
        # These errors are logged by the wrapper or dispatcher, just return None
        # Logging the specific error type here might be redundant
        logging.error(f"Conversion failed for {anonymized_input_name}: {e}")
        # Ensure error callback is triggered if not already done by wrapper
        if file_info_callback:
            file_info_callback(input_path.name, "failed", {"message": str(e), "type": getattr(e, 'error_type', 'conversion_error')})
        return None
    except Exception as e:
        stack_trace = traceback.format_exc()
        logging.error(f"Unexpected error processing {anonymized_input_name}: {e}")
        logging.debug(f"Stack trace:\n{stack_trace}")
        if file_info_callback:
            file_info_callback(input_path.name, "failed", {"message":f"Unexpected error: {e}", "type":"unexpected_error", "details":str(e), "stack_trace":stack_trace})
        return None