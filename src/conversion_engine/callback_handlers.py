# src/conversion_engine/callback_handlers.py
"""
Callback handler functions for events dispatched from the conversion process.

These functions typically update the GUI state or log information based on
events like progress updates, errors, or completion for a single file.
"""

import logging
import statistics
import time # Needed for handle_progress -> update_conversion_statistics logic

# Project Imports
from src.utils import (
    format_time, format_file_size, update_ui_safely, anonymize_filename
)
from src.gui.gui_updates import (
    update_statistics_summary, reset_current_file_details, update_progress_bars,
    update_conversion_statistics, update_elapsed_time # update_total_elapsed_time called by update_elapsed_time
)
from src.config import DEFAULT_VMAF_TARGET # Needed for reset/defaults

logger = logging.getLogger(__name__)


# --- Callback Handler Functions ---

def handle_starting(gui, filename) -> None:
    """Handle the start of processing for a file."""
    anonymized_file = anonymize_filename(filename) # Anonymize for logging if needed
    logger.info(f"Starting conversion of {anonymized_file}")
    # Update UI: Set current file label and reset progress bars
    update_ui_safely(gui.root, lambda f=filename: gui.current_file_label.config(text=f"Processing: {f}"))
    update_ui_safely(gui.root, update_progress_bars, gui, 0, 0) # Reset bars

def handle_file_info(gui, filename, info) -> None:
    """Handle initial file information updates (e.g., size)."""
    if info and 'file_size_mb' in info:
         size_str = format_file_size(info['file_size_mb'] * (1024**2))
         # Update original size label in the UI
         update_ui_safely(gui.root, lambda ss=size_str: gui.orig_size_label.config(text=ss))
         logger.debug(f"Updated original size display for {anonymize_filename(filename)} to {size_str}")

def handle_progress(gui, filename: str, info: dict) -> None:
    """Handle progress updates from the conversion process."""
    quality_prog = info.get("progress_quality", 0)
    encoding_prog = info.get("progress_encoding", 0)
    phase = info.get("phase", "crf-search")
    message = info.get("message", "")
    anonymized_file = anonymize_filename(filename)

    # Update progress bars
    update_ui_safely(gui.root, update_progress_bars, gui, quality_prog, encoding_prog)

    # Update current file message
    update_ui_safely(gui.root, lambda f=filename, m=message: gui.current_file_label.config(text=f"Processing: {f} - {m}"))

    # Ensure original size is available for statistics calculations
    if "original_size" not in info and hasattr(gui, 'last_input_size') and gui.last_input_size is not None:
        info["original_size"] = gui.last_input_size

    # Update ETA, VMAF, CRF, Size Prediction labels
    update_conversion_statistics(gui, info) # This function handles UI updates safely

    # Force UI updates during encoding phase to maintain responsiveness
    if phase == "encoding":
        def force_update():
            try: gui.root.update_idletasks()
            except Exception as e: logging.error(f"Error in forced UI update: {e}")
        update_ui_safely(gui.root, force_update)

    # Minimal logging here, more detailed progress logging is in the wrapper/parser
    logger.debug(f"Progress update for {anonymized_file}: Qual={quality_prog:.1f}%, Enc={encoding_prog:.1f}%, Phase={phase}")

def handle_error(gui, filename, error_info) -> None:
    """Handle errors that occur during file processing."""
    message="Unknown error"; error_type="unknown"; details=""
    if isinstance(error_info, dict):
        message = error_info.get("message", "Unknown error")
        error_type = error_info.get("type", "unknown")
        details = error_info.get("details", "")
    elif isinstance(error_info, str):
        message = error_info # Assume the string is the message

    anonymized_name = anonymize_filename(filename)
    log_msg = f"Error converting {anonymized_name} (Type: {error_type}): {message}"
    if details: log_msg += f" - Details: {details}"
    logger.error(log_msg)

    # Log stack trace if provided
    if isinstance(error_info, dict) and "stack_trace" in error_info:
        logger.error(f"Stack Trace for {anonymized_name}:\n{error_info['stack_trace']}")

    # Increment error count on the GUI object
    if hasattr(gui,'error_count'): gui.error_count += 1
    else: gui.error_count = 1

    # Update overall status label in the UI to reflect the error count
    def update_status():
        total_files = len(gui.video_files) if hasattr(gui,'video_files') and gui.video_files else 0
        base_status = f"Progress: {gui.processed_files}/{total_files} files ({gui.successful_conversions} successful)"
        gui.status_label.config(text=f"{base_status} - {gui.error_count} errors")
    update_ui_safely(gui.root, update_status)

