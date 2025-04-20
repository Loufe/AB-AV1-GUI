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

from convert_app.ab_av1_wrapper import (
    AbAv1Wrapper,
    InputFileError, OutputFileError, VMAFError, EncodingError, AbAv1Error
)
from convert_app.utils import (
    log_video_properties, log_conversion_result, anonymize_filename,
    get_video_info, DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET,
    format_file_size, format_time
)

logger = logging.getLogger(__name__)


def process_video(video_path: str, input_folder: str, output_folder: str, overwrite: bool = False,
                 convert_audio: bool = True, audio_codec: str = "opus",
                 progress_callback: callable = None, file_info_callback: callable = None, pid_callback: callable = None) -> tuple:
    """
    Process a single video file using ab-av1 with hardcoded quality settings.
    
    Args:
        video_path: Path to the input video file
        input_folder: Base input folder path for calculating relative paths
        output_folder: Destination folder for converted files
        overwrite: Whether to overwrite existing output files
        convert_audio: Whether to convert audio to a different codec
        audio_codec: Target audio codec if conversion is enabled
        progress_callback: Optional callback for reporting progress
        file_info_callback: Optional callback for reporting file status changes
        pid_callback: Optional callback for receiving process ID
        
    Returns:
        tuple: (output_path, elapsed_time, input_size, output_size, final_crf, final_vmaf, final_vmaf_target) on success, None otherwise.
    """
    input_path = Path(video_path).resolve()
    input_folder_path = Path(input_folder).resolve()
    output_folder_path = Path(output_folder).resolve()
    # Anonymize early for logging consistency
    anonymized_input_name = anonymize_filename(str(input_path))

    logging.debug(f"Processing video: {anonymized_input_name}")

    # Determine output path
    try:
        relative_dir = input_path.parent.relative_to(input_folder_path)
        output_dir = output_folder_path / relative_dir
    except ValueError:
        logging.warning(f"Input {anonymized_input_name} not relative. Outputting to base.")
        output_dir = output_folder_path; relative_dir = Path(".")
    except Exception as e:
         error_msg = f"Error calculating output dir: {e}"; logging.error(error_msg)
         if file_info_callback: file_info_callback(input_path.name, "failed", {"message":error_msg,"type":"output_dir_error"})
         return None

    try: output_dir.mkdir(parents=True, exist_ok=True)
    except Exception as e:
        error_msg = f"Failed create output dir '{output_dir}': {e}"; logging.error(error_msg)
        if file_info_callback: file_info_callback(input_path.name, "failed", {"message":error_msg,"type":"output_dir_error"})
        return None

    output_filename = input_path.stem + ".mkv"
    output_path = output_dir / output_filename
    anonymized_output_name = anonymize_filename(str(output_path)) # Anonymize expected output
    logging.debug(f"Output Path: {anonymized_output_name}") # Log anonymized

    # --- Pre-flight Checks ---
    if output_path.exists() and not overwrite:
        logging.info(f"Skipping {anonymized_input_name} - output exists: {anonymized_output_name}")
        if file_info_callback: file_info_callback(input_path.name, "skipped", f"Output exists")
        return None

    video_info = None; input_size = 0; input_vcodec = "?"; input_acodec = "?"; input_duration = 0
    try: # Get Pre-conversion Info
        video_info = get_video_info(str(input_path))
        if not video_info:
            if file_info_callback: file_info_callback(input_path.name, "failed", {"message":f"Cannot analyze {input_path.name}", "type":"analysis_failed"})
            return None
        input_size = video_info.get('file_size', 0)
        for stream in video_info.get("streams", []):
            if stream.get("codec_type") == "video": input_vcodec = stream.get("codec_name", "?").upper()
            elif stream.get("codec_type") == "audio": input_acodec = stream.get("codec_name", "?").upper()
        try: input_duration = float(video_info.get('format', {}).get('duration', '0'))
        except: input_duration = 0
    except Exception as e:
        error_msg = f"Error analyzing {anonymized_input_name}: {e}"; logging.error(error_msg, exc_info=True)
        if file_info_callback: file_info_callback(input_path.name, "failed", {"message":error_msg, "type":"analysis_error", "details":str(e)})
        return None

    # Re-check if already AV1/MKV
    is_already_av1 = False; video_stream_found = False
    for stream in video_info.get("streams", []):
        if stream.get("codec_type") == "video":
            video_stream_found = True
            if stream.get("codec_name", "").lower() == "av1": is_already_av1 = True; break
    if not video_stream_found:
         logging.warning(f"No video stream in {anonymized_input_name} - skipping.");
         if file_info_callback: file_info_callback(input_path.name, "skipped", "No video stream")
         return None
    is_mkv_container = input_path.suffix.lower() == ".mkv"
    if is_already_av1 and is_mkv_container:
        logging.info(f"Skipping {anonymized_input_name} - already AV1/MKV.");
        if file_info_callback: file_info_callback(input_path.name, "skipped", "Already AV1/MKV")
        return None

    logging.info(f"Processing {anonymized_input_name} - Size: {format_file_size(input_size)}")
    if file_info_callback: file_info_callback(input_path.name, "file_info", {"file_size_mb": input_size / (1024**2) if input_size else 0})

    logging.info(f"Using settings -> Preset: {DEFAULT_ENCODING_PRESET}, VMAF Target: {DEFAULT_VMAF_TARGET}")
    log_video_properties(video_info, prefix="Input")

    # --- Execute Conversion ---
    conversion_start_time = time.time(); result_stats = None; output_size = 0
    final_crf = None; final_vmaf = None; final_vmaf_target = None
    try:
        logging.info(f"Starting ab-av1 for {anonymized_input_name}")
        ab_av1 = AbAv1Wrapper()
        result_stats = ab_av1.auto_encode(
            input_path=str(input_path), output_path=str(output_path),
            progress_callback=progress_callback, file_info_callback=file_info_callback,
            pid_callback=pid_callback
        )
        conversion_elapsed_time = time.time() - conversion_start_time
        logging.info(f"ab-av1 finished for {anonymized_input_name} in {format_time(conversion_elapsed_time)}.")

        # --- Post-Conversion Verification & Stat Gathering ---
        if not output_path.exists(): raise OutputFileError(f"Output missing: {anonymized_output_name}", error_type="missing_output")
        output_size = output_path.stat().st_size
        if output_size < 1024:
             error_msg = f"Output too small ({output_size} bytes): {anonymized_output_name}"
             try: output_path.unlink()
             except OSError as rm_err: logging.warning(f"Cannot remove small file {anonymized_output_name}: {rm_err}")
             raise OutputFileError(error_msg, error_type="invalid_output")

        log_conversion_result(str(input_path), str(output_path), conversion_elapsed_time)

        final_vmaf = result_stats.get('vmaf') if result_stats else None
        final_crf = result_stats.get('crf') if result_stats else None
        final_vmaf_target = result_stats.get('vmaf_target_used') if result_stats else DEFAULT_VMAF_TARGET # Get actual target used
        logging.info(f"Conversion successful - Final VMAF: {final_vmaf if final_vmaf else 'N/A'}, Final CRF: {final_crf if final_crf else 'N/A'} (Target VMAF: {final_vmaf_target})")

        # Check VMAF achieved vs target used
        if isinstance(final_vmaf, (int, float)) and final_vmaf < final_vmaf_target - 1.0:
             logging.warning(f"Final VMAF {final_vmaf:.1f} is below target {final_vmaf_target} for {anonymized_input_name}")

        output_acodec = input_acodec
        if convert_audio and input_acodec.lower() not in ['aac', 'opus']: output_acodec = audio_codec

        # Return success tuple including stats for history
        return (str(output_path), conversion_elapsed_time, input_size, output_size, final_crf, final_vmaf, final_vmaf_target)

    except (InputFileError, OutputFileError, VMAFError, EncodingError, AbAv1Error) as e:
        logging.error(f"ab-av1 wrapper error for {anonymized_input_name}: {e}")
        return None
    except Exception as e:
        stack_trace = traceback.format_exc()
        logging.error(f"Unexpected error processing {anonymized_input_name}: {e}")
        logging.debug(f"Stack trace:\n{stack_trace}")
        if file_info_callback:
            file_info_callback(input_path.name, "failed", {"message":f"Unexpected error: {e}", "type":"unexpected_error", "details":str(e), "stack_trace":stack_trace})
        return None