# src/video_conversion.py
"""
Video conversion functions for the AV1 Video Converter application.

This module provides functions to convert videos to AV1 format using ab-av1.
"""

import logging
import time
import traceback
from pathlib import Path
from typing import Any, Callable

from src.ab_av1.exceptions import (
    AbAv1Error,
    ConversionNotWorthwhileError,
    EncodingError,
    InputFileError,
    OutputFileError,
    VMAFError,
)

# Corrected import path: Use src.ab_av1 instead of src.ab_av1_wrapper
from src.ab_av1.wrapper import AbAv1Wrapper

# Import constants from config
from src.cache_helpers import can_reuse_crf, is_file_unchanged
from src.config import DEFAULT_ENCODING_PRESET, DEFAULT_VMAF_TARGET, MIN_OUTPUT_FILE_SIZE
from src.history_index import get_history_index
from src.models import FileStatus, OutputMode
from src.privacy import anonymize_filename
from src.utils import format_file_size, format_time, get_video_info, log_conversion_result, log_video_properties

logger = logging.getLogger(__name__)


def calculate_output_path(
    input_path: str,
    output_mode: OutputMode,
    suffix: str | None = None,
    output_folder: str | None = None,
    source_folder: str | None = None,
) -> tuple[Path, bool, bool]:
    """Calculate output path based on output mode.

    Args:
        input_path: Path to input video file
        output_mode: REPLACE, SUFFIX, or SEPARATE_FOLDER
        suffix: Suffix to append for SUFFIX mode (e.g., "_av1")
        output_folder: Target folder for SEPARATE_FOLDER mode
        source_folder: Base folder for preserving subfolder structure (for folder queue items)

    Returns:
        Tuple of (output_path, overwrite, delete_original)
        - output_path: Path object for the output file
        - overwrite: Whether to allow overwriting existing files
        - delete_original: Whether to delete the original after conversion
    """
    input_file = Path(input_path).resolve()

    if output_mode == OutputMode.REPLACE:
        # Same folder as input, .mkv extension
        output_path = input_file.with_suffix(".mkv")
        overwrite = True
        # Only delete original if the output path is different from input
        # (i.e., input wasn't already .mkv)
        delete_original = input_file != output_path

    elif output_mode == OutputMode.SUFFIX:
        # Same folder as input, add suffix before extension
        suffix_str = suffix if suffix else "_av1"
        output_filename = input_file.stem + suffix_str + ".mkv"
        output_path = input_file.parent / output_filename
        overwrite = False
        delete_original = False

    elif output_mode == OutputMode.SEPARATE_FOLDER:
        # Output to different folder, preserve subfolder structure if source_folder provided
        if not output_folder:
            raise ValueError("output_folder is required for SEPARATE_FOLDER mode")

        output_base = Path(output_folder).resolve()

        # Preserve subfolder structure if source_folder is provided
        if source_folder:
            source_base = Path(source_folder).resolve()
            try:
                # Get the relative path from source_folder to input file's parent
                relative_dir = input_file.parent.relative_to(source_base)
                output_dir = output_base / relative_dir
            except ValueError:
                # Input is not relative to source_folder, output to base
                logger.warning(
                    f"Input {input_file} not relative to source folder {source_base}. Outputting to base: {output_base}"
                )
                output_dir = output_base
        else:
            # No source folder - output directly to output_folder
            output_dir = output_base

        output_filename = input_file.stem + ".mkv"
        output_path = output_dir / output_filename
        overwrite = False
        delete_original = False

    else:
        raise ValueError(f"Unknown output mode: {output_mode}")

    return output_path, overwrite, delete_original


def process_video(
    video_path: str,
    output_path: str,
    overwrite: bool = False,
    delete_original: bool = False,
    convert_audio: bool = True,
    audio_codec: str = "opus",
    file_info_callback: Callable[..., Any] | None = None,
    pid_callback: Callable[..., Any] | None = None,
    total_duration_seconds: float = 0.0,
    hw_decoder: str | None = None,
) -> tuple[str, float, int, int, int | None, float | None, int | None, float, float] | None:
    """
    Process a single video file using ab-av1 with hardcoded quality settings.

    Args:
        video_path: Path to the input video file
        output_path: Pre-calculated path for the output file
        overwrite: Whether to overwrite existing output files
        delete_original: Whether to delete the original file after successful conversion
        convert_audio: Whether to convert audio to a different codec
        audio_codec: Target audio codec if conversion is enabled
        file_info_callback: Optional callback for reporting file status changes
        pid_callback: Optional callback for receiving process ID
        total_duration_seconds: Total duration of the input video in seconds (for progress calc)
        hw_decoder: Optional hardware decoder name (e.g., "h264_cuvid", "hevc_qsv")

    Returns:
        tuple: (output_path, elapsed_time, input_size, output_size, final_crf, final_vmaf,
                final_vmaf_target, crf_search_time_sec, encoding_time_sec) on success, None otherwise.
    """
    input_path = Path(video_path).resolve()
    output_path_obj = Path(output_path).resolve()
    # Anonymize early for logging consistency
    anonymized_input_name = anonymize_filename(str(input_path))
    anonymized_output_name = anonymize_filename(str(output_path_obj))

    logger.debug(f"Processing video: {anonymized_input_name}")
    logger.debug(f"Output path: {anonymized_output_name}")

    # --- Create Output Directory ---
    try:
        output_path_obj.parent.mkdir(parents=True, exist_ok=True)
    except Exception as e:
        error_msg = f"Failed to create output directory '{output_path_obj.parent}': {e}"
        logger.exception(error_msg)
        if file_info_callback:
            file_info_callback(input_path.name, "failed", {"message": error_msg, "type": "output_dir_error"})
        return None

    # --- Initial Skip Checks (Standard Overwrite/Existence) ---
    # This check is primarily for non-in-place conversions where output exists
    if output_path_obj.exists() and not overwrite and input_path != output_path_obj:
        logger.info(f"Skipping {anonymized_input_name} - output exists: {anonymized_output_name}")
        if file_info_callback:
            file_info_callback(input_path.name, "skipped", "Output exists")
        return None

    # --- Get Video Info ---
    video_info = None
    input_size = 0
    try:
        video_info = get_video_info(str(input_path))
        if not video_info:
            if file_info_callback:
                file_info_callback(
                    input_path.name,
                    "failed",
                    {"message": f"Cannot analyze {input_path.name}", "type": "analysis_failed"},
                )
            return None
        input_size = video_info.get("file_size", 0)
    except Exception as e:
        error_msg = f"Error analyzing {anonymized_input_name}: {e}"
        logger.error(error_msg, exc_info=True)
        if file_info_callback:
            file_info_callback(
                input_path.name, "failed", {"message": error_msg, "type": "analysis_error", "details": str(e)}
            )
        return None

    # --- Log conversion start ---
    logger.info(f"Processing {anonymized_input_name} - Size: {format_file_size(input_size)}")
    logger.info(f"Output will be written to: {anonymized_output_name}")
    if file_info_callback:
        file_info_callback(input_path.name, "file_info", {"file_size_mb": input_size / (1024**2) if input_size else 0})

    # Use constants from config
    logger.info(f"Using settings -> Preset: {DEFAULT_ENCODING_PRESET}, VMAF Target: {DEFAULT_VMAF_TARGET}")
    log_video_properties(video_info, prefix="Input")

    # --- Check for Cached CRF from Quality Analysis ---
    index = get_history_index()
    record = index.lookup_file(str(input_path))
    use_cached_crf = False
    cached_crf = None

    if record:
        # Check if file was already marked as not worthwhile
        if record.status == FileStatus.NOT_WORTHWHILE:
            if is_file_unchanged(record, str(input_path)):
                logger.info(f"Skipping {anonymized_input_name} - previously marked not worthwhile")
                if file_info_callback:
                    file_info_callback(
                        input_path.name,
                        "skipped_not_worth",
                        {
                            "message": record.skip_reason or "Not worth converting",
                            "original_size": input_size,
                            "min_vmaf_attempted": record.min_vmaf_attempted,
                        },
                    )
                return None
            logger.info(f"File {anonymized_input_name} changed since NOT_WORTHWHILE analysis, re-attempting")

        # Check if we can reuse cached CRF
        if is_file_unchanged(record, str(input_path)) and can_reuse_crf(
            record, DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET
        ):
            use_cached_crf = True
            cached_crf = record.best_crf
            logger.info(
                f"Using cached CRF {cached_crf} for {anonymized_input_name} "
                f"(VMAF {record.vmaf_target_when_analyzed}, preset {record.preset_when_analyzed})"
            )

    # --- Execute Conversion ---
    conversion_start_time = time.time()
    result_stats = None
    output_size = 0
    final_crf = None
    final_vmaf = None
    final_vmaf_target = None
    try:
        ab_av1 = AbAv1Wrapper()

        if use_cached_crf and cached_crf is not None:
            # Use cached CRF - skip CRF search phase
            logger.info(f"Starting ab-av1 encode (cached CRF) for {anonymized_input_name} -> {anonymized_output_name}")
            result_stats = ab_av1.encode_with_crf(
                input_path=str(input_path),
                output_path=str(output_path_obj),
                crf=cached_crf,
                preset=DEFAULT_ENCODING_PRESET,
                file_info_callback=file_info_callback,
                pid_callback=pid_callback,
                total_duration_seconds=total_duration_seconds,
                hw_decoder=hw_decoder,
            )
        else:
            # No cache - run full auto-encode with CRF search
            logger.info(f"Starting ab-av1 auto-encode for {anonymized_input_name} -> {anonymized_output_name}")
            result_stats = ab_av1.auto_encode(
                input_path=str(input_path),
                output_path=str(output_path_obj),
                file_info_callback=file_info_callback,
                pid_callback=pid_callback,
                total_duration_seconds=total_duration_seconds,
                hw_decoder=hw_decoder,
            )

        conversion_elapsed_time = time.time() - conversion_start_time
        cache_note = " (cached CRF)" if use_cached_crf else ""
        logger.info(
            f"ab-av1 finished{cache_note} for {anonymized_input_name} in {format_time(conversion_elapsed_time)}."
        )

        # --- Post-Conversion Verification & Stat Gathering ---
        if not output_path_obj.exists():
            raise OutputFileError(f"Output missing: {anonymized_output_name}", error_type="missing_output")
        output_size = output_path_obj.stat().st_size
        if output_size < MIN_OUTPUT_FILE_SIZE:  # Check if output is suspiciously small
            error_msg = f"Output too small ({output_size} bytes): {anonymized_output_name}"
            try:
                output_path_obj.unlink()
            except OSError as rm_err:
                logger.warning(f"Cannot remove small file {anonymized_output_name}: {rm_err}")
            raise OutputFileError(error_msg, error_type="invalid_output")

        log_conversion_result(str(input_path), str(output_path_obj), conversion_elapsed_time)

        final_vmaf = result_stats.get("vmaf") if result_stats else None
        final_crf = result_stats.get("crf") if result_stats else None
        if use_cached_crf and record and record.vmaf_target_when_analyzed is not None:
            final_vmaf_target = record.vmaf_target_when_analyzed
        else:
            final_vmaf_target = result_stats.get("vmaf_target_used") if result_stats else DEFAULT_VMAF_TARGET
        logger.info(
            f"Conversion successful - Final VMAF: {final_vmaf if final_vmaf else 'N/A'}, "
            f"Final CRF: {final_crf if final_crf else 'N/A'} (Target VMAF: {final_vmaf_target})"
        )

        # Check VMAF achieved vs target used (use a small tolerance)
        if (
            isinstance(final_vmaf, (int, float))
            and isinstance(final_vmaf_target, (int, float))
            and final_vmaf < final_vmaf_target - 1.0
        ):
            logger.warning(
                f"Final VMAF {final_vmaf:.1f} is below target {final_vmaf_target} for {anonymized_input_name}"
            )

        # --- Delete Original if Requested ---
        if delete_original and input_path != output_path_obj:
            try:
                logger.info(f"Deleting original file: {anonymized_input_name}")
                input_path.unlink()
                logger.debug(f"Successfully deleted original: {anonymized_input_name}")
            except Exception as e:
                logger.warning(f"Failed to delete original file {anonymized_input_name}: {e}")
                # Don't fail the conversion if we can't delete the original

        # Extract timing breakdown from result_stats
        crf_search_time = result_stats.get("crf_search_time_sec", 0) if result_stats else 0
        encoding_time = result_stats.get("encoding_time_sec", 0) if result_stats else conversion_elapsed_time

        # Return success tuple including stats for history
        return (
            str(output_path_obj),
            conversion_elapsed_time,
            input_size,
            output_size,
            final_crf,
            final_vmaf,
            final_vmaf_target,
            crf_search_time,
            encoding_time,
        )

    except ConversionNotWorthwhileError as e:
        # Note: The wrapper already called the file_info_callback with "skipped_not_worth"
        # before raising this exception, so we don't call it again here to avoid double-counting
        logger.info(f"Conversion not worthwhile for {anonymized_input_name}: {e}")
        return None  # Return None like other skipped files, not an error
    except (InputFileError, OutputFileError, VMAFError, EncodingError, AbAv1Error):
        # Note: The wrapper already called the file_info_callback with "failed" status
        # before raising these exceptions, so we don't call it again here to avoid double-counting
        logger.exception(f"Conversion failed for {anonymized_input_name}")
        return None
    except Exception as e:
        stack_trace = traceback.format_exc()
        logger.exception(f"Unexpected error processing {anonymized_input_name}")
        logger.debug(f"Stack trace:\n{stack_trace}")
        if file_info_callback:
            file_info_callback(
                input_path.name,
                "failed",
                {
                    "message": f"Unexpected error: {e}",
                    "type": "unexpected_error",
                    "details": str(e),
                    "stack_trace": stack_trace,
                },
            )
        return None
