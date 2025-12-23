# src/conversion_engine/worker.py
"""
Contains the main worker thread function for sequential video conversion.
"""

# Standard library imports
import datetime  # For history timestamp
import logging
import os
import time

# GUI-related imports (for type hinting gui object, not direct use of widgets here)
from pathlib import Path
from typing import Callable  # Import Callable

# Project imports
from src.config import DEFAULT_ENCODING_PRESET, DEFAULT_VMAF_TARGET
from src.history_index import compute_path_hash, get_history_index
from src.models import FileRecord, FileStatus, QueueConversionConfig
from src.utils import anonymize_filename, format_file_size, update_ui_safely
from src.video_conversion import calculate_output_path, process_video

# Import functions/modules from the engine package
from .scanner import scan_video_needs_conversion

logger = logging.getLogger(__name__)


def _find_video_files(folder_path: str, extensions: list[str]) -> list[str]:
    """Find all video files in a folder matching the given extensions.

    Args:
        folder_path: Path to folder to scan
        extensions: List of file extensions to match (e.g., ["mp4", "mkv"])

    Returns:
        Sorted list of absolute file paths
    """
    files = set()
    folder = Path(folder_path)
    for ext in extensions:
        for pattern in [f"*.{ext.lower()}", f"*.{ext.upper()}"]:
            for file_path in folder.rglob(pattern):
                if file_path.is_file():
                    files.add(str(file_path.resolve()))
    return sorted(files)