def handle_retrying(gui, filename, info) -> None:
    """Handle retry attempts with fallback VMAF targets."""
    message = "Retrying"; log_details = ""; current_target_vmaf = None
    if isinstance(info, dict):
        message = info.get("message", message)
        # The key 'fallback_vmaf' now holds the VMAF target *being attempted*
        if "fallback_vmaf" in info:
            current_target_vmaf = info['fallback_vmaf']
            log_details = f" (Target VMAF: {current_target_vmaf})"

    anonymized_file = anonymize_filename(filename)
    logger.info(f"{message} for {anonymized_file}{log_details}")

    # Update UI: Show retrying message and the VMAF target being attempted
    update_ui_safely(gui.root, lambda f=filename, m=message: gui.current_file_label.config(text=f"Processing: {f} - {m}"))
    if current_target_vmaf:
        update_ui_safely(gui.root, lambda v=current_target_vmaf: gui.vmaf_label.config(text=f"{v} (Target)"))

def handle_completed(gui, filename, info) -> None:
    """Handle successful completion of a file conversion and update stats."""
    anonymized_name = anonymize_filename(filename)
    log_msg = f"Successfully converted {anonymized_name}"

    # Extract values from info dict or fall back to gui attributes
    vmaf_value = info.get("vmaf")
    crf_value = info.get("crf")
    original_size = getattr(gui, 'last_input_size', None)
    # Get final output size directly from info (more reliable than gui attribute)
    output_size = info.get("output_size")
    elapsed_time = getattr(gui, 'last_elapsed_time', None) # Get from GUI state set by worker

    # Update VMAF stats
    if vmaf_value is not None:
        try:
            vmaf_float = float(vmaf_value)
            gui.vmaf_scores.append(vmaf_float)
            log_msg += f" - VMAF: {vmaf_float:.1f}"
        except (ValueError, TypeError):
             logger.warning(f"Invalid VMAF value '{vmaf_value}' for stats in {anonymized_name}")

    # Update CRF stats
    if crf_value is not None:
        try:
            crf_int = int(crf_value)
            gui.crf_values.append(crf_int)
            log_msg += f", CRF: {crf_int}"
        except (ValueError, TypeError):
            logger.warning(f"Invalid CRF value '{crf_value}' for stats in {anonymized_name}")

    # Update size reduction stats and final size label
    size_str = "-" # Default display value
    if original_size is not None and output_size is not None and original_size > 0:
        try:
            ratio = (output_size / original_size) * 100
            size_reduction = 100.0 - ratio
            gui.size_reductions.append(size_reduction) # Add to list for overall stats
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

    # Update total bytes and time stats (used for final summary)
    if elapsed_time is not None and original_size is not None and output_size is not None:
        try:
            gui.total_input_bytes_success += original_size
            gui.total_output_bytes_success += output_size
            gui.total_time_success += elapsed_time
        except TypeError as e:
             logger.warning(f"Type error updating totals for {anonymized_name}: {e}")

    # Log completion message and update summary statistics display
    logger.info(log_msg)
    update_statistics_summary(gui) # Update the overall avg/min/max stats display

def handle_skipped(gui, filename, reason) -> None:
    """Handle skipped files."""
    # This is primarily for logging, UI doesn't show individual skipped files typically
    logger.info(f"Skipped {anonymize_filename(filename)}: {reason}")