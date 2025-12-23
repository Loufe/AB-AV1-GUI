# src/gui/callback_handlers.py
"""
Callback handler functions for events dispatched from the conversion process.

These functions typically update the GUI state or log information based on
events like progress updates, errors, or completion for a single file.
"""

import logging
from dataclasses import replace

from src.gui.gui_updates import (
    update_conversion_statistics,  # update_total_elapsed_time called by update_elapsed_time
    update_progress_bars,
    update_statistics_summary,
)
from src.models import ErrorInfo, FileInfoEvent, ProgressEvent, RetryInfo, SkippedInfo

# Project Imports
from src.utils import anonymize_filename, format_file_size, update_ui_safely

logger = logging.getLogger(__name__)


# --- Callback Handler Functions ---


def handle_starting(gui, filename) -> None:
    """Handle the start of processing for a file."""
    anonymized_file = anonymize_filename(filename)  # Anonymize for logging if needed
    logger.info(f"Starting conversion of {anonymized_file}")
    # Update UI: Set current file label and reset progress bars
    update_ui_safely(gui.root, lambda f=filename: gui.current_file_label.config(text=f"Processing: {f}"))
    update_ui_safely(gui.root, update_progress_bars, gui, 0, 0)  # Reset bars


def handle_file_info(gui, filename, info) -> None:
    """Handle initial file information updates (e.g., size)."""
    if info and "file_size_mb" in info:
        # Construct FileInfoEvent from dict for type safety
        event = FileInfoEvent(file_size_mb=info.get("file_size_mb", 0.0))
        size_str = format_file_size(int(event.file_size_mb * (1024**2)))
        # Update original size label in the UI
        update_ui_safely(gui.root, lambda ss=size_str: gui.orig_size_label.config(text=ss))
        logger.debug(f"Updated original size display for {anonymize_filename(filename)} to {size_str}")


def handle_progress(gui, filename: str, info: ProgressEvent) -> None:
    """Handle progress updates from the conversion process."""
    anonymized_file = anonymize_filename(filename)

    # Update progress bars
    update_ui_safely(gui.root, update_progress_bars, gui, info.progress_quality, info.progress_encoding)

    # Update current file message
    update_ui_safely(
        gui.root, lambda f=filename, m=info.message: gui.current_file_label.config(text=f"Processing: {f} - {m}")
    )

    # Ensure original size is available for statistics calculations
    if info.original_size is None and gui.session.last_input_size is not None:
        info = replace(info, original_size=gui.session.last_input_size)

    # Update ETA, VMAF, CRF, Size Prediction labels
    update_conversion_statistics(gui, info)  # This function handles UI updates safely

    # Force UI updates during encoding phase to maintain responsiveness
    if info.phase == "encoding":

        def force_update():
            try:
                gui.root.update_idletasks()
            except Exception:
                logger.exception("Error in forced UI update")

        update_ui_safely(gui.root, force_update)

    # Minimal logging here, more detailed progress logging is in the wrapper/parser
    logger.debug(
        f"Progress update for {anonymized_file}: Qual={info.progress_quality:.1f}%, "
        f"Enc={info.progress_encoding:.1f}%, Phase={info.phase}"
    )


def handle_error(gui, filename, error_info) -> None:
    """Handle errors that occur during file processing."""
    # Construct ErrorInfo from dict or string for type safety
    if isinstance(error_info, dict):
        error = ErrorInfo(
            message=error_info.get("message", "Unknown error"),
            error_type=error_info.get("type", "unknown"),
            details=error_info.get("details", ""),
            stack_trace=error_info.get("stack_trace"),
        )
    elif isinstance(error_info, str):
        error = ErrorInfo(message=error_info, error_type="unknown", details="")
    else:
        error = ErrorInfo(message="Unknown error", error_type="unknown", details="")

    anonymized_name = anonymize_filename(filename)
    log_msg = f"Error converting {anonymized_name} (Type: {error.error_type}): {error.message}"
    if error.details:
        log_msg += f" - Details: {error.details}"
    logger.error(log_msg)

    # Log stack trace if provided
    if error.stack_trace:
        logger.error(f"Stack Trace for {anonymized_name}:\n{error.stack_trace}")

    # Increment error count on the GUI object (thread-safe)
    def update_error_count():
        gui.session.error_count += 1

    update_ui_safely(gui.root, update_error_count)

    # Track error details for summary (thread-safe)
    def update_error_details():
        gui.session.error_details.append(
            {"filename": filename, "error_type": error.error_type, "message": error.message, "details": error.details}
        )

    update_ui_safely(gui.root, update_error_details)

    # Update overall status label in the UI to reflect the error count
    def update_status():
        total_files = len(gui.session.video_files)
        successful = gui.session.successful_conversions
        base_status = f"Progress: {gui.session.processed_files}/{total_files} files ({successful} successful)"
        gui.status_label.config(text=f"{base_status} - {gui.session.error_count} errors")

    update_ui_safely(gui.root, update_status)


def handle_retrying(gui, filename, info) -> None:
    """Handle retry attempts with fallback VMAF targets."""
    # Construct RetryInfo from dict for type safety
    if isinstance(info, dict):
        retry = RetryInfo(message=info.get("message", "Retrying"), fallback_vmaf=info.get("fallback_vmaf"))
    else:
        retry = RetryInfo(message="Retrying")

    anonymized_file = anonymize_filename(filename)
    log_details = f" (Target VMAF: {retry.fallback_vmaf})" if retry.fallback_vmaf else ""
    logger.info(f"{retry.message} for {anonymized_file}{log_details}")

    # Update UI: Show retrying message and the VMAF target being attempted
    update_ui_safely(
        gui.root, lambda f=filename, m=retry.message: gui.current_file_label.config(text=f"Processing: {f} - {m}")
    )
    if retry.fallback_vmaf:
        update_ui_safely(gui.root, lambda v=retry.fallback_vmaf: gui.vmaf_label.config(text=f"{v} (Target)"))


def handle_completed(gui, filename, info) -> None:
    """Handle successful completion of a file conversion and update stats."""
    anonymized_name = anonymize_filename(filename)
    log_msg = f"Successfully converted {anonymized_name}"

    # Extract values from info dict or fall back to gui attributes
    vmaf_value = info.get("vmaf")
    crf_value = info.get("crf")
    original_size = gui.session.last_input_size
    # Get final output size directly from info (more reliable than gui attribute)
    output_size = info.get("output_size")

    # Update VMAF stats (thread-safe)
    if vmaf_value is not None:
        try:
            vmaf_float = float(vmaf_value)
            update_ui_safely(gui.root, lambda v=vmaf_float: gui.session.vmaf_scores.append(v))
            log_msg += f" - VMAF: {vmaf_float:.1f}"
        except (ValueError, TypeError):
            logger.warning(f"Invalid VMAF value '{vmaf_value}' for stats in {anonymized_name}")

    # Update CRF stats (thread-safe)
    if crf_value is not None:
        try:
            crf_int = int(crf_value)
            update_ui_safely(gui.root, lambda c=crf_int: gui.session.crf_values.append(c))
            log_msg += f", CRF: {crf_int}"
        except (ValueError, TypeError):
            logger.warning(f"Invalid CRF value '{crf_value}' for stats in {anonymized_name}")

    # Update size reduction stats and final size label
    size_str = "-"  # Default display value
    if original_size is not None and output_size is not None and original_size > 0:
        try:
            ratio = (output_size / original_size) * 100
            size_reduction = 100.0 - ratio
            update_ui_safely(gui.root, lambda sr=size_reduction: gui.session.size_reductions.append(sr))  # Thread-safe
            log_msg += f", Size: {format_file_size(output_size)} ({ratio:.1f}% / {size_reduction:.1f}% reduction)"
            size_str = f"{format_file_size(output_size)} ({ratio:.1f}%)"
        except (TypeError, ZeroDivisionError) as e:
            logger.warning(f"Could not calculate size reduction for {anonymized_name}: {e}")
            size_str = format_file_size(output_size) if output_size is not None else "-"
            log_msg += f", Size: {size_str}"
    elif output_size is not None:
        size_str = format_file_size(output_size)
        log_msg += f", Size: {size_str}"

    # Update the final size label in UI
    update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))

    # Note: Total bytes/time stats are updated by the worker (conversion_engine/worker.py)
    # after process_video returns, since elapsed_time isn't available when this callback
    # is dispatched from within the wrapper.

    # Log completion message and update summary statistics display
    logger.info(log_msg)
    update_statistics_summary(gui)  # Update the overall avg/min/max stats display


def handle_skipped(gui, filename, reason) -> None:
    """Handle skipped files."""
    # This is primarily for logging, UI doesn't show individual skipped files typically
    logger.info(f"Skipped {anonymize_filename(filename)}: {reason}")


def handle_skipped_not_worth(gui, filename, info):
    """Handle files skipped because conversion isn't worthwhile."""
    # Construct SkippedInfo from dict for type safety
    if isinstance(info, dict):
        skipped = SkippedInfo(
            message=info.get("message", "Conversion not beneficial"),
            original_size=info.get("original_size"),
            min_vmaf_attempted=info.get("min_vmaf_attempted"),
        )
    else:
        skipped = SkippedInfo(message="Conversion not beneficial")

    anonymized_name = anonymize_filename(filename)
    log_msg = f"Skipped {anonymized_name} (not worth converting): {skipped.message}"

    if skipped.original_size:
        log_msg += f" - Original size: {format_file_size(skipped.original_size)}"

    logger.info(log_msg)

    # Store data for history recording by worker.py (thread-safe)
    def store_skip_data():
        gui.session.last_skip_reason = skipped.message
        gui.session.last_min_vmaf_attempted = skipped.min_vmaf_attempted

    update_ui_safely(gui.root, store_skip_data)

    # Update skipped count instead of error count (thread-safe)
    def update_skipped_count():
        gui.session.skipped_not_worth_count += 1

    update_ui_safely(gui.root, update_skipped_count)

    # Track filename for summary (thread-safe)
    def update_skipped_files():
        gui.session.skipped_not_worth_files.append(filename)

    update_ui_safely(gui.root, update_skipped_files)

    # Update UI to show this as a skip, not an error
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text=f"Skipped (inefficient): {filename}"))
