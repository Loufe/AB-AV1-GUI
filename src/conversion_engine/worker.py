# src/conversion_engine/worker.py
"""
Contains the main worker thread function for sequential video conversion.
"""

# Standard library imports
import dataclasses
import datetime  # For history timestamp
import logging
import os
import time

# GUI-related imports (for type hinting gui object, not direct use of widgets here)
from pathlib import Path
from typing import Callable  # Import Callable

# Project imports
from src.models import ConversionConfig, HistoryRecord
from src.utils import (
    anonymize_filename,
    append_to_history,
    format_file_size,
    get_video_info,
    update_ui_safely,
)
from src.video_conversion import process_video

# Import functions/modules from the engine package
from .scanner import scan_video_needs_conversion

logger = logging.getLogger(__name__)


def sequential_conversion_worker(
    gui,
    config: ConversionConfig,
    stop_event,
    file_event_callback: Callable,
    reset_ui_callback: Callable,
    elapsed_time_callback: Callable,
    pid_storage_callback: Callable,
    completion_callback: Callable,
):
    """Process files sequentially, scanning and converting each eligible video.

    This is the main worker function that runs in a separate thread to handle
    the entire conversion process from scanning to processing.

    Args:
        gui: The main GUI instance (passed for accessing settings, state, and root window).
        config: ConversionConfig containing all conversion settings.
        stop_event: Threading event to signal stopping.
        file_event_callback: Callback for file conversion events (progress, errors, completion).
        reset_ui_callback: Callback to reset UI details for a new file.
        elapsed_time_callback: Callback to update elapsed time display.
        pid_storage_callback: Function to call to store the process ID (e.g., store_process_id(gui, pid, path)).
        completion_callback: Function to call when the worker finishes (e.g., conversion_complete(gui, message)).
    """
    gui.output_folder_path = config.output_folder  # Store for potential cleanup use later
    logger.info(f"Worker started. Input: '{config.input_folder}', Output: '{config.output_folder}'")

    # Initialize conversion state variables (thread-safe)
    def init_state():
        gui.error_count = 0
        gui.total_input_bytes_success = 0
        gui.total_output_bytes_success = 0
        gui.total_time_success = 0
        gui.skipped_not_worth_count = 0  # Track files skipped because conversion isn't beneficial
        gui.skipped_not_worth_files = []  # Track filenames of skipped files
        gui.skipped_low_resolution_count = 0  # Track files skipped due to low resolution
        gui.skipped_low_resolution_files = []  # Track filenames of low resolution files
        gui.error_details = []  # Track error details for summary

    update_ui_safely(gui.root, init_state)

    video_info_cache = {}  # Initialize cache for this run

    # Determine extensions from config
    extensions = config.extensions
    if not extensions:
        logger.error("Worker: No extensions selected.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "Error: No extensions selected"))
        return
    logger.info(f"Worker: Processing extensions: {', '.join(extensions)}")

    # --- File Scanning (Quick Count) ---
    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Searching for files..."))
    all_video_files_set = set()
    try:
        input_path_obj = Path(config.input_folder)
        if not input_path_obj.is_dir():
            raise FileNotFoundError(f"Input folder invalid: {config.input_folder}")
        for ext in extensions:
            # Check both lowercase and uppercase extensions
            for pattern in [f"*.{ext.lower()}", f"*.{ext.upper()}"]:
                for file_path in input_path_obj.rglob(pattern):
                    # Ensure it's a file, not a directory ending with the extension
                    if file_path.is_file():
                        all_video_files_set.add(str(file_path.resolve()))
    except Exception:
        logger.exception(f"Error scanning input folder '{config.input_folder}'")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "Error scanning files"))
        return

    all_video_files = sorted(all_video_files_set)
    total_files_found = len(all_video_files)
    logger.info(f"Found {total_files_found} potential files matching extensions.")
    if not all_video_files:
        logger.info("No matching files found in input folder.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "No matching files found"))
        return

    # --- Detailed Scan & Eligibility Check (with progress) ---
    files_to_process = []
    skipped_files_count = 0
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0))  # Reset progress bar
    update_ui_safely(
        gui.root,
        lambda t=total_files_found: gui.status_label.config(
            text=f"Found {t} potential files. Analyzing eligibility..."
        ),
    )

    for i, video_path in enumerate(all_video_files):
        if stop_event.is_set():
            logger.info("Scan interrupted by user stop request.")
            update_ui_safely(gui.root, lambda: completion_callback(gui, "Scan stopped"))
            return

        # Update UI for scan progress
        scan_progress = (i + 1) / total_files_found * 100
        update_ui_safely(gui.root, lambda p=scan_progress: gui.overall_progress.config(value=p))
        update_ui_safely(
            gui.root, lambda i=i, t=total_files_found: gui.status_label.config(text=f"Analyzing file {i + 1}/{t}...")
        )

        try:
            # Call the scanner function (now imported)
            needs_conversion, reason, _ = scan_video_needs_conversion(
                input_video_path=video_path,
                input_base_folder=config.input_folder,
                output_base_folder=config.output_folder,
                overwrite=config.overwrite,
                video_info_cache=video_info_cache,
            )
            if needs_conversion:
                files_to_process.append(video_path)
            else:
                skipped_files_count += 1
                filename = os.path.basename(video_path)

                # Check if skipped due to resolution (thread-safe)
                if "Below minimum resolution" in reason:

                    def update_low_res_skip(fn=filename):
                        gui.skipped_low_resolution_count += 1
                        gui.skipped_low_resolution_files.append(fn)

                    update_ui_safely(gui.root, update_low_res_skip)

                # Log skip reason using the callback
                file_event_callback(filename, "skipped", reason)
        except Exception as e:
            logger.error(
                f"Unexpected error during detailed scan for {anonymize_filename(video_path)}: {e}", exc_info=True
            )
            skipped_files_count += 1
            logger.info(f"Skipping {anonymize_filename(video_path)} due to scan error.")
            # Optionally trigger handle_error here if desired for scan errors

    # Store the list of files to process on the gui object (thread-safe)
    total_videos_to_process = len(files_to_process)

    def set_file_lists():
        gui.video_files = files_to_process
        gui.pending_files = files_to_process.copy()  # Create a copy for tracking remaining files

    update_ui_safely(gui.root, set_file_lists)

    logger.info(
        f"Scan complete: {total_videos_to_process} files require conversion, {skipped_files_count} files skipped."
    )
    logger.info(f"Pending files initialized: {total_videos_to_process} files")

    # --- Transition to Conversion Phase ---
    if not files_to_process:
        logger.info("No files need conversion based on analysis.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "No files need conversion"))
        return

    logger.info(f"Starting conversion of {total_videos_to_process} files...")
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0))  # Reset progress bar for conversion phase
    update_ui_safely(
        gui.root, lambda t=total_videos_to_process: gui.status_label.config(text=f"Starting conversion of {t} files...")
    )

    gui.processed_files = 0
    gui.successful_conversions = 0

    # --- File Processing Loop ---
    for video_path in files_to_process:
        if stop_event.is_set():
            logger.info("Conversion loop interrupted by user stop request.")
            break  # Exit the loop

        # Store current file path for estimation
        gui.current_file_path = video_path
        logger.debug(f"Current file path set to: {video_path}")

        file_number = gui.processed_files + 1
        filename = os.path.basename(video_path)
        anonymized_name = anonymize_filename(video_path)
        update_ui_safely(
            gui.root,
            lambda fn=file_number, fname=filename: gui.status_label.config(
                text=f"Converting {fn}/{total_videos_to_process}: {fname}"
            ),
        )
        update_ui_safely(gui.root, reset_ui_callback)  # Reset UI elements for the new file

        original_size = 0
        input_vcodec = "?"
        input_acodec = "?"
        input_duration = 0.0
        output_acodec = "?"  # Initialize output audio codec

        # --- Use Cached Video Info ---
        video_info = video_info_cache.get(video_path)
        if not video_info:
            # This shouldn't normally happen if scan phase succeeded, but handle defensively
            logger.warning(f"Cache miss during processing phase for {anonymized_name}. Calling get_video_info again.")
            try:
                video_info = get_video_info(video_path)  # Attempt to get info again
            except Exception:
                logger.exception(f"Failed to get video info during processing for {anonymized_name}")
            # Proceed even if video_info is None, process_video might handle it

        # --- Extract Info & Update UI ---
        if video_info:
            try:
                format_info_text = "-"
                size_str = "-"
                for stream in video_info.get("streams", []):
                    codec_type = stream.get("codec_type")
                    if codec_type == "video":
                        input_vcodec = stream.get("codec_name", "?").upper()
                    elif codec_type == "audio":
                        input_acodec = stream.get("codec_name", "?").upper()
                format_info_text = f"{input_vcodec} / {input_acodec}"
                original_size = video_info.get("file_size", 0)
                size_str = format_file_size(original_size)
                duration_str = video_info.get("format", {}).get("duration", "0")
                try:
                    input_duration = float(duration_str)
                except (ValueError, TypeError):
                    input_duration = 0.0
                    logger.warning(f"Invalid duration '{duration_str}' for {anonymized_name}")

                update_ui_safely(gui.root, lambda fi=format_info_text: gui.orig_format_label.config(text=fi))
                update_ui_safely(gui.root, lambda ss=size_str: gui.orig_size_label.config(text=ss))
                gui.last_input_size = original_size  # Store for potential use in handlers
                output_acodec = input_acodec  # Default output codec
            except Exception:
                logger.exception(f"Error extracting details from video_info for {anonymized_name}")
                gui.last_input_size = None
                input_duration = 0.0
        else:
            logger.warning(f"Cannot get pre-conversion info for {anonymized_name}.")
            gui.last_input_size = None
            input_duration = 0.0

        gui.current_file_start_time = time.time()
        gui.current_file_encoding_start_time = None
        if not hasattr(gui, "elapsed_timer_id"):
            gui.elapsed_timer_id = None
        update_ui_safely(gui.root, elapsed_time_callback, gui.current_file_start_time)  # Start timer UI updates
        gui.current_process_info = None  # Reset PID info for the new file

        # --- Process Video ---
        process_successful = False
        output_file_path = None
        elapsed_time_file = 0
        output_size = 0
        final_crf = None
        final_vmaf = None
        final_vmaf_target = None
        try:
            # Pass the dispatcher and the specific PID callback for this file
            # Note: pid_storage_callback is the function reference passed into this worker
            result_tuple = process_video(
                video_path=video_path,
                input_folder=config.input_folder,
                output_folder=config.output_folder,
                overwrite=config.overwrite,
                convert_audio=config.convert_audio,
                audio_codec=config.audio_codec,
                file_info_callback=file_event_callback,  # Pass the dispatcher
                pid_callback=lambda pid, path=video_path: pid_storage_callback(
                    gui, pid, path
                ),  # Use lambda to pass gui object and path
                total_duration_seconds=input_duration,
            )
            if result_tuple:
                # Unpack potentially extended tuple including stats
                output_file_path, elapsed_time_file, _, output_size, final_crf, final_vmaf, final_vmaf_target = (
                    result_tuple
                )
                process_successful = True
                gui.last_output_size = output_size  # Store for use in handle_completed
                gui.last_elapsed_time = elapsed_time_file  # Store for use in handle_completed
                # Determine final audio codec based on conversion settings
                if config.convert_audio and input_acodec.lower() not in ["aac", "opus"]:
                    output_acodec = config.audio_codec.lower()
                # else: output_acodec remains input_acodec (set earlier)
            else:
                # If process_video returns None, it means failure was reported via callback
                gui.last_output_size = None
                gui.last_elapsed_time = None
                process_successful = False  # Ensure state reflects failure

        except Exception as e:
            logger.exception(f"Critical error during process_video call for {anonymized_name}")
            # Dispatch a generic failure if process_video itself crashes
            file_event_callback(
                filename, "failed", {"message": f"Internal processing error: {e}", "type": "processing_crash"}
            )
            process_successful = False
            gui.last_output_size = None
            gui.last_elapsed_time = None

        # --- Post-processing & History ---
        gui.processed_files += 1

        # Remove file from pending list after processing is complete (thread-safe)
        def remove_from_pending(vpath=video_path):
            if vpath in gui.pending_files:
                gui.pending_files.remove(vpath)
                logger.debug(f"Removed completed file {vpath} from pending files. Remaining: {len(gui.pending_files)}")

        update_ui_safely(gui.root, remove_from_pending)

        if process_successful:
            gui.successful_conversions += 1
            try:  # Append to History
                anonymize_hist = gui.anonymize_history.get()
                input_path_for_hist = anonymize_filename(video_path) if anonymize_hist else video_path
                # Ensure output_file_path is a string before anonymizing, provide a placeholder if None
                output_file_path_str = output_file_path if output_file_path is not None else "N/A"
                output_path_for_hist = (
                    anonymize_filename(output_file_path_str) if anonymize_hist else output_file_path_str
                )
                hist_record = HistoryRecord(
                    timestamp=datetime.datetime.now().isoformat(sep=" ", timespec="seconds"),
                    input_file=input_path_for_hist,
                    output_file=output_path_for_hist,
                    input_size_mb=round(original_size / (1024**2), 2) if original_size is not None else None,
                    output_size_mb=round(output_size / (1024**2), 2) if output_size is not None else None,
                    reduction_percent=round(100 - (output_size / original_size * 100), 1)
                    if original_size and output_size and original_size > 0
                    else None,
                    duration_sec=round(input_duration, 1) if input_duration is not None else None,
                    time_sec=round(elapsed_time_file, 1) if elapsed_time_file is not None else None,
                    input_vcodec=input_vcodec,
                    input_acodec=input_acodec,
                    output_acodec=output_acodec,
                    input_codec=input_vcodec,  # Duplicate for compatibility with estimation functions
                    final_crf=final_crf,
                    final_vmaf=round(final_vmaf, 2) if final_vmaf is not None else None,
                    final_vmaf_target=final_vmaf_target if final_vmaf_target is not None else 95,
                )
                append_to_history(dataclasses.asdict(hist_record))
            except Exception:
                logger.exception(f"Failed to append to history for {anonymized_name}")
            # Dispatch completed callback *after* history attempt
            # Pass necessary info for the handler
            completed_info = {"vmaf": final_vmaf, "crf": final_crf, "output_size": output_size}
            file_event_callback(filename, "completed", completed_info)
        else:
            # Error handling is done via the callback dispatcher calling handle_error
            pass

        # --- Delete Original File if Requested ---
        if process_successful and config.delete_original and video_path and os.path.exists(video_path):
            try:
                logger.info(f"Deleting original file as requested: {anonymize_filename(video_path)}")
                os.remove(video_path)
                logger.info(f"Successfully deleted original file: {anonymize_filename(video_path)}")
            except OSError:
                logger.exception(f"Failed to delete original file '{anonymize_filename(video_path)}'")
                # Optionally, inform the user via a status update or a specific error counter
                # For now, just logging the error as per instructions.

        # --- Update Overall Progress ---
        overall_progress_percent = (gui.processed_files / total_videos_to_process) * 100
        update_ui_safely(gui.root, lambda p=overall_progress_percent: gui.overall_progress.config(value=p))

        def update_final_status():  # Update overall status label text
            base_status = f"Progress: {gui.processed_files}/{total_videos_to_process} files"
            converted_msg = f" ({gui.successful_conversions} converted"

            # Show different skip categories
            if gui.skipped_not_worth_count > 0:
                converted_msg += f", {gui.skipped_not_worth_count} inefficient"

            if gui.skipped_low_resolution_count > 0:
                converted_msg += f", {gui.skipped_low_resolution_count} low-res"

            # Calculate other skips (files skipped for reasons other than "not worth" and low resolution)
            other_skips = skipped_files_count - gui.skipped_not_worth_count - gui.skipped_low_resolution_count
            if other_skips > 0:
                converted_msg += f", {other_skips} skipped"

            converted_msg += ")"

            # Only show errors if there are actual errors
            error_suffix = f" - {gui.error_count} errors" if gui.error_count > 0 else ""
            gui.status_label.config(text=f"{base_status}{converted_msg}{error_suffix}")

        update_ui_safely(gui.root, update_final_status)

    # --- End of Processing Loop ---
    final_status_message = "Conversion complete"
    if stop_event.is_set():
        final_status_message = "Conversion stopped by user"
    elif gui.error_count > 0:
        final_status_message = f"Conversion complete with {gui.error_count} errors"

    logger.info(f"Worker finished. Status: {final_status_message}")
    # Call the completion callback passed from the controller
    update_ui_safely(gui.root, lambda msg=final_status_message: completion_callback(gui, msg))
