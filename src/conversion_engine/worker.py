# src/conversion_engine/worker.py
"""
Contains the main worker thread function for sequential video conversion.
"""
# Standard library imports
import os
import sys
import glob
import json
import threading
import time
import logging
import shutil
import subprocess
import signal
import statistics
import traceback
from pathlib import Path
import math # For ceil
import datetime # For history timestamp
from typing import Callable # Import Callable

# GUI-related imports (for type hinting gui object, not direct use of widgets here)
import tkinter as tk

# Project imports
from src.ab_av1.exceptions import AbAv1Error, InputFileError, OutputFileError, VMAFError, EncodingError
from src.utils import (
    get_video_info, format_time, format_file_size, anonymize_filename,
    update_ui_safely, append_to_history
)
from src.config import (
    DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET
)
from src.video_conversion import process_video
# Import functions/modules from the engine package and GUI updates
from .scanner import scan_video_needs_conversion
from .callback_handlers import (
    handle_starting, handle_file_info, handle_progress, handle_error,
    handle_retrying, handle_completed, handle_skipped, handle_skipped_not_worth
)
# Import GUI update functions (needed by callback dispatcher indirectly via handlers)
from src.gui.gui_updates import (
    update_statistics_summary, reset_current_file_details, update_progress_bars,
    update_conversion_statistics, update_elapsed_time, update_total_elapsed_time
)
# REMOVED direct import causing circular dependency. Functions are passed as args.
# from src.gui.conversion_controller import conversion_complete, store_process_id


logger = logging.getLogger(__name__)


def sequential_conversion_worker(gui, input_folder, output_folder, overwrite, stop_event,
                                 convert_audio, audio_codec, delete_original: bool, # Added delete_original
                                 pid_storage_callback: Callable, # Changed to Callable
                                 completion_callback: Callable): # Changed to Callable
    """Process files sequentially, scanning and converting each eligible video.

    This is the main worker function that runs in a separate thread to handle
    the entire conversion process from scanning to processing.

    Args:
        gui: The main GUI instance (passed for accessing settings, state, and root window).
        input_folder: Root folder to scan for video files.
        output_folder: Destination folder for converted files.
        overwrite: Whether to overwrite existing output files.
        stop_event: Threading event to signal stopping.
        convert_audio: Whether to convert audio to a different codec.
        audio_codec: Target audio codec if conversion is enabled.
        delete_original: Whether to delete the original file after successful conversion.
        pid_storage_callback: Function to call to store the process ID (e.g., store_process_id(gui, pid, path)).
        completion_callback: Function to call when the worker finishes (e.g., conversion_complete(gui, message)).
    """
    gui.output_folder_path = output_folder # Store for potential cleanup use later
    logger.info(f"Worker started. Input: '{input_folder}', Output: '{output_folder}'")
    gui.error_count = 0; gui.total_input_bytes_success = 0; gui.total_output_bytes_success = 0; gui.total_time_success = 0
    gui.skipped_not_worth_count = 0  # Track files skipped because conversion isn't beneficial
    gui.skipped_not_worth_files = []  # Track filenames of skipped files
    gui.skipped_low_resolution_count = 0  # Track files skipped due to low resolution
    gui.skipped_low_resolution_files = []  # Track filenames of low resolution files
    gui.error_details = []  # Track error details for summary
    video_info_cache = {} # Initialize cache for this run

    # Determine extensions from GUI state
    extensions = [ext for ext, var in [("mp4",gui.ext_mp4),("mkv",gui.ext_mkv),("avi",gui.ext_avi),("wmv",gui.ext_wmv)] if var.get()]
    if not extensions:
        logger.error("Worker: No extensions selected.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "Error: No extensions selected"))
        return
    logger.info(f"Worker: Processing extensions: {', '.join(extensions)}")

    # --- File Scanning (Quick Count) ---
    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Searching for files..."))
    all_video_files_set = set()
    try:
        input_path_obj = Path(input_folder)
        if not input_path_obj.is_dir(): raise FileNotFoundError(f"Input folder invalid: {input_folder}")
        for ext in extensions:
            # Check both lowercase and uppercase extensions
            for pattern in [f"*.{ext.lower()}", f"*.{ext.upper()}"]:
                 for file_path in input_path_obj.rglob(pattern):
                     # Ensure it's a file, not a directory ending with the extension
                     if file_path.is_file():
                         all_video_files_set.add(str(file_path.resolve()))
    except Exception as scan_error:
        logger.error(f"Error scanning input folder '{input_folder}': {scan_error}", exc_info=True)
        update_ui_safely(gui.root, lambda err=scan_error: completion_callback(gui, f"Error scanning files: {err}"))
        return

    all_video_files = sorted(list(all_video_files_set)); total_files_found = len(all_video_files)
    logger.info(f"Found {total_files_found} potential files matching extensions.")
    if not all_video_files:
        logger.info("No matching files found in input folder.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "No matching files found"))
        return

    # --- Detailed Scan & Eligibility Check (with progress) ---
    files_to_process = []; skipped_files_count = 0
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0)) # Reset progress bar
    update_ui_safely(gui.root, lambda t=total_files_found: gui.status_label.config(text=f"Found {t} potential files. Analyzing eligibility..."))

    for i, video_path in enumerate(all_video_files):
        if stop_event.is_set():
            logger.info("Scan interrupted by user stop request.")
            update_ui_safely(gui.root, lambda: completion_callback(gui, "Scan stopped"))
            return

        # Update UI for scan progress
        scan_progress = (i + 1) / total_files_found * 100
        update_ui_safely(gui.root, lambda p=scan_progress: gui.overall_progress.config(value=p))
        update_ui_safely(gui.root, lambda i=i, t=total_files_found: gui.status_label.config(text=f"Analyzing file {i+1}/{t}..."))

        try:
            # Call the scanner function (now imported)
            needs_conversion, reason, _ = scan_video_needs_conversion(
                input_video_path=video_path,
                input_base_folder=input_folder,
                output_base_folder=output_folder,
                overwrite=overwrite,
                video_info_cache=video_info_cache
            )
            if needs_conversion:
                files_to_process.append(video_path)
            else:
                skipped_files_count += 1
                filename = os.path.basename(video_path)
                
                # Check if skipped due to resolution
                if "Below minimum resolution" in reason:
                    gui.skipped_low_resolution_count += 1
                    gui.skipped_low_resolution_files.append(filename)
                
                # Log skip reason using the handler for consistency
                handle_skipped(gui, filename, reason)
        except Exception as e:
            logger.error(f"Unexpected error during detailed scan for {anonymize_filename(video_path)}: {e}", exc_info=True)
            skipped_files_count += 1
            logger.info(f"Skipping {anonymize_filename(video_path)} due to scan error.")
            # Optionally trigger handle_error here if desired for scan errors

    gui.video_files = files_to_process # Store the list of files to process on the gui object
    gui.pending_files = files_to_process.copy()  # Create a copy for tracking remaining files
    total_videos_to_process = len(files_to_process)
    logger.info(f"Scan complete: {total_videos_to_process} files require conversion, {skipped_files_count} files skipped.")
    logger.info(f"Pending files initialized: {len(gui.pending_files)} files")

    # --- Transition to Conversion Phase ---
    if not files_to_process:
        logger.info("No files need conversion based on analysis.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "No files need conversion"))
        return

    logger.info(f"Starting conversion of {total_videos_to_process} files...")
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0)) # Reset progress bar for conversion phase
    update_ui_safely(gui.root, lambda t=total_videos_to_process: gui.status_label.config(text=f"Starting conversion of {t} files..."))

    gui.processed_files = 0; gui.successful_conversions = 0

    # --- Callback Dispatcher Definition ---
    def file_callback_dispatcher(filename, status, info=None):
        """Dispatch file status events to appropriate handler functions."""
        logging.debug(f"Callback Dispatcher: File={filename}, Status={status}, Info={info}")
        try:
            handler_map = {
                "starting": handle_starting,
                "file_info": handle_file_info,
                "progress": handle_progress,
                "warning": handle_error, # Treat warning as error for logging/counting
                "error": handle_error,
                "failed": handle_error,
                "retrying": handle_retrying,
                "completed": handle_completed,
                "skipped": handle_skipped,
                "skipped_not_worth": handle_skipped_not_worth,
            }
            handler = handler_map.get(status)
            if handler:
                # Pass the gui object and necessary arguments to the handler
                if status == "starting":
                    # handle_starting only takes gui and filename
                    handler(gui, filename)
                elif status == "skipped":
                    # handle_skipped takes gui, filename, and reason (which is passed as info)
                    handler(gui, filename, info)
                elif info is not None:
                    # Most other handlers take gui, filename, and info dict
                    handler(gui, filename, info)
                else:
                    # Fallback for handlers that might expect info but didn't receive it
                    # (e.g., if error reporting changes in future)
                    logger.warning(f"Handler for status '{status}' called without info data.")
                    handler(gui, filename, {}) # Pass empty dict to avoid crash
            else:
                logger.warning(f"Unknown status '{status}' received for file {filename}. Info: {info}")
        except Exception as e:
            logger.error(f"Error executing callback handler for status '{status}': {e}", exc_info=True)


    # --- File Processing Loop ---
    for video_path in files_to_process:
        if stop_event.is_set():
            logger.info("Conversion loop interrupted by user stop request.")
            break # Exit the loop
            
        # Store current file path for estimation
        gui.current_file_path = video_path
        logger.debug(f"Current file path set to: {video_path}")
        
        # Move file removal after processing is complete, not at the start
        # if video_path in gui.pending_files:
        #     gui.pending_files.remove(video_path)
        #     logger.debug(f"Removed {video_path} from pending files. Remaining: {len(gui.pending_files)}")

        file_number = gui.processed_files + 1
        filename = os.path.basename(video_path)
        anonymized_name = anonymize_filename(video_path)
        update_ui_safely(gui.root, lambda: gui.status_label.config(text=f"Converting {file_number}/{total_videos_to_process}: {filename}"))
        update_ui_safely(gui.root, reset_current_file_details, gui) # Reset UI elements for the new file

        original_size = 0; input_vcodec = "?"; input_acodec = "?"; input_duration = 0.0
        output_acodec = "?" # Initialize output audio codec

        # --- Use Cached Video Info ---
        video_info = video_info_cache.get(video_path)
        if not video_info:
            # This shouldn't normally happen if scan phase succeeded, but handle defensively
            logger.warning(f"Cache miss during processing phase for {anonymized_name}. Calling get_video_info again.")
            try: video_info = get_video_info(video_path) # Attempt to get info again
            except Exception as e: logger.error(f"Failed to get video info during processing for {anonymized_name}: {e}")
            # Proceed even if video_info is None, process_video might handle it

        # --- Extract Info & Update UI ---
        if video_info:
            try:
                format_info_text = "-"; size_str = "-"
                for stream in video_info.get("streams", []):
                    codec_type = stream.get("codec_type")
                    if codec_type == "video": input_vcodec = stream.get("codec_name", "?").upper()
                    elif codec_type == "audio": input_acodec = stream.get("codec_name", "?").upper()
                format_info_text = f"{input_vcodec} / {input_acodec}"
                original_size = video_info.get('file_size', 0); size_str = format_file_size(original_size)
                duration_str = video_info.get('format', {}).get('duration', '0')
                try: input_duration = float(duration_str)
                except (ValueError, TypeError): input_duration = 0.0; logger.warning(f"Invalid duration '{duration_str}' for {anonymized_name}")

                update_ui_safely(gui.root, lambda fi=format_info_text: gui.orig_format_label.config(text=fi))
                update_ui_safely(gui.root, lambda ss=size_str: gui.orig_size_label.config(text=ss))
                gui.last_input_size = original_size # Store for potential use in handlers
                output_acodec = input_acodec # Default output codec
            except Exception as e:
                logger.error(f"Error extracting details from video_info for {anonymized_name}: {e}")
                gui.last_input_size = None; input_duration = 0.0
        else:
            logger.warning(f"Cannot get pre-conversion info for {anonymized_name}.")
            gui.last_input_size = None; input_duration = 0.0

        gui.current_file_start_time = time.time(); gui.current_file_encoding_start_time = None
        if not hasattr(gui, 'elapsed_timer_id'):
            gui.elapsed_timer_id = None
        update_ui_safely(gui.root, update_elapsed_time, gui, gui.current_file_start_time) # Start timer UI updates
        gui.current_process_info = None # Reset PID info for the new file

        # --- Process Video ---
        process_successful = False; output_file_path = None; elapsed_time_file = 0; output_size = 0; final_crf = None; final_vmaf = None; final_vmaf_target = None
        try:
            # Pass the dispatcher and the specific PID callback for this file
            # Note: pid_storage_callback is the function reference passed into this worker
            result_tuple = process_video(
                video_path=video_path,
                input_folder=input_folder,
                output_folder=output_folder,
                overwrite=overwrite,
                convert_audio=convert_audio,
                audio_codec=audio_codec,
                file_info_callback=file_callback_dispatcher, # Pass the dispatcher
                pid_callback=lambda pid, path=video_path: pid_storage_callback(gui, pid, path), # Use lambda to pass gui object and path
                total_duration_seconds=input_duration
            )
            if result_tuple:
                 # Unpack potentially extended tuple including stats
                 output_file_path, elapsed_time_file, _, output_size, final_crf, final_vmaf, final_vmaf_target = result_tuple
                 process_successful = True
                 gui.last_output_size = output_size # Store for use in handle_completed
                 gui.last_elapsed_time = elapsed_time_file # Store for use in handle_completed
                 # Determine final audio codec based on conversion settings
                 if convert_audio and input_acodec.lower() not in ['aac', 'opus']: output_acodec = audio_codec.lower()
                 # else: output_acodec remains input_acodec (set earlier)
            else:
                 # If process_video returns None, it means failure was reported via callback
                 gui.last_output_size = None
                 gui.last_elapsed_time = None
                 process_successful = False # Ensure state reflects failure

        except Exception as e:
            logger.error(f"Critical error during process_video call for {anonymized_name}: {e}", exc_info=True)
            # Dispatch a generic failure if process_video itself crashes
            file_callback_dispatcher(filename, "failed", {"message": f"Internal processing error: {e}", "type": "processing_crash"})
            process_successful = False
            gui.last_output_size = None
            gui.last_elapsed_time = None


        # --- Post-processing & History ---
        gui.processed_files += 1
        
        # Remove file from pending list after processing is complete
        if video_path in gui.pending_files:
            gui.pending_files.remove(video_path)
            logger.debug(f"Removed completed file {video_path} from pending files. Remaining: {len(gui.pending_files)}")

        if process_successful:
            gui.successful_conversions += 1
            try: # Append to History
                anonymize_hist = gui.anonymize_history.get()
                input_path_for_hist = anonymize_filename(video_path) if anonymize_hist else video_path
                # Ensure output_file_path is a string before anonymizing, provide a placeholder if None
                output_file_path_str = output_file_path if output_file_path is not None else "N/A"
                output_path_for_hist = anonymize_filename(output_file_path_str) if anonymize_hist else output_file_path_str
                hist_record = {
                    "timestamp": datetime.datetime.now().isoformat(sep=' ', timespec='seconds'),
                    "input_file": input_path_for_hist, "output_file": output_path_for_hist,
                    "input_size_mb": round(original_size / (1024**2), 2) if original_size is not None else None,
                    "output_size_mb": round(output_size / (1024**2), 2) if output_size is not None else None,
                    "reduction_percent": round(100 - (output_size / original_size * 100), 1) if original_size and output_size and original_size > 0 else None,
                    "duration_sec": round(input_duration, 1) if input_duration is not None else None,
                    "time_sec": round(elapsed_time_file, 1) if elapsed_time_file is not None else None,
                    "input_vcodec": input_vcodec, "input_acodec": input_acodec, "output_acodec": output_acodec,
                    "input_codec": input_vcodec,  # Duplicate for compatibility with estimation functions
                    "final_crf": final_crf,
                    "final_vmaf": round(final_vmaf, 2) if final_vmaf is not None else None,
                    "final_vmaf_target": final_vmaf_target
                }
                append_to_history(hist_record)
            except Exception as hist_e:
                logger.error(f"Failed to append to history for {anonymized_name}: {hist_e}")
            # Dispatch completed callback *after* history attempt
            # Pass necessary info for the handler
            completed_info = {"vmaf": final_vmaf, "crf": final_crf, "output_size": output_size}
            file_callback_dispatcher(filename, "completed", completed_info)
        else:
            # Error handling is done via the callback dispatcher calling handle_error
            pass

        # --- Delete Original File if Requested ---
        if process_successful and delete_original and video_path and os.path.exists(video_path):
            try:
                logger.info(f"Deleting original file as requested: {anonymize_filename(video_path)}")
                os.remove(video_path)
                logger.info(f"Successfully deleted original file: {anonymize_filename(video_path)}")
            except OSError as e:
                logger.error(f"Failed to delete original file '{anonymize_filename(video_path)}': {e}")
                # Optionally, inform the user via a status update or a specific error counter
                # For now, just logging the error as per instructions.

        # --- Update Overall Progress ---
        overall_progress_percent = (gui.processed_files / total_videos_to_process) * 100
        update_ui_safely(gui.root, lambda p=overall_progress_percent: gui.overall_progress.config(value=p))
        def update_final_status(): # Update overall status label text
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
    if stop_event.is_set(): final_status_message = "Conversion stopped by user"
    elif gui.error_count > 0: final_status_message = f"Conversion complete with {gui.error_count} errors"

    logger.info(f"Worker finished. Status: {final_status_message}")
    # Call the completion callback passed from the controller
    update_ui_safely(gui.root, lambda msg=final_status_message: completion_callback(gui, msg))