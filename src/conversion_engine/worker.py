# src/conversion_engine/worker.py
"""
Contains the main worker thread function for sequential video conversion.
"""

# Standard library imports
import datetime  # For history timestamp
import logging
import os
import threading
import time

# GUI-related imports (for type hinting gui object, not direct use of widgets here)
from typing import Callable  # Import Callable

# Project imports
from src.ab_av1.exceptions import ConversionNotWorthwhileError
from src.ab_av1.wrapper import AbAv1Wrapper
from src.cache_helpers import is_file_unchanged
from src.config import DEFAULT_ENCODING_PRESET, DEFAULT_VMAF_TARGET, MIN_VMAF_FALLBACK_TARGET
from src.hardware_accel import get_hw_decoder_for_codec, get_video_codec_from_info
from src.history_index import compute_filename_hash, compute_path_hash, get_history_index
from src.models import FileRecord, FileStatus, OperationType, ProgressEvent, QueueConversionConfig, QueueItemStatus
from src.privacy import anonymize_filename
from src.utils import get_video_info, update_ui_safely
from src.video_conversion import calculate_output_path, process_video
from src.video_metadata import extract_video_metadata

# Import functions/modules from the engine package
from .scanner import scan_video_needs_conversion

logger = logging.getLogger(__name__)

# THREAD SAFETY NOTE:
# This worker runs in a dedicated thread and directly mutates QueueItem attributes.
# This is safe because:
# 1. Python's GIL protects simple attribute assignments (int, str, enum)
# 2. Only ONE worker thread processes the queue sequentially
# 3. GUI thread reads these fields infrequently via callbacks scheduled with update_ui_safely()
# 4. Stale reads cause harmless UI lag (<100ms), not data corruption
# 5. The queue_status_callback is read-only and does NOT write back to queue_item
#
# DO NOT add locks here - they would serialize with Tkinter's event loop and cause deadlocks.
# DO NOT use update_ui_safely() for every mutation - 100+ callbacks per file would tank performance.


def _update_file_status(queue_item, file_index: int, status: QueueItemStatus, error_msg: str | None = None) -> None:
    """Update the status of a file within a folder queue item."""
    if queue_item.is_folder and file_index < len(queue_item.files):
        queue_item.files[file_index].status = status
        if error_msg:
            queue_item.files[file_index].error_message = error_msg
    elif queue_item.is_folder:
        logger.warning(f"File index {file_index} out of range for queue item with {len(queue_item.files)} files")


def _create_file_record(
    file_path: str,
    anonymize_history: bool | None,
    status: FileStatus,
    original_size: int,
    input_duration: float | None,
    input_vcodec: str,
    input_width: int | None,
    input_height: int | None,
    # Metadata fields
    bitrate_kbps: float | None = None,
    audio_streams: list | None = None,
    # Status-specific optional fields
    output_path: str | None = None,
    output_size: int | None = None,
    crf_search_time_sec: float | None = None,
    encoding_time_sec: float | None = None,
    final_crf: int | None = None,
    final_vmaf: float | None = None,
    vmaf_target: int | None = None,
    output_acodec: str | None = None,
    predicted_output_size: int | None = None,
    predicted_size_reduction: float | None = None,
    vmaf_target_attempted: int | None = None,
    min_vmaf_attempted: int | None = None,
    skip_reason: str | None = None,
) -> FileRecord:
    """Create a FileRecord with common setup and status-specific fields.

    Handles path hashing, timestamp generation, mtime extraction, and anonymization.
    Populates common fields and accepts status-specific fields as parameters.
    """
    # Common setup
    path_hash = compute_path_hash(file_path)
    now = datetime.datetime.now().isoformat(sep=" ", timespec="seconds")

    try:
        file_mtime = os.path.getmtime(file_path)
    except OSError:
        file_mtime = 0.0

    # Handle path anonymization (treat None as False)
    should_anonymize = anonymize_history or False
    original_path_for_record = None if should_anonymize else file_path
    output_path_str = None
    if output_path:
        output_path_str = anonymize_filename(output_path) if should_anonymize else output_path

    # Calculate reduction percent for CONVERTED status
    reduction_pct = None
    if status == FileStatus.CONVERTED and original_size and output_size and original_size > 0:
        reduction_pct = round(100 - (output_size / original_size * 100), 1)

    return FileRecord(
        path_hash=path_hash,
        original_path=original_path_for_record,
        status=status,
        filename_hash=compute_filename_hash(file_path),  # ADR-001 duplicate detection
        file_size_bytes=original_size,
        file_mtime=file_mtime,
        duration_sec=round(input_duration, 1) if input_duration else None,
        video_codec=input_vcodec.lower() if input_vcodec != "?" else None,
        width=input_width,
        height=input_height,
        bitrate_kbps=bitrate_kbps,
        audio_streams=audio_streams or [],
        # SCANNED/ANALYZED status fields (Layer 1 and Layer 2 analysis)
        vmaf_target_when_analyzed=vmaf_target
        if status in (FileStatus.SCANNED, FileStatus.ANALYZED)
        else (DEFAULT_VMAF_TARGET if status == FileStatus.NOT_WORTHWHILE else None),
        preset_when_analyzed=DEFAULT_ENCODING_PRESET,
        best_crf=final_crf if status in (FileStatus.SCANNED, FileStatus.ANALYZED) else None,
        best_vmaf_achieved=round(final_vmaf, 2)
        if final_vmaf is not None and status in (FileStatus.SCANNED, FileStatus.ANALYZED)
        else None,
        predicted_output_size=predicted_output_size,
        predicted_size_reduction=round(predicted_size_reduction, 1) if predicted_size_reduction is not None else None,
        # NOT_WORTHWHILE status fields
        vmaf_target_attempted=vmaf_target_attempted,
        min_vmaf_attempted=min_vmaf_attempted,
        skip_reason=skip_reason,
        # CONVERTED status fields
        output_path=output_path_str,
        output_size_bytes=output_size,
        reduction_percent=reduction_pct,
        crf_search_time_sec=round(crf_search_time_sec, 1) if crf_search_time_sec else None,
        encoding_time_sec=round(encoding_time_sec, 1) if encoding_time_sec else None,
        final_crf=final_crf if status == FileStatus.CONVERTED else None,
        final_vmaf=round(final_vmaf, 2) if final_vmaf is not None and status == FileStatus.CONVERTED else None,
        vmaf_target_used=vmaf_target if status == FileStatus.CONVERTED else None,
        output_audio_codec=output_acodec.lower() if output_acodec and output_acodec != "?" else None,
        # Timestamps
        first_seen=now,
        last_updated=now,
    )


def _save_file_record(record: FileRecord) -> None:
    """Save a FileRecord to the history index."""
    index = get_history_index()
    index.upsert(record)
    index.save()


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

    # Capture Tkinter variable values safely from main thread (thread-safety fix)
    anonymize_history_value = [None]  # Use list for mutability in closure
    hw_decode_enabled_value = [None]  # Capture hardware decode setting
    capture_complete = threading.Event()

    def capture_settings():
        anonymize_history_value[0] = gui.anonymize_history.get()
        hw_decode_enabled_value[0] = gui.hw_decode_enabled.get()
        capture_complete.set()

    update_ui_safely(gui.root, capture_settings)
    capture_complete.wait(timeout=1.0)  # Wait for main thread to capture settings

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
        gui.session.stopped_count = 0  # Track files skipped due to user stop request
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

    # --- Phase 1: Count pending items and total files ---
    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Processing queue..."))

    # Count pending items and total files across all pending queue items
    pending_items = [item for item in config.queue_items if item.status == QueueItemStatus.PENDING]
    items_total = len(pending_items)
    total_files_in_queue = sum(
        len(item.files) if item.is_folder else 1
        for item in pending_items
    )
    items_completed = 0

    logger.info(f"Total queue items to process: {items_total} ({total_files_in_queue} files)")

    if items_total == 0:
        logger.info("No pending items in queue.")
        update_ui_safely(gui.root, lambda: completion_callback(gui, "No pending items in queue"))
        return

    # Initialize overall progress tracking
    update_ui_safely(gui.root, lambda: setattr(gui.session, "processed_files", 0))
    update_ui_safely(gui.root, lambda: setattr(gui.session, "successful_conversions", 0))

    # --- Phase 2: Process queue items dynamically ---
    global_file_index = 0  # Track overall file progress across all queue items
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
        # Reset outcome counters for fresh processing
        queue_item.files_succeeded = 0
        queue_item.files_skipped = 0
        queue_item.files_failed = 0

        # Get files for this item
        # For folders, use the pre-filtered files list from queue_item.files
        # (populated by create_queue_item with filter_file_for_queue applied)
        try:
            files = [f.path for f in queue_item.files] if queue_item.is_folder else [queue_item.source_path]
            queue_item.total_files = len(files)
            logger.info(f"Queue item has {len(files)} file(s) to process")
        except Exception as e:
            logger.exception(f"Error getting files for queue item {queue_item.id}")
            queue_item.status = QueueItemStatus.ERROR
            queue_item.total_files = 0
            queue_item.last_error = f"Error getting files: {e!s}"
            queue_status_callback(
                queue_item.id, QueueItemStatus.ERROR, queue_item.processed_files, queue_item.total_files
            )
            continue

        # Handle empty folders
        if queue_item.total_files == 0:
            logger.info(f"Queue item {queue_item.source_path} has no eligible video files")
            queue_item.status = QueueItemStatus.COMPLETED
            queue_status_callback(queue_item.id, QueueItemStatus.COMPLETED, 0, 0)
            items_completed += 1
            continue

        # Update queue status to converting with file count
        queue_status_callback(queue_item.id, QueueItemStatus.CONVERTING, 0, queue_item.total_files)

        # Process each file in this queue item
        for file_index, file_path in enumerate(files):
            if stop_event.is_set():
                logger.info("Conversion interrupted by user stop request.")
                # Mark remaining files as stopped and track count
                remaining_files = queue_item.files[file_index:]
                stopped_file_count = len(remaining_files)
                for remaining_file in remaining_files:
                    remaining_file.status = QueueItemStatus.STOPPED
                def increment_stopped_count(n=stopped_file_count):
                    gui.session.stopped_count += n

                update_ui_safely(gui.root, increment_stopped_count)
                break

            global_file_index += 1

            # Update current file index and file status
            queue_item.current_file_index = file_index
            _update_file_status(queue_item, file_index, QueueItemStatus.CONVERTING)

            # Store current file path for estimation and callback handlers
            # Set synchronously - thread-safe per single-writer model (see worker.py:34-44)
            gui.session.current_file_path = file_path
            logger.debug(f"Current file path set to: {file_path}")

            filename = os.path.basename(file_path)
            anonymized_name = anonymize_filename(file_path)
            # Show overall file progress (filename shown separately in current_file_label)
            update_ui_safely(
                gui.root,
                lambda idx=global_file_index, total=total_files_in_queue: (
                    gui.status_label.config(text=f"File {idx}/{total}")
                ),
            )
            update_ui_safely(gui.root, reset_ui_callback)  # Reset UI elements for the new file

            # Calculate output path using the new helper (skip for ANALYZE operations)
            output_path = None
            overwrite = False
            delete_original = False
            if queue_item.operation_type == OperationType.CONVERT:
                try:
                    output_path_obj, overwrite, delete_original = calculate_output_path(
                        input_path=file_path,
                        output_mode=queue_item.output_mode,
                        suffix=queue_item.output_suffix or config.default_suffix,
                        output_folder=queue_item.output_folder,
                        source_folder=queue_item.source_path if queue_item.is_folder else None,
                    )
                    output_path = str(output_path_obj)
                except Exception as e:
                    logger.exception(f"Error calculating output path for {anonymized_name}")
                    error_msg = f"Failed to calculate output path: {e!s}"
                    file_event_callback(filename, "failed", {"message": error_msg, "type": "path_error"})
                    queue_item.processed_files += 1
                    queue_item.files_failed += 1
                    queue_item.last_error = error_msg
                    _update_file_status(queue_item, file_index, QueueItemStatus.ERROR, error_msg)
                    queue_status_callback(
                        queue_item.id, QueueItemStatus.CONVERTING, queue_item.processed_files, queue_item.total_files
                    )
                    continue

            # Check eligibility with new scanner signature (skip for ANALYZE operations)
            if queue_item.operation_type == OperationType.CONVERT and output_path is not None:
                try:
                    needs_conversion, reason, video_info = scan_video_needs_conversion(
                        input_video_path=file_path,
                        output_path=output_path,
                        overwrite=overwrite,
                        video_info_cache=video_info_cache,
                    )
                except Exception as e:
                    logger.exception(f"Error during eligibility scan for {anonymized_name}")
                    error_msg = f"Scan error: {e!s}"
                    file_event_callback(filename, "failed", {"message": error_msg, "type": "scan_error"})
                    queue_item.processed_files += 1
                    queue_item.files_failed += 1
                    queue_item.last_error = error_msg
                    _update_file_status(queue_item, file_index, QueueItemStatus.ERROR, error_msg)
                    queue_status_callback(
                        queue_item.id, QueueItemStatus.CONVERTING, queue_item.processed_files, queue_item.total_files
                    )
                    continue
            else:
                # ANALYZE operation - always "needs conversion" (really means "needs analysis")
                needs_conversion = True
                reason = ""

                try:
                    video_info = video_info_cache.get(file_path) or get_video_info(file_path)
                    if video_info and file_path not in video_info_cache:
                        video_info_cache[file_path] = video_info
                except Exception as e:
                    logger.exception(f"Error getting video info for ANALYZE operation: {anonymized_name}")
                    error_msg = f"Video info error: {e!s}"
                    file_event_callback(filename, "failed", {"message": error_msg, "type": "video_info_error"})
                    queue_item.processed_files += 1
                    queue_item.files_failed += 1
                    queue_item.last_error = error_msg
                    _update_file_status(queue_item, file_index, QueueItemStatus.ERROR, error_msg)
                    queue_status_callback(
                        queue_item.id, QueueItemStatus.CONVERTING, queue_item.processed_files, queue_item.total_files
                    )
                    continue

            if queue_item.operation_type == OperationType.CONVERT and not needs_conversion:
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
                queue_item.files_skipped += 1
                _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)
                queue_status_callback(
                    queue_item.id, QueueItemStatus.CONVERTING, queue_item.processed_files, queue_item.total_files
                )
                # Update analysis tree - "done" for already-converted, "skip" for others
                reason_lower = reason.lower() if reason else ""
                tree_status = "done" if "already converted" in reason_lower else "skip"
                update_ui_safely(
                    gui.root,
                    lambda fp=file_path, s=tree_status: gui.update_analysis_tree_for_completed_file(fp, s)
                )
                continue

            # File needs conversion - proceed with processing
            original_size = 0
            input_vcodec = "?"
            input_acodec = "?"
            input_duration = 0.0
            input_width = None
            input_height = None
            input_bitrate_kbps = None
            input_audio_streams = []
            output_acodec = "?"  # Initialize output audio codec

            # --- Extract Info & Update UI ---
            if video_info:
                try:
                    meta = extract_video_metadata(video_info)
                    input_vcodec = (meta.video_codec or "?").upper()
                    # Get audio codec from first stream (for display purposes)
                    input_acodec = (meta.audio_streams[0].codec if meta.audio_streams else "?").upper()
                    input_width = meta.width
                    input_height = meta.height
                    input_duration = meta.duration_sec or 0.0
                    original_size = meta.file_size_bytes or 0
                    input_bitrate_kbps = meta.bitrate_kbps
                    input_audio_streams = meta.audio_streams

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

            # --- Determine Hardware Decoder ---
            hw_decoder = None
            if hw_decode_enabled_value[0] and video_info:
                source_codec = get_video_codec_from_info(video_info)
                if source_codec:
                    hw_decoder = get_hw_decoder_for_codec(source_codec)
                    if hw_decoder:
                        logger.info(f"Using hardware decoder {hw_decoder} for {source_codec}")
                    else:
                        logger.debug(f"No hardware decoder available for {source_codec}")

            # --- Process Video ---
            process_successful = False
            output_file_path = None
            elapsed_time_file = 0
            output_size = 0
            final_crf = None
            final_vmaf = None
            final_vmaf_target = None

            try:
                if queue_item.operation_type == OperationType.ANALYZE:
                    # ANALYZE operation: Run CRF search only, no encoding

                    # Check for cached status that should skip analysis
                    index = get_history_index()
                    cached_record = index.lookup_file(file_path)
                    if cached_record:
                        # Skip CONVERTED files - no point analyzing, conversion history is valuable
                        if cached_record.status == FileStatus.CONVERTED:
                            if is_file_unchanged(cached_record, file_path):
                                logger.info(f"Skipping {anonymized_name} - already converted")
                                file_event_callback(filename, "skipped", "Already converted")
                                queue_item.files_skipped += 1
                                _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)
                                queue_item.processed_files += 1
                                queue_status_callback(
                                    queue_item.id,
                                    QueueItemStatus.CONVERTING,
                                    queue_item.processed_files,
                                    queue_item.total_files,
                                )
                                continue  # Skip to next file
                            # File changed since conversion - needs re-conversion, not just analysis
                            logger.info(f"File {anonymized_name} changed since conversion, re-analyzing")

                        # Skip NOT_WORTHWHILE files - already determined conversion isn't beneficial
                        elif cached_record.status == FileStatus.NOT_WORTHWHILE:
                            if is_file_unchanged(cached_record, file_path):
                                logger.info(f"Skipping {anonymized_name} - previously marked not worthwhile")
                                file_event_callback(
                                    filename, "skipped", cached_record.skip_reason or "Previously marked not worthwhile"
                                )
                                queue_item.files_skipped += 1
                                _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)
                                queue_item.processed_files += 1
                                queue_status_callback(
                                    queue_item.id,
                                    QueueItemStatus.CONVERTING,
                                    queue_item.processed_files,
                                    queue_item.total_files,
                                )
                                continue  # Skip to next file
                            logger.info(f"File {anonymized_name} changed since NOT_WORTHWHILE analysis, re-analyzing")

                        # Skip ANALYZED files - already have Layer 2 data (CRF search complete)
                        elif cached_record.status == FileStatus.ANALYZED:
                            if is_file_unchanged(cached_record, file_path):
                                logger.info(f"Skipping {anonymized_name} - already analyzed")
                                file_event_callback(filename, "skipped", "Already analyzed")
                                queue_item.files_skipped += 1
                                _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)
                                queue_item.processed_files += 1
                                queue_status_callback(
                                    queue_item.id,
                                    QueueItemStatus.CONVERTING,
                                    queue_item.processed_files,
                                    queue_item.total_files,
                                )
                                continue  # Skip to next file
                            logger.info(f"File {anonymized_name} changed since analysis, re-analyzing")

                    logger.info(f"Running CRF search (analysis only) for {anonymized_name}")
                    wrapper = AbAv1Wrapper()
                    crf_search_start = time.time()  # Track timing for both success and NOT_WORTHWHILE

                    def progress_cb(progress_pct, message, fname=filename):
                        # Report progress as quality detection progress
                        event = ProgressEvent(
                            progress_quality=progress_pct, progress_encoding=0.0, phase="crf-search", message=message
                        )
                        file_event_callback(fname, "progress", event)

                    try:
                        crf_result = wrapper.crf_search(
                            input_path=file_path,
                            vmaf_target=DEFAULT_VMAF_TARGET,
                            preset=DEFAULT_ENCODING_PRESET,
                            progress_callback=progress_cb,
                            stop_event=stop_event,
                            hw_decoder=hw_decoder,
                        )

                        # Extract results from crf_search
                        final_crf = crf_result.get("best_crf")
                        final_vmaf = crf_result.get("best_vmaf")
                        final_vmaf_target = crf_result.get("vmaf_target_used")
                        predicted_output_size = crf_result.get("predicted_output_size")
                        predicted_size_reduction = crf_result.get("predicted_size_reduction")
                        crf_search_time = crf_result.get("crf_search_time_sec")

                        # Update history index with Layer 2 data
                        record = _create_file_record(
                            file_path,
                            anonymize_history_value[0],
                            FileStatus.ANALYZED,
                            original_size,
                            input_duration,
                            input_vcodec,
                            input_width,
                            input_height,
                            bitrate_kbps=input_bitrate_kbps,
                            audio_streams=input_audio_streams,
                            crf_search_time_sec=crf_search_time,
                            final_crf=final_crf,
                            final_vmaf=final_vmaf,
                            vmaf_target=final_vmaf_target,
                            predicted_output_size=predicted_output_size,
                            predicted_size_reduction=predicted_size_reduction,
                        )
                        _save_file_record(record)

                        # Report completion via callback
                        file_event_callback(
                            filename,
                            "completed",
                            {
                                "message": f"Analysis complete (CRF {final_crf}, VMAF {final_vmaf:.2f})",
                                "crf": final_crf,
                                "vmaf": final_vmaf,
                                "vmaf_target_used": final_vmaf_target,
                            },
                        )
                        process_successful = True
                        queue_item.files_succeeded += 1
                        _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)
                        logger.info(f"Analysis complete for {anonymized_name}: CRF {final_crf}, VMAF {final_vmaf:.2f}")

                        # Update analysis tree now that history is saved
                        update_ui_safely(
                            gui.root,
                            lambda fp=file_path: gui.update_analysis_tree_for_completed_file(fp, "done")
                        )

                    except ConversionNotWorthwhileError as e:
                        # CRF search failed at all VMAF targets - record as NOT_WORTHWHILE
                        crf_search_elapsed = time.time() - crf_search_start
                        logger.warning(f"Analysis showed conversion not worthwhile for {anonymized_name}: {e}")

                        # Record NOT_WORTHWHILE to history BEFORE callback (so folder aggregates are correct)
                        record = _create_file_record(
                            file_path,
                            anonymize_history_value[0],
                            FileStatus.NOT_WORTHWHILE,
                            original_size,
                            input_duration,
                            input_vcodec,
                            input_width,
                            input_height,
                            bitrate_kbps=input_bitrate_kbps,
                            audio_streams=input_audio_streams,
                            crf_search_time_sec=crf_search_elapsed,
                            vmaf_target_attempted=DEFAULT_VMAF_TARGET,
                            min_vmaf_attempted=MIN_VMAF_FALLBACK_TARGET,
                            skip_reason=str(e),
                        )
                        _save_file_record(record)

                        file_event_callback(
                            filename,
                            "skipped_not_worth",
                            {
                                "message": str(e),
                                "original_size": original_size,
                                "min_vmaf_attempted": MIN_VMAF_FALLBACK_TARGET,
                            },
                        )
                        queue_item.files_skipped += 1
                        _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)

                        # Update analysis tree now that history is saved
                        update_ui_safely(
                            gui.root,
                            lambda fp=file_path: gui.update_analysis_tree_for_completed_file(fp, "skip")
                        )

                        process_successful = False

                elif output_path is not None:
                    # CONVERT operation: Full conversion with process_video
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
                        hw_decoder=hw_decoder,
                    )
                    if result_tuple:
                        # Unpack tuple including timing breakdown
                        (
                            output_file_path,
                            elapsed_time_file,
                            _,
                            output_size,
                            final_crf,
                            final_vmaf,
                            final_vmaf_target,
                            crf_search_time_file,
                            encoding_time_file,
                        ) = result_tuple
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
                logger.exception(f"Critical error during processing for {anonymized_name}")
                # Dispatch a generic failure
                error_msg = f"Internal processing error: {e!s}"
                file_event_callback(filename, "failed", {"message": error_msg, "type": "processing_crash"})
                process_successful = False
                queue_item.files_failed += 1
                queue_item.last_error = error_msg
                _update_file_status(queue_item, file_index, QueueItemStatus.ERROR, error_msg)
                update_ui_safely(gui.root, lambda: setattr(gui.session, "last_output_size", None))
                update_ui_safely(gui.root, lambda: setattr(gui.session, "last_elapsed_time", None))

            # --- Post-processing & History ---
            update_ui_safely(gui.root, lambda: setattr(gui.session, "processed_files", gui.session.processed_files + 1))

            # Update queue item progress
            queue_item.processed_files += 1
            queue_status_callback(
                queue_item.id, QueueItemStatus.CONVERTING, queue_item.processed_files, queue_item.total_files
            )

            if process_successful:
                update_ui_safely(
                    gui.root,
                    lambda: setattr(gui.session, "successful_conversions", gui.session.successful_conversions + 1),
                )
                # Track successful file outcome
                if queue_item.operation_type == OperationType.CONVERT:
                    queue_item.files_succeeded += 1
                    _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)
                # Note: For ANALYZE operations, files_succeeded is incremented earlier
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

                # Only save CONVERTED record for CONVERT operations
                # (ANALYZE operations save their ANALYZED record earlier in the flow)
                if queue_item.operation_type == OperationType.CONVERT:
                    try:  # Record to History Index
                        record = _create_file_record(
                            file_path,
                            anonymize_history_value[0],
                            FileStatus.CONVERTED,
                            original_size,
                            input_duration,
                            input_vcodec,
                            input_width,
                            input_height,
                            bitrate_kbps=input_bitrate_kbps,
                            audio_streams=input_audio_streams,
                            output_path=output_file_path,
                            output_size=output_size,
                            crf_search_time_sec=crf_search_time_file,
                            encoding_time_sec=encoding_time_file,
                            final_crf=final_crf,
                            final_vmaf=final_vmaf,
                            vmaf_target=final_vmaf_target if final_vmaf_target is not None else DEFAULT_VMAF_TARGET,
                            output_acodec=output_acodec,
                        )
                        _save_file_record(record)
                        # Update analysis tree now that history is saved
                        update_ui_safely(
                            gui.root,
                            lambda fp=file_path: gui.update_analysis_tree_for_completed_file(fp, "done")
                        )
                    except Exception:
                        logger.exception(f"Failed to record history for {anonymized_name}")
            # Check if this was a NOT_WORTHWHILE skip and record to history
            elif gui.session.last_skip_reason:
                queue_item.files_skipped += 1
                _update_file_status(queue_item, file_index, QueueItemStatus.COMPLETED)

                # Capture skip data and timing before clearing
                skip_reason = gui.session.last_skip_reason
                min_vmaf_attempted = gui.session.last_min_vmaf_attempted
                # Calculate elapsed time for CRF search attempt (set at line 477)
                file_start = gui.session.current_file_start_time
                crf_search_elapsed = time.time() - file_start if file_start else None

                # Clear skip state (safe to do synchronously per THREAD SAFETY NOTE)
                gui.session.last_skip_reason = None
                gui.session.last_min_vmaf_attempted = None

                try:  # Record NOT_WORTHWHILE to History Index
                    record = _create_file_record(
                        file_path,
                        anonymize_history_value[0],
                        FileStatus.NOT_WORTHWHILE,
                        original_size,
                        input_duration,
                        input_vcodec,
                        input_width,
                        input_height,
                        bitrate_kbps=input_bitrate_kbps,
                        audio_streams=input_audio_streams,
                        crf_search_time_sec=crf_search_elapsed,
                        vmaf_target_attempted=DEFAULT_VMAF_TARGET,
                        min_vmaf_attempted=min_vmaf_attempted,
                        skip_reason=skip_reason,
                    )
                    _save_file_record(record)
                    logger.info(f"Recorded NOT_WORTHWHILE status to history for {anonymized_name}")
                    # Update analysis tree now that history is saved
                    update_ui_safely(
                        gui.root,
                        lambda fp=file_path: gui.update_analysis_tree_for_completed_file(fp, "skip")
                    )
                except Exception:
                    logger.exception(f"Failed to record NOT_WORTHWHILE history for {anonymized_name}")
            else:
                # process_video returned None due to error (not a NOT_WORTHWHILE skip)
                # The error was already reported via callback, but we need to track it in queue item
                queue_item.files_failed += 1
                error_msg = queue_item.last_error or "Processing failed (see logs for details)"
                if not queue_item.last_error:
                    queue_item.last_error = error_msg
                _update_file_status(queue_item, file_index, QueueItemStatus.ERROR, error_msg)

            # Note: Original file deletion (for REPLACE mode) is handled by process_video()

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

        # Mark queue item as completed or stopped and increment counter
        if stop_event.is_set() and queue_item.processed_files < queue_item.total_files:
            # Stopped before completing all files
            queue_item.status = QueueItemStatus.STOPPED
            queue_status_callback(
                queue_item.id, QueueItemStatus.STOPPED, queue_item.processed_files, queue_item.total_files
            )
        else:
            # Completed all files (or stopped after completing all files)
            queue_item.status = QueueItemStatus.COMPLETED
            queue_status_callback(
                queue_item.id, QueueItemStatus.COMPLETED, queue_item.processed_files, queue_item.total_files
            )
        items_completed += 1

    # --- End of Processing Loop ---
    final_status_message = "Queue complete"
    if stop_event.is_set():
        final_status_message = "Queue stopped by user"
    elif gui.session.error_count > 0:
        final_status_message = f"Queue complete with {gui.session.error_count} errors"

    logger.info(f"Worker finished. Status: {final_status_message}")
    # Call the completion callback passed from the controller
    update_ui_safely(gui.root, lambda msg=final_status_message: completion_callback(gui, msg))
