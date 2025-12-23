# src/gui/conversion_controller.py
"""
Conversion control logic (start, stop, force-stop), state management,
and GUI interaction layer for the conversion process.
"""

# Standard library imports
import logging
import os
import signal
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

# GUI-related imports
from tkinter import messagebox

# Project imports
# Import from specific ab_av1 submodules
# Import constants from config
from src.config import MIN_RESOLUTION_HEIGHT, MIN_RESOLUTION_WIDTH
from src.conversion_engine.cleanup import schedule_temp_folder_cleanup  # Import cleaner scheduling

# Import from the new conversion_engine package
from src.conversion_engine.worker import sequential_conversion_worker

# Import callback handlers
from src.gui.callback_handlers import (
    handle_completed,
    handle_error,
    handle_file_info,
    handle_progress,
    handle_retrying,
    handle_skipped,
    handle_skipped_not_worth,
    handle_starting,
)

# Import GUI actions needed here (like check_ffmpeg)
from src.gui.gui_actions import check_ffmpeg

# Import GUI update functions
from src.gui.gui_updates import (
    reset_current_file_details,
    update_elapsed_time,
    update_total_elapsed_time,
    update_total_remaining_time,
)
from src.models import OutputMode, QueueConversionConfig

# Import from utils
from src.utils import (
    allow_sleep_mode,
    anonymize_filename,
    format_file_size,
    format_time,
    get_windows_subprocess_startupinfo,
    prevent_sleep_mode,
    update_ui_safely,
)

# Import the single-file processing function

logger = logging.getLogger(__name__)


# --- Callback Dispatcher ---


def create_file_callback_dispatcher(gui):
    """Create a callback dispatcher for file conversion events.

    This dispatcher routes file status events to appropriate handler functions.
    Defined in GUI layer to avoid conversion_engine importing from gui.

    Args:
        gui: The main GUI instance

    Returns:
        A callback function that can be passed to the worker
    """

    def file_callback_dispatcher(filename, status, info=None):
        """Dispatch file status events to appropriate handler functions."""
        logger.debug(f"Callback Dispatcher: File={filename}, Status={status}, Info={info}")
        try:
            handler_map = {
                "starting": handle_starting,
                "starting_no_size": handle_starting,
                "file_info": handle_file_info,
                "progress": handle_progress,
                "warning": handle_error,
                "error": handle_error,
                "failed": handle_error,
                "retrying": handle_retrying,
                "completed": handle_completed,
                "skipped": handle_skipped,
                "skipped_not_worth": handle_skipped_not_worth,
            }
            handler = handler_map.get(status)
            if handler:
                if status in ("starting", "starting_no_size"):
                    handler(gui, filename)
                elif status == "skipped" or info is not None:
                    handler(gui, filename, info)
                else:
                    logger.warning(f"Handler for status '{status}' called without info data.")
                    handler(gui, filename, {})
            else:
                logger.warning(f"Unknown status '{status}' received for file {filename}. Info: {info}")
        except Exception:
            logger.exception(f"Error executing callback handler for status '{status}'")

    return file_callback_dispatcher


def create_queue_status_callback(gui):
    """Create callback for updating queue item status in UI.

    Args:
        gui: The main GUI instance

    Returns:
        A callback function that updates queue tree rows
    """

    def queue_status_callback(queue_item_id: str, status: str, processed: int, total: int):
        """Update queue tree with item status."""

        def update():
            # Update queue item data
            for item in gui.get_queue_items():
                if item.id == queue_item_id:
                    item.status = status
                    item.processed_files = processed
                    item.total_files = total
                    break

            # Update tree row if it exists
            tree_id = gui.get_queue_tree_id(queue_item_id)
            if tree_id:
                try:
                    progress_display = f"{processed}/{total}" if total > 0 else "—"
                    gui.queue_tree.set(tree_id, "status", status.capitalize())
                    gui.queue_tree.set(tree_id, "progress", progress_display)
                except Exception:
                    logger.debug(f"Could not update tree row for {queue_item_id}")

        update_ui_safely(gui.root, update)

    return queue_status_callback


def create_get_next_pending_item_callback(gui):
    """Create callback for dynamic queue item fetching.

    Args:
        gui: The main GUI instance

    Returns:
        Callback returning (QueueItem or None, remaining_pending_count, timed_out)
    """

    def get_next_pending_item():
        """Fetch next pending queue item dynamically.

        Returns:
            tuple: (QueueItem or None, remaining_pending_count, timed_out)
        """
        done_event = threading.Event()
        result: list[tuple] = [(None, 0)]

        def fetch_on_main_thread():
            try:
                pending_count = 0
                claimed_item = None

                for item in gui.get_queue_items():
                    if item.status == "pending":
                        if claimed_item is None:
                            claimed_item = item
                        else:
                            pending_count += 1

                # Only set status AFTER successful iteration
                if claimed_item is not None:
                    claimed_item.status = "converting"

                result[0] = (claimed_item, pending_count)
            except Exception:
                logger.exception("Error fetching next pending item")
                result[0] = (None, 0)
            finally:
                done_event.set()

        update_ui_safely(gui.root, fetch_on_main_thread)

        if not done_event.wait(timeout=5.0):
            logger.warning("Timeout fetching next pending item - will retry")
            return (None, 0, True)  # timed_out = True

        return (*result[0], False)  # timed_out = False

    return get_next_pending_item


# --- Process Management & State ---


def store_process_id(gui, pid: int, input_path: str) -> None:
    """Store the current process ID and its associated input file in session state.

    Args:
        gui: The main GUI instance with session state.
        pid: Process ID of the conversion process.
        input_path: Path to the input file being processed.
    """
    gui.session.current_process_info = {"pid": pid, "input_path": input_path}
    logger.info(f"ab-av1 process started with PID: {pid} for file {anonymize_filename(input_path)}")


# --- Main Conversion Control Functions ---


def start_conversion(gui) -> None:
    """Start queue-based video conversion process."""
    # Check if already running
    if gui.session.running:
        logger.warning("Start clicked while conversion is already running.")
        return

    # Validate queue has items
    if not gui.get_queue_items():
        messagebox.showerror("Error", "Add files or folders to the queue first.")
        return

    # Filter to pending items only
    pending_items = [item for item in gui.get_queue_items() if item.status == "pending"]
    if not pending_items:
        messagebox.showinfo("Info", "All queue items have already been processed.\nClear the queue or add new items.")
        return

    # Get file extensions for folder scanning
    selected_extensions = [
        ext
        for ext, var in [("mp4", gui.ext_mp4), ("mkv", gui.ext_mkv), ("avi", gui.ext_avi), ("wmv", gui.ext_wmv)]
        if var.get()
    ]

    if not selected_extensions:
        messagebox.showerror("Error", "Please select at least one file extension type to process in the Settings tab.")
        return

    # Validate SEPARATE_FOLDER items have output folders
    for item in pending_items:
        if item.output_mode == OutputMode.SEPARATE_FOLDER:
            if not item.output_folder:
                item.output_folder = gui.default_output_folder.get()
            if not item.output_folder:
                messagebox.showerror(
                    "Error",
                    f"Item '{os.path.basename(item.source_path)}' uses 'separate folder' mode "
                    "but no output folder is set.\n\n"
                    "Set a default output folder in Settings, or select the item and set its output folder.",
                )
                return
            # Ensure output folder exists
            try:
                os.makedirs(item.output_folder, exist_ok=True)
            except Exception as e:
                messagebox.showerror("Error", f"Cannot create output folder '{item.output_folder}':\n{e}")
                return

    # Check FFmpeg and ab-av1 dependencies
    if not check_ffmpeg(gui):
        return

    # Create queue config
    config = QueueConversionConfig(
        queue_items=pending_items,
        extensions=selected_extensions,
        convert_audio=gui.convert_audio.get(),
        audio_codec=gui.audio_codec.get(),
        default_suffix=gui.default_suffix.get(),
    )

    # Log settings
    logger.info("--- Starting Queue Conversion ---")
    logger.info(f"Queue items: {len(pending_items)}, Extensions: {', '.join(selected_extensions)}")
    audio_codec_info = config.audio_codec if config.convert_audio else "N/A"
    logger.info(f"Convert Audio: {config.convert_audio} (Codec: {audio_codec_info})")

    # Update UI state
    gui.status_label.config(text="Starting...")
    gui.start_button.config(state="disabled")
    gui.stop_button.config(state="normal")
    gui.force_stop_button.config(state="normal")

    # Initialize conversion state
    gui.session.running = True
    gui.session.total_start_time = time.time()
    gui.session.vmaf_scores = []
    gui.session.crf_values = []
    gui.session.size_reductions = []
    gui.session.processed_files = 0
    gui.session.successful_conversions = 0
    gui.session.error_count = 0
    gui.session.current_process_info = None
    gui.session.total_input_bytes_success = 0
    gui.session.total_output_bytes_success = 0
    gui.session.total_time_success = 0.0
    gui.session.elapsed_timer_id = None
    gui.session.pending_files = []
    gui.session.current_file_path = None
    gui.stop_event = threading.Event()

    # Start timers
    def start_total_timer():
        update_total_elapsed_time(gui)
        if gui.session.running:
            gui.root.after(1000, start_total_timer)

    def start_remaining_timer():
        update_total_remaining_time(gui)
        if gui.session.running:
            gui.root.after(5000, start_remaining_timer)

    start_total_timer()
    start_remaining_timer()

    # Prevent system sleep
    sleep_prevented = prevent_sleep_mode()
    gui.session.sleep_prevention_active = sleep_prevented

    # Reset UI elements
    reset_current_file_details(gui)

    # Create callbacks
    file_callback = create_file_callback_dispatcher(gui)
    queue_callback = create_queue_status_callback(gui)
    get_next_item_callback = create_get_next_pending_item_callback(gui)

    def reset_ui_callback():
        return reset_current_file_details(gui)

    def elapsed_time_callback(start_time):
        return update_elapsed_time(gui, start_time)

    # Start worker thread
    gui.conversion_thread = threading.Thread(
        target=sequential_conversion_worker,
        args=(
            gui,
            config,
            gui.stop_event,
            file_callback,
            queue_callback,
            reset_ui_callback,
            elapsed_time_callback,
            store_process_id,
            conversion_complete,
            get_next_item_callback,
        ),
        daemon=True,
    )
    gui.conversion_thread.start()
    logger.info("Queue conversion worker thread started.")


def stop_conversion(gui) -> None:
    """Stop the conversion process gracefully after the current file finishes.

    Sets the stop event flag to signal the worker thread to terminate after
    the current file completes processing.

    Args:
        gui: The main GUI instance containing conversion state.
    """
    if gui.session.running and gui.stop_event and not gui.stop_event.is_set():
        gui.status_label.config(text="Stopping... (after current file)")
        logger.info("Graceful stop requested (Stop After Current File). Signalling worker thread.")
        gui.stop_event.set()
        gui.stop_button.config(state="disabled")  # Disable button once stop is requested
    elif not gui.session.running:
        logger.info("Stop requested but conversion is not currently running.")
    elif gui.stop_event and gui.stop_event.is_set():
        logger.info("Stop signal has already been sent.")


def terminate_process(pid: int) -> bool:
    """Terminate a process by PID, using platform-specific methods.

    Args:
        pid: Process ID to terminate

    Returns:
        True if termination was successful or process not found, False on error
    """
    if not pid:
        logger.warning("No PID provided for termination")
        return False

    logger.info(f"Attempting to terminate process PID {pid}...")
    try:
        if sys.platform == "win32":
            startupinfo, _ = get_windows_subprocess_startupinfo()
            result = subprocess.run(
                ["taskkill", "/F", "/T", "/PID", str(pid)],
                capture_output=True,
                text=True,
                check=False,
                startupinfo=startupinfo,
            )
            if result.returncode == 0:
                logger.info(f"Successfully terminated PID {pid} and its child processes via taskkill.")
                return True
            logger.warning(f"taskkill failed for PID {pid} (rc={result.returncode}): {result.stderr.strip()}")
            # Fallback: try killing ffmpeg directly if taskkill failed
            try:
                subprocess.run(
                    ["taskkill", "/F", "/IM", "ffmpeg.exe"],
                    capture_output=True,
                    text=True,
                    check=False,
                    startupinfo=startupinfo,
                )
                logger.info("Attempted fallback kill of ffmpeg.exe processes.")
                return True
            except Exception:
                logger.exception("Failed fallback kill of ffmpeg processes")
                return False
        else:  # Linux/macOS
            # Try SIGTERM first, then SIGKILL
            os.kill(pid, signal.SIGTERM)
            time.sleep(0.5)
            try:
                os.kill(pid, 0)  # Check if process still exists
                # If it exists, SIGTERM failed, use SIGKILL
                logger.warning(f"Process {pid} still alive after SIGTERM, sending SIGKILL.")
                os.kill(pid, signal.SIGKILL)
                logger.info(f"Sent SIGKILL to PID {pid}.")
                return True
            except ProcessLookupError:
                logger.info(f"Process {pid} terminated successfully with SIGTERM.")
                return True
    except ProcessLookupError:
        logger.warning(f"Process PID {pid} not found during termination attempt.")
        return True  # Process already gone
    except Exception:
        logger.exception(f"Failed to terminate process PID {pid}")
        return False


def cleanup_temp_file(input_path: str, output_folder: str, input_folder: str) -> None:
    """Clean up temporary conversion file associated with an input file.

    Args:
        input_path: Path to the input file that was being converted
        output_folder: Output folder path
        input_folder: Input folder path (for calculating relative paths)
    """
    if not input_path or not output_folder:
        logger.debug("Cannot cleanup temp file: missing paths")
        return

    temp_filename = "unknown_temp_file.mkv"
    try:
        in_path_obj = Path(input_path)
        out_folder_obj = Path(output_folder)
        relative_dir = Path()

        try:
            relative_dir = in_path_obj.parent.relative_to(Path(input_folder))
        except ValueError:
            logger.debug("Killed file not relative to input base.")

        output_dir_for_file = out_folder_obj / relative_dir
        temp_filename = in_path_obj.stem + ".mkv.temp.mkv"
        temp_file_path = output_dir_for_file / temp_filename

        if temp_file_path.exists():
            logger.info(f"Removing temporary file: {temp_file_path}")
            os.remove(temp_file_path)
            logger.info("Removed temporary file successfully.")
        else:
            logger.debug(f"Specific temp file not found for cleanup: {temp_file_path}")
    except Exception:
        logger.exception(f"Failed to remove specific temp file '{temp_filename}'")


def restore_ui_after_stop(gui) -> None:
    """Restore UI state and system settings after stopping conversion.

    Args:
        gui: The main GUI instance
    """
    # Restore sleep functionality if it was active
    if gui.session.sleep_prevention_active:
        allow_sleep_mode()
        logger.info("System sleep prevention disabled after force stop.")
        gui.session.sleep_prevention_active = False

    # Reset UI and internal state
    gui.session.running = False
    if gui.session.elapsed_timer_id:
        gui.root.after_cancel(gui.session.elapsed_timer_id)
        gui.session.elapsed_timer_id = None

    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Conversion force stopped"))
    update_ui_safely(gui.root, reset_current_file_details, gui)
    update_ui_safely(gui.root, lambda: gui.start_button.config(state="normal"))
    update_ui_safely(gui.root, lambda: gui.stop_button.config(state="disabled"))
    update_ui_safely(gui.root, lambda: gui.force_stop_button.config(state="disabled"))


def force_stop_conversion(gui, confirm: bool = True) -> None:
    """Force stop the conversion process immediately by killing the process and cleaning temp files.

    Args:
        gui: The main GUI instance containing conversion state.
        confirm: Whether to show a confirmation dialog before stopping.
    """
    if not gui.session.running:
        logger.info("Force stop requested but conversion is not running.")
        return

    if confirm and not messagebox.askyesno(
        "Confirm Force Stop",
        "This will immediately terminate the current conversion.\n"
        "The current file will be incomplete.\n\n"
        "Are you sure you want to force stop?",
    ):
        logger.info("User cancelled the force stop request.")
        return

    logger.warning("Force stop initiated by user or internal call.")
    gui.status_label.config(text="Force stopping...")
    if gui.stop_event:
        gui.stop_event.set()

    # Get process information
    pid_to_kill = None
    input_path_killed = None
    if gui.session.current_process_info:
        pid_to_kill = gui.session.current_process_info.get("pid")
        input_path_killed = gui.session.current_process_info.get("input_path")

    # Terminate the process
    if pid_to_kill:
        terminate_process(pid_to_kill)
    else:
        logger.warning("Force stop: No active process PID was recorded. Cannot target specific process.")
        # Fallback: try killing any ffmpeg process if PID wasn't known
        if sys.platform == "win32":
            try:
                startupinfo, _ = get_windows_subprocess_startupinfo()
                subprocess.run(
                    ["taskkill", "/F", "/IM", "ffmpeg.exe"],
                    capture_output=True,
                    text=True,
                    check=False,
                    startupinfo=startupinfo,
                )
                logger.info("Attempted fallback kill of any running ffmpeg.exe processes.")
            except Exception:
                logger.exception("Failed fallback kill of ffmpeg processes")

    # Clear the process info after termination
    gui.session.current_process_info = None

    # Clean up temporary files
    if input_path_killed and gui.output_folder.get():
        cleanup_temp_file(input_path_killed, gui.output_folder.get(), gui.input_folder.get())

    # Restore UI and system state
    restore_ui_after_stop(gui)

    # Schedule general temp folder cleanup
    output_dir_to_clean = gui.session.output_folder_path or gui.output_folder.get()
    if output_dir_to_clean:
        gui.root.after(500, lambda output_dir=output_dir_to_clean: schedule_temp_folder_cleanup(output_dir))
    else:
        logger.warning("Cannot schedule general temp folder cleanup: output directory unclear.")

    logger.info("Conversion force stop procedure complete.")


def build_summary_message(gui, final_message: str, total_duration: float) -> str:
    """Build the completion summary message with statistics.

    Args:
        gui: The main GUI instance containing conversion statistics
        final_message: The main completion message
        total_duration: Total conversion time in seconds

    Returns:
        Formatted summary message string
    """
    s = gui.session  # Shorthand for session state
    if s.processed_files == 0:
        return ""

    summary_msg = (
        f"{final_message}.\n\n"
        f"Files Processed: {s.processed_files}\n"
        f"Successfully Converted: {s.successful_conversions}\n"
    )

    # Show different skip categories
    if s.skipped_not_worth_count > 0:
        summary_msg += f"Skipped (Inefficient): {s.skipped_not_worth_count}\n"

    if s.skipped_low_resolution_count > 0:
        summary_msg += f"Skipped (Low Resolution): {s.skipped_low_resolution_count}\n"

    # Count other skips
    other_skips = (
        s.processed_files
        - s.successful_conversions
        - s.skipped_not_worth_count
        - s.skipped_low_resolution_count
        - s.error_count
    )
    if other_skips > 0:
        summary_msg += f"Skipped (Other): {other_skips}\n"

    if s.error_count > 0:
        summary_msg += f"Errors: {s.error_count}\n"

    summary_msg += f"\nTotal Time: {format_time(total_duration)}\n\n"

    # Show conversion stats if available
    if s.successful_conversions > 0:
        data_saved_bytes = s.total_input_bytes_success - s.total_output_bytes_success
        input_gb_success = s.total_input_bytes_success / (1024**3)
        time_per_gb = (s.total_time_success / input_gb_success) if input_gb_success > 0 else 0
        data_saved_str = format_file_size(data_saved_bytes) if data_saved_bytes > 0 else "N/A"
        time_per_gb_str = format_time(time_per_gb) if time_per_gb > 0 else "N/A"

        summary_msg += "--- Avg. Stats (Successful Files) ---\n"
        summary_msg += f"VMAF Score: {f'{statistics.mean(s.vmaf_scores):.1f}' if s.vmaf_scores else 'N/A'}\n"
        summary_msg += f"CRF Value: {f'{statistics.mean(s.crf_values):.1f}' if s.crf_values else 'N/A'}\n"
        summary_msg += (
            f"Size Reduction: {f'{statistics.mean(s.size_reductions):.1f}%' if s.size_reductions else 'N/A'}\n\n"
            f"--- Overall Performance ---\n"
            f"Total Data Saved: {data_saved_str}\n"
            f"Avg. Processing Time: {time_per_gb_str} per GB Input\n"
        )

    return summary_msg


def format_error_details(error_details: list) -> str:
    """Format error details for display in the summary.

    Args:
        error_details: List of error detail dictionaries

    Returns:
        Formatted error details string
    """
    if not error_details:
        return ""

    msg = "\n--- Error Details ---\n"
    # Group errors by type
    error_types = {}
    for error in error_details:
        error_type = error.get("error_type", "unknown")
        if error_type not in error_types:
            error_types[error_type] = []
        error_types[error_type].append(error["filename"])

    # Display up to 3 files per error type
    for error_type, filenames in error_types.items():
        msg += f"\n{error_type}: {len(filenames)} file{'s' if len(filenames) > 1 else ''}\n"
        for _, filename in enumerate(filenames[:3]):
            msg += f"  - {filename}\n"
        if len(filenames) > 3:  # noqa: PLR2004
            msg += f"  ... and {len(filenames) - 3} more\n"

    return msg


def format_skip_details(skipped_files: list, skipped_low_res_files: list) -> str:
    """Format skip details for display in the summary.

    Args:
        skipped_files: List of files skipped due to inefficiency
        skipped_low_res_files: List of files skipped due to low resolution

    Returns:
        Formatted skip details string
    """
    msg = ""

    # Show files skipped due to inefficiency
    if skipped_files:
        msg += "\n--- Files Skipped (Inefficient Conversion) ---\n"
        msg += "Files where conversion would not save space:\n"
        for _, filename in enumerate(skipped_files[:5]):
            msg += f"  - {filename}\n"
        if len(skipped_files) > 5:  # noqa: PLR2004
            msg += f"  ... and {len(skipped_files) - 5} more\n"

    # Show files skipped due to low resolution
    if skipped_low_res_files:
        msg += "\n--- Files Skipped (Low Resolution) ---\n"
        msg += f"Files below {MIN_RESOLUTION_WIDTH}x{MIN_RESOLUTION_HEIGHT}:\n"
        for _, filename in enumerate(skipped_low_res_files[:5]):
            msg += f"  - {filename}\n"
        if len(skipped_low_res_files) > 5:  # noqa: PLR2004
            msg += f"  ... and {len(skipped_low_res_files) - 5} more\n"

    return msg


def conversion_complete(gui, final_message="Conversion complete"):
    """Handle conversion completion or stopping, update UI, show summary.

    Args:
        gui: The main GUI instance for updating UI elements.
        final_message: Message to display in the UI and logs (e.g., "Complete", "Stopped").
    """
    logger.info(f"--- {final_message} ---")
    s = gui.session  # Shorthand for session state

    # Schedule final cleanup of general temp folders
    output_dir_to_clean = s.output_folder_path or gui.output_folder.get()
    if output_dir_to_clean:
        gui.root.after(100, lambda output_dir=output_dir_to_clean: schedule_temp_folder_cleanup(output_dir))
    else:
        logger.warning("Cannot schedule final temp folder cleanup: output directory unclear.")

    # Log final statistics
    total_duration = time.time() - s.total_start_time if s.total_start_time else 0
    logger.info(f"Total elapsed time: {format_time(total_duration)}")
    logger.info(
        f"Files processed: {s.processed_files}, Successful: {s.successful_conversions}, Errors: {s.error_count}"
    )
    if s.vmaf_scores:
        logger.info(
            f"VMAF (Avg/Min/Max): {statistics.mean(s.vmaf_scores):.1f}/"
            f"{min(s.vmaf_scores):.1f}/{max(s.vmaf_scores):.1f}"
        )
    if s.crf_values:
        logger.info(f"CRF (Avg/Min/Max): {statistics.mean(s.crf_values):.1f}/{min(s.crf_values)}/{max(s.crf_values)}")
    if s.size_reductions:
        logger.info(
            f"Size Reduction (Avg/Min/Max): {statistics.mean(s.size_reductions):.1f}%/"
            f"{min(s.size_reductions):.1f}%/{max(s.size_reductions):.1f}%"
        )

    # Update final UI elements
    gui.status_label.config(text=final_message)
    reset_current_file_details(gui)  # Clear current file section

    # Restore sleep functionality if it was active
    if s.sleep_prevention_active:
        allow_sleep_mode()
        logger.info("System sleep prevention disabled.")
        s.sleep_prevention_active = False

    # Reset button states
    gui.start_button.config(state="normal")
    gui.stop_button.config(state="disabled")
    gui.force_stop_button.config(state="disabled")

    # Final state reset
    s.running = False
    gui.conversion_thread = None
    gui.stop_event = None
    s.current_process_info = None
    s.current_file_path = None
    if s.elapsed_timer_id:  # Ensure timer is stopped if still running
        gui.root.after_cancel(s.elapsed_timer_id)
        s.elapsed_timer_id = None

    # Show summary message box if files were processed
    if s.processed_files > 0:
        # Build main summary message
        summary_msg = build_summary_message(gui, final_message, total_duration)

        # Add error details if any
        if s.error_details:
            summary_msg += format_error_details(s.error_details)
            summary_msg += f"\nCheck the logs ({gui.log_directory}) for full details."

        # Add skip details if any
        summary_msg += format_skip_details(s.skipped_not_worth_files, s.skipped_low_resolution_files)

        # Show appropriate message box
        if s.error_count > 0:
            messagebox.showwarning("Conversion Summary", summary_msg)
        else:
            messagebox.showinfo("Conversion Summary", summary_msg)