def sequential_conversion_worker(
    gui,
    config: QueueConversionConfig,
    stop_event,
    file_event_callback: Callable,
    queue_status_callback: Callable,
    reset_ui_callback: Callable,
    elapsed_time_callback: Callable,
    pid_storage_callback: Callable,
    completion_callback: Callable,
    get_next_item_callback: Callable | None = None,
):
    """Process queue items sequentially, converting each eligible video.

    This is the main worker function that runs in a separate thread to handle
    the entire conversion process from scanning queue items to processing files.

    Args:
        gui: The main GUI instance (passed for accessing settings, state, and root window).
        config: QueueConversionConfig containing queue items and conversion settings.
        stop_event: Threading event to signal stopping.
        file_event_callback: Callback for file conversion events (progress, errors, completion).
        queue_status_callback: Callback for updating queue tree (queue_item_id, status, processed, total).
        reset_ui_callback: Callback to reset UI details for a new file.
        elapsed_time_callback: Callback to update elapsed time display.
        pid_storage_callback: Function to call to store the process ID (e.g., store_process_id(gui, pid, path)).
        completion_callback: Function to call when the worker finishes (e.g., conversion_complete(gui, message)).
        get_next_item_callback: Callback for dynamic queue item fetching (returns tuple: item, remaining, timed_out).
    """
    logger.info(f"Worker started with {len(config.queue_items)} queue items")

    # Capture Tkinter variable value safely from main thread (thread-safety fix)
    anonymize_history_value = [None]  # Use list for mutability in closure

    def capture_anonymize_setting():
        anonymize_history_value[0] = gui.anonymize_history.get()

    update_ui_safely(gui.root, capture_anonymize_setting)
    # Wait briefly to ensure the value is captured (the after() call is async)
    time.sleep(0.01)

    # Initialize conversion state variables (thread-safe)
    def init_state():
        gui.session.error_count = 0
        gui.session.total_input_bytes_success = 0
        gui.session.total_output_bytes_success = 0
        gui.session.total_time_success = 0
        gui.session.skipped_not_worth_count = 0  # Track files skipped because conversion isn't beneficial
        gui.session.skipped_not_worth_files = []  # Track filenames of skipped files
        gui.session.skipped_low_resolution_count = 0  # Track files skipped due to low resolution
        gui.session.skipped_low_resolution_files = []  # Track filenames of low resolution files
        gui.session.error_details = []  # Track error details for summary

    update_ui_safely(gui.root, init_state)

    video_info_cache = {}  # Initialize cache for this run

    # Validate queue callback provided
    if get_next_item_callback is None:
        logger.error("Worker: No get_next_item_callback provided - dynamic queue fetch required")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "Error: Missing queue callback"))
        return

    # Determine extensions from config
    extensions = config.extensions
    if not extensions:
        logger.error("Worker: No extensions selected.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "Error: No extensions selected"))
        return
    logger.info(f"Worker: Processing extensions: {', '.join(extensions)}")

    # --- Phase 1: Count pending items (not files) ---
    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Starting queue conversion..."))

    # Count initial pending items
    items_total = len([item for item in config.queue_items if item.status == "pending"])
    items_completed = 0

    logger.info(f"Total queue items to process: {items_total}")

    if items_total == 0:
        logger.info("No pending items in queue.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "No pending items in queue"))
        return

    # Initialize overall progress tracking
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0))
    update_ui_safely(gui.root, lambda: setattr(gui.session, "processed_files", 0))
    update_ui_safely(gui.root, lambda: setattr(gui.session, "successful_conversions", 0))

    # --- Phase 2: Process queue items dynamically ---
    global_file_index = 0  # Track overall progress across all items (for file-level progress)
    retry_count = 0
    max_retries = 10

    while not stop_event.is_set():
        # Fetch next pending item dynamically
        queue_item, remaining_pending, timed_out = get_next_item_callback()

        if timed_out:
            retry_count += 1
            if retry_count > max_retries:
                logger.error(f"Failed to fetch queue item after {max_retries} retries")
                break
            logger.warning(f"Retry {retry_count}/{max_retries} fetching queue item...")
            time.sleep(0.5 * retry_count)  # Exponential backoff
            continue

        retry_count = 0  # Reset on success

        if queue_item is None:
            # No more pending items
            logger.info("No more pending queue items")
            break

        # Update items_total dynamically (remaining + 1 for current + completed)
        items_total = remaining_pending + 1 + items_completed

        logger.info(f"Processing queue item {items_completed + 1}/{items_total}: {queue_item.source_path}")
        queue_item.processed_files = 0

        # Get files for this item
        try:
            if queue_item.is_folder:
                files = _find_video_files(queue_item.source_path, extensions)
            else:
                files = [queue_item.source_path]

            queue_item.total_files = len(files)
            logger.info(f"Queue item has {len(files)} file(s) to process")
        except Exception:
            logger.exception(f"Error getting files for queue item {queue_item.id}")
            queue_item.status = "error"
            queue_item.total_files = 0
            queue_status_callback(queue_item.id, "error", queue_item.processed_files, queue_item.total_files)
            continue

        # Update queue status to converting with file count
        queue_status_callback(queue_item.id, "converting", 0, queue_item.total_files)

        # Process each file in this queue item
        for file_path in files:
            if stop_event.is_set():
                logger.info("Conversion interrupted by user stop request.")
                break

            global_file_index += 1

            # Store current file path for estimation
            update_ui_safely(gui.root, lambda vp=file_path: setattr(gui.session, "current_file_path", vp))
            logger.debug(f"Current file path set to: {file_path}")

            filename = os.path.basename(file_path)
            anonymized_name = anonymize_filename(file_path)
            # Show item-based progress with file info
            item_num = items_completed + 1
            file_num = queue_item.processed_files + 1
            file_total = queue_item.total_files
            update_ui_safely(
                gui.root,
                lambda ic=item_num, it=items_total, fp=file_num, ft=file_total, fname=filename: (
                    gui.status_label.config(text=f"Item {ic}/{it}: Converting file {fp}/{ft} - {fname}")
                ),
            )
            update_ui_safely(gui.root, reset_ui_callback)  # Reset UI elements for the new file

            # Calculate output path using the new helper
            try:
                output_path_obj, overwrite, delete_original = calculate_output_path(
                    input_path=file_path,
                    output_mode=queue_item.output_mode,
                    suffix=queue_item.output_suffix or config.default_suffix,
                    output_folder=queue_item.output_folder,
                    source_folder=queue_item.source_path if queue_item.is_folder else None,
                )
                output_path = str(output_path_obj)
            except Exception:
                logger.exception(f"Error calculating output path for {anonymized_name}")
                file_event_callback(
                    filename, "failed", {"message": "Failed to calculate output path", "type": "path_error"}
                )
                queue_item.processed_files += 1
                queue_status_callback(queue_item.id, "converting", queue_item.processed_files, queue_item.total_files)
                continue

            # Check eligibility with new scanner signature
            try:
                needs_conversion, reason, video_info = scan_video_needs_conversion(
                    input_video_path=file_path,
                    output_path=output_path,
                    overwrite=overwrite,
                    video_info_cache=video_info_cache,
                )

                if not needs_conversion:
                    # File doesn't need conversion, skip it
                    filename_skip = os.path.basename(file_path)
                    file_event_callback(filename_skip, "skipped", reason)

                    # Check if skipped due to resolution (thread-safe)
                    if "Below minimum resolution" in reason:

                        def update_low_res_skip(fn=filename_skip):
                            gui.session.skipped_low_resolution_count += 1
                            gui.session.skipped_low_resolution_files.append(fn)

                        update_ui_safely(gui.root, update_low_res_skip)

                    queue_item.processed_files += 1
                    queue_status_callback(
                        queue_item.id, "converting", queue_item.processed_files, queue_item.total_files
                    )

                    # Update overall progress (item-based)
                    total_files = queue_item.total_files
                    file_progress = queue_item.processed_files / total_files if total_files > 0 else 0
                    item_progress = items_completed + file_progress
                    progress_pct = (item_progress / items_total) * 100 if items_total > 0 else 0
                    update_ui_safely(gui.root, lambda p=progress_pct: gui.overall_progress.config(value=p))
                    continue

            except Exception:
                logger.exception(f"Error during eligibility scan for {anonymized_name}")
                file_event_callback(filename, "failed", {"message": "Scan error", "type": "scan_error"})
                queue_item.processed_files += 1
                queue_status_callback(queue_item.id, "converting", queue_item.processed_files, queue_item.total_files)
                continue

            # File needs conversion - proceed with processing
            original_size = 0
            input_vcodec = "?"
            input_acodec = "?"
            input_duration = 0.0
            input_width = None
            input_height = None
            output_acodec = "?"  # Initialize output audio codec

            # --- Extract Info & Update UI ---
            if video_info:
                try:
                    format_info_text = "-"
                    size_str = "-"
                    for stream in video_info.get("streams", []):
                        codec_type = stream.get("codec_type")
                        if codec_type == "video":
                            input_vcodec = stream.get("codec_name", "?").upper()
                            input_width = stream.get("width")
                            input_height = stream.get("height")
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
                    update_ui_safely(gui.root, lambda os=original_size: setattr(gui.session, "last_input_size", os))
                    output_acodec = input_acodec  # Default output codec
                except Exception:
                    logger.exception(f"Error extracting details from video_info for {anonymized_name}")
                    update_ui_safely(gui.root, lambda: setattr(gui.session, "last_input_size", None))
                    input_duration = 0.0
            else:
                logger.warning(f"Cannot get pre-conversion info for {anonymized_name}.")
                update_ui_safely(gui.root, lambda: setattr(gui.session, "last_input_size", None))
                input_duration = 0.0

            current_time = time.time()
            update_ui_safely(gui.root, lambda t=current_time: setattr(gui.session, "current_file_start_time", t))
            update_ui_safely(gui.root, lambda: setattr(gui.session, "current_file_encoding_start_time", None))
            update_ui_safely(
                gui.root, elapsed_time_callback, gui.session.current_file_start_time
            )  # Start timer UI updates
            update_ui_safely(gui.root, lambda: setattr(gui.session, "current_process_info", None))  # Reset PID info

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
                    video_path=file_path,
                    output_path=output_path,
                    overwrite=overwrite,
                    delete_original=delete_original,
                    convert_audio=config.convert_audio,
                    audio_codec=config.audio_codec,
                    file_info_callback=file_event_callback,
                    pid_callback=lambda pid, path=file_path: pid_storage_callback(gui, pid, path),
                    total_duration_seconds=input_duration,
                )
                if result_tuple:
                    # Unpack potentially extended tuple including stats
                    output_file_path, elapsed_time_file, _, output_size, final_crf, final_vmaf, final_vmaf_target = (
                        result_tuple
                    )
                    process_successful = True
                    update_ui_safely(gui.root, lambda os=output_size: setattr(gui.session, "last_output_size", os))
                    update_ui_safely(
                        gui.root, lambda et=elapsed_time_file: setattr(gui.session, "last_elapsed_time", et)
                    )
                    # Determine final audio codec based on conversion settings
                    if config.convert_audio and input_acodec.lower() not in ["aac", "opus"]:
                        output_acodec = config.audio_codec.lower()
                    # else: output_acodec remains input_acodec (set earlier)
                else:
                    # If process_video returns None, it means failure was reported via callback
                    update_ui_safely(gui.root, lambda: setattr(gui.session, "last_output_size", None))
                    update_ui_safely(gui.root, lambda: setattr(gui.session, "last_elapsed_time", None))
                    process_successful = False  # Ensure state reflects failure

            except Exception as e:
                logger.exception(f"Critical error during process_video call for {anonymized_name}")
                # Dispatch a generic failure if process_video itself crashes
                file_event_callback(
                    filename, "failed", {"message": f"Internal processing error: {e}", "type": "processing_crash"}
                )
                process_successful = False
                update_ui_safely(gui.root, lambda: setattr(gui.session, "last_output_size", None))
                update_ui_safely(gui.root, lambda: setattr(gui.session, "last_elapsed_time", None))

            # --- Post-processing & History ---
            update_ui_safely(gui.root, lambda: setattr(gui.session, "processed_files", gui.session.processed_files + 1))

            # Update queue item progress
            queue_item.processed_files += 1
            queue_status_callback(queue_item.id, "converting", queue_item.processed_files, queue_item.total_files)

            if process_successful:
                update_ui_safely(
                    gui.root,
                    lambda: setattr(gui.session, "successful_conversions", gui.session.successful_conversions + 1),
                )
                # Note: The "completed" callback is already dispatched by the wrapper (ab_av1/wrapper.py)
                # before returning, so we don't call it again here to avoid double-counting statistics.
                # However, we DO need to update the totals here because the wrapper's callback fires
                # before we have elapsed_time available (it's calculated after process_video returns).
                if original_size and output_size and elapsed_time_file:

                    def update_totals_from_worker(
                        inp_size=original_size, out_size=output_size, elapsed=elapsed_time_file
                    ):
                        gui.session.total_input_bytes_success += inp_size
                        gui.session.total_output_bytes_success += out_size
                        gui.session.total_time_success += elapsed

                    update_ui_safely(gui.root, update_totals_from_worker)

                try:  # Record to History Index
                    anonymize_hist = anonymize_history_value[0]
                    path_hash = compute_path_hash(file_path)
                    now = datetime.datetime.now().isoformat(sep=" ", timespec="seconds")

                    # Get file mtime for cache validation
                    try:
                        file_mtime = os.path.getmtime(file_path)
                    except OSError:
                        file_mtime = 0.0

                    # Prepare paths based on anonymization setting
                    original_path = None if anonymize_hist else file_path
                    output_path_str = None
                    if output_file_path:
                        output_path_str = anonymize_filename(output_file_path) if anonymize_hist else output_file_path

                    record = FileRecord(
                        path_hash=path_hash,
                        original_path=original_path,
                        status=FileStatus.CONVERTED,
                        file_size_bytes=original_size,
                        file_mtime=file_mtime,
                        duration_sec=round(input_duration, 1) if input_duration else None,
                        video_codec=input_vcodec.lower() if input_vcodec != "?" else None,
                        audio_codec=input_acodec.lower() if input_acodec != "?" else None,
                        width=input_width,
                        height=input_height,
                        output_path=output_path_str,
                        output_size_bytes=output_size,
                        reduction_percent=round(100 - (output_size / original_size * 100), 1)
                        if original_size and output_size and original_size > 0
                        else None,
                        conversion_time_sec=round(elapsed_time_file, 1) if elapsed_time_file else None,
                        final_crf=final_crf,
                        final_vmaf=round(final_vmaf, 2) if final_vmaf is not None else None,
                        vmaf_target_used=final_vmaf_target if final_vmaf_target is not None else 95,
                        preset_when_analyzed=DEFAULT_ENCODING_PRESET,
                        output_audio_codec=output_acodec.lower() if output_acodec != "?" else None,
                        first_seen=now,
                        last_updated=now,
                    )
                    index = get_history_index()
                    index.upsert(record)
                    index.save()
                except Exception:
                    logger.exception(f"Failed to record history for {anonymized_name}")
            # Check if this was a NOT_WORTHWHILE skip and record to history
            elif gui.session.last_skip_reason:
                try:  # Record NOT_WORTHWHILE to History Index
                    anonymize_hist = anonymize_history_value[0]
                    path_hash = compute_path_hash(file_path)
                    now = datetime.datetime.now().isoformat(sep=" ", timespec="seconds")

                    # Get file mtime for cache validation
                    try:
                        file_mtime = os.path.getmtime(file_path)
                    except OSError:
                        file_mtime = 0.0

                    # Prepare path based on anonymization setting
                    original_path = None if anonymize_hist else file_path

                    record = FileRecord(
                        path_hash=path_hash,
                        original_path=original_path,
                        status=FileStatus.NOT_WORTHWHILE,
                        file_size_bytes=original_size,
                        file_mtime=file_mtime,
                        duration_sec=round(input_duration, 1) if input_duration else None,
                        video_codec=input_vcodec.lower() if input_vcodec != "?" else None,
                        audio_codec=input_acodec.lower() if input_acodec != "?" else None,
                        width=input_width,
                        height=input_height,
                        vmaf_target_when_analyzed=DEFAULT_VMAF_TARGET,
                        preset_when_analyzed=DEFAULT_ENCODING_PRESET,
                        vmaf_target_attempted=DEFAULT_VMAF_TARGET,
                        min_vmaf_attempted=gui.session.last_min_vmaf_attempted,
                        skip_reason=gui.session.last_skip_reason,
                        first_seen=now,
                        last_updated=now,
                    )
                    index = get_history_index()
                    index.upsert(record)
                    index.save()
                    logger.info(f"Recorded NOT_WORTHWHILE status to history for {anonymized_name}")

                    # Clear the stored skip data
                    update_ui_safely(gui.root, lambda: setattr(gui.session, "last_skip_reason", None))
                    update_ui_safely(gui.root, lambda: setattr(gui.session, "last_min_vmaf_attempted", None))
                except Exception:
                    logger.exception(f"Failed to record NOT_WORTHWHILE history for {anonymized_name}")
                # Else: Error handling is done via the callback dispatcher calling handle_error

            # Note: Original file deletion (for REPLACE mode) is handled by process_video()

            # --- Update Overall Progress (item-based) ---
            total_files = queue_item.total_files
            file_progress = queue_item.processed_files / total_files if total_files > 0 else 0
            item_progress = items_completed + file_progress
            progress_pct = (item_progress / items_total) * 100 if items_total > 0 else 0
            update_ui_safely(gui.root, lambda p=progress_pct: gui.overall_progress.config(value=p))

            def update_status(ic=items_completed + 1, it=items_total):  # Capture loop variables
                base_status = f"Item {ic}/{it}"
                converted_msg = f" ({gui.session.successful_conversions} converted"

                # Show different skip categories
                if gui.session.skipped_not_worth_count > 0:
                    converted_msg += f", {gui.session.skipped_not_worth_count} inefficient"

                if gui.session.skipped_low_resolution_count > 0:
                    converted_msg += f", {gui.session.skipped_low_resolution_count} low-res"

                converted_msg += ")"

                # Only show errors if there are actual errors
                error_suffix = f" - {gui.session.error_count} errors" if gui.session.error_count > 0 else ""
                gui.status_label.config(text=f"{base_status}{converted_msg}{error_suffix}")

            update_ui_safely(gui.root, update_status)

        # Mark queue item as completed and increment counter
        queue_item.status = "completed"
        queue_status_callback(queue_item.id, "completed", queue_item.processed_files, queue_item.total_files)
        items_completed += 1

        # Update overall progress to reflect completed item
        overall_progress_percent = (items_completed / items_total) * 100 if items_total > 0 else 100
        update_ui_safely(gui.root, lambda p=overall_progress_percent: gui.overall_progress.config(value=p))

    # --- End of Processing Loop ---
    final_status_message = "Conversion complete"
    if stop_event.is_set():
        final_status_message = "Conversion stopped by user"
    elif gui.session.error_count > 0:
        final_status_message = f"Conversion complete with {gui.session.error_count} errors"

    logger.info(f"Worker finished. Status: {final_status_message}")
    # Call the completion callback passed from the controller
    update_ui_safely(gui.root, lambda msg=final_status_message: completion_callback(gui, msg))
