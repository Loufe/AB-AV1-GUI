# src/gui/conversion_controller.py
"""
Conversion control logic (start, stop, force-stop), state management,
and GUI interaction layer for the conversion process.
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

# GUI-related imports
from tkinter import messagebox
import tkinter as tk # For type hinting if needed

# Project imports
# Import from specific ab_av1 submodules
from src.ab_av1.checker import check_ab_av1_available
# Import from utils
from src.utils import (
    get_video_info, format_time, format_file_size, anonymize_filename,
    check_ffmpeg_availability, update_ui_safely,
    append_to_history, get_history_file_path,
    prevent_sleep_mode, allow_sleep_mode
)
# Import constants from config
from src.config import (
    DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET,
    MIN_RESOLUTION_WIDTH, MIN_RESOLUTION_HEIGHT
)
# Import the single-file processing function
from src.video_conversion import process_video
# Import GUI update functions
from src.gui.gui_updates import (
    update_statistics_summary, reset_current_file_details, update_progress_bars,
    update_conversion_statistics, update_elapsed_time, update_total_elapsed_time,
    update_total_remaining_time
)
# Import GUI actions needed here (like check_ffmpeg)
from src.gui.gui_actions import check_ffmpeg
# Import from the new conversion_engine package
from src.conversion_engine.worker import sequential_conversion_worker
from src.conversion_engine.cleanup import schedule_temp_folder_cleanup # Import cleaner scheduling


logger = logging.getLogger(__name__)


# --- Process Management & State ---

def store_process_id(gui, pid: int, input_path: str) -> None:
    """Store the current process ID and its associated input file on the GUI object.

    Args:
        gui: The main GUI instance to store process information on.
        pid: Process ID of the conversion process.
        input_path: Path to the input file being processed.
    """
    gui.current_process_info = {"pid": pid, "input_path": input_path}
    logger.info(f"ab-av1 process started with PID: {pid} for file {anonymize_filename(input_path)}")


# --- Main Conversion Control Functions ---

def start_conversion(gui) -> None:
    """Start the video conversion process.

    Initializes the conversion worker thread, sets up UI state, and prevents
    the system from sleeping during conversion.

    Args:
        gui: The main GUI instance containing conversion settings and UI elements.
    """
    # Check if already running
    if gui.conversion_running:
        logger.warning("Start clicked while conversion is already running.")
        return

    # Get input and output folders
    input_folder = gui.input_folder.get()
    output_folder = gui.output_folder.get()

    # Validate input folder
    if not input_folder or not os.path.isdir(input_folder):
        messagebox.showerror("Error", "Invalid input folder selected.")
        return

    # If no output folder, use input folder
    if not output_folder:
        output_folder = input_folder
        gui.output_folder.set(output_folder) # Update the GUI variable
        logger.info(f"Output folder was empty, automatically set to match input: {output_folder}")
    # Ensure output folder exists (create if needed)
    elif not os.path.isdir(output_folder):
         try:
            logger.info(f"Output folder '{output_folder}' does not exist. Attempting to create.")
            os.makedirs(output_folder, exist_ok=True)
            logger.info(f"Successfully created output folder: {output_folder}")
         except Exception as e:
             logger.error(f"Failed to create output folder '{output_folder}': {e}", exc_info=True)
             messagebox.showerror("Error", f"Cannot create output folder:\n{e}")
             return

    # Get file extensions to process from GUI state
    selected_extensions = [ext for ext, var in [
        ("mp4", gui.ext_mp4), ("mkv", gui.ext_mkv),
        ("avi", gui.ext_avi), ("wmv", gui.ext_wmv)
    ] if var.get()]

    # Check if at least one extension is selected
    if not selected_extensions:
        messagebox.showerror("Error", "Please select at least one file extension type to process in the Settings tab.")
        return

    # Get overwrite setting from GUI state
    overwrite = gui.overwrite.get()

    # Check FFmpeg and ab-av1 dependencies
    if not check_ffmpeg(gui): # check_ffmpeg is from gui_actions
        return

    # --- REMOVED Pre-check for MKV in-place conversion without overwrite ---
    # This is now handled within process_video by adding a suffix.

    # Get remaining conversion settings from GUI state
    convert_audio = gui.convert_audio.get()
    audio_codec = gui.audio_codec.get()
    delete_original = gui.delete_original_var.get() # Get the state of the checkbox

    # Log settings for the run
    logger.info("--- Starting Conversion ---")
    logger.info(f"Input: {input_folder}, Output: {output_folder}, Extensions: {', '.join(selected_extensions)}")
    logger.info(f"Overwrite: {overwrite}, Convert Audio: {convert_audio} (Codec: {audio_codec if convert_audio else 'N/A'}), Delete Original: {delete_original}")
    logger.info(f"Using -> Preset: {DEFAULT_ENCODING_PRESET}, VMAF Target: {DEFAULT_VMAF_TARGET}")

    # Update UI state for running conversion
    gui.status_label.config(text="Starting...")
    logging.info("Preparing conversion worker...")
    gui.start_button.config(state="disabled")
    gui.stop_button.config(state="normal")
    gui.force_stop_button.config(state="normal")

    # Initialize conversion state variables on the GUI object
    gui.conversion_running = True
    gui.stop_event = threading.Event()
    gui.total_conversion_start_time = time.time()
    gui.vmaf_scores = []; gui.crf_values = []; gui.size_reductions = []
    gui.processed_files = 0; gui.successful_conversions = 0; gui.error_count = 0
    gui.current_process_info = None
    gui.total_input_bytes_success = 0; gui.total_output_bytes_success = 0; gui.total_time_success = 0
    gui.elapsed_timer_id = None  # Initialize timer ID
    gui.pending_files = []  # Initialize pending files as empty - will be populated after scan
    gui.current_file_path = None  # Initialize current file path
    
    logger.info("Initialized conversion state variables")
    
    # Start timer for total elapsed time immediately
    def start_total_timer():
        update_total_elapsed_time(gui)
        if gui.conversion_running:
            gui.root.after(1000, start_total_timer)
    
    # Start separate timer for remaining time updates - less frequent
    def start_remaining_timer():
        update_total_remaining_time(gui)
        if gui.conversion_running:
            gui.root.after(5000, start_remaining_timer)  # Update every 5 seconds instead of 1
    
    start_total_timer()
    start_remaining_timer()

    # Prevent system sleep (Windows only)
    sleep_prevented = prevent_sleep_mode()
    if sleep_prevented:
        logging.info("System sleep prevention enabled.")
        gui.sleep_prevention_active = True
    else:
        # Log warning if couldn't prevent sleep (non-Windows or error)
        if sys.platform == "win32":
            logging.warning("Could not enable sleep prevention on Windows.")
        else:
             logging.info("Sleep prevention is only implemented for Windows.")
        gui.sleep_prevention_active = False

    # Reset UI elements related to conversion progress and stats
    update_statistics_summary(gui) # Reset overall stats display
    reset_current_file_details(gui) # Reset current file details display

    # Start the conversion worker thread
    # Pass necessary callbacks (store_process_id, conversion_complete)
    gui.conversion_thread = threading.Thread(
        target=sequential_conversion_worker,
        args=(gui, input_folder, output_folder, overwrite, gui.stop_event,
              convert_audio, audio_codec, delete_original, # Pass the delete_original flag
              store_process_id, conversion_complete),
        daemon=True # Ensure thread exits if main app crashes
    )
    gui.conversion_thread.start()
    logger.info("Conversion worker thread started.")


def stop_conversion(gui) -> None:
    """Stop the conversion process gracefully after the current file finishes.

    Sets the stop event flag to signal the worker thread to terminate after
    the current file completes processing.

    Args:
        gui: The main GUI instance containing conversion state.
    """
    if gui.conversion_running and gui.stop_event and not gui.stop_event.is_set():
        gui.status_label.config(text="Stopping... (after current file)")
        logger.info("Graceful stop requested (Stop After Current File). Signalling worker thread.")
        gui.stop_event.set()
        gui.stop_button.config(state="disabled") # Disable button once stop is requested
    elif not gui.conversion_running:
        logger.info("Stop requested but conversion is not currently running.")
    elif gui.stop_event and gui.stop_event.is_set():
        logger.info("Stop signal has already been sent.")


def force_stop_conversion(gui, confirm: bool = True) -> None:
    """Force stop the conversion process immediately by killing the process and cleaning temp files.

    Args:
        gui: The main GUI instance containing conversion state.
        confirm: Whether to show a confirmation dialog before stopping.
    """
    if not gui.conversion_running:
        logger.info("Force stop requested but conversion is not running.")
        return

    if confirm:
        # Ask for user confirmation before proceeding with force stop
        if not messagebox.askyesno("Confirm Force Stop",
                                   "This will immediately terminate the current conversion.\n"
                                   "The current file will be incomplete.\n\n"
                                   "Are you sure you want to force stop?"):
            logger.info("User cancelled the force stop request.")
            return

    logger.warning("Force stop initiated by user or internal call.")
    gui.status_label.config(text="Force stopping...")
    if gui.stop_event:
        gui.stop_event.set() # Signal the worker thread as well, though process kill is primary

    pid_to_kill = None
    input_path_killed = None
    # Get PID of the currently running process, if available
    if gui.current_process_info:
        pid_to_kill = gui.current_process_info.get("pid")
        input_path_killed = gui.current_process_info.get("input_path")

    # --- Terminate Process ---
    if pid_to_kill:
        logger.info(f"Attempting to terminate process PID {pid_to_kill}...")
        try:
            if sys.platform == "win32":
                # Use taskkill on Windows with /T to kill process tree, /F for force
                startupinfo = subprocess.STARTUPINFO()
                startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                startupinfo.wShowWindow = subprocess.SW_HIDE
                result = subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid_to_kill)],
                                       capture_output=True, text=True, check=False, startupinfo=startupinfo)
                if result.returncode == 0:
                    logger.info(f"Successfully terminated PID {pid_to_kill} and its child processes via taskkill.")
                else:
                    # Log failure details if taskkill returns non-zero
                    logger.warning(f"taskkill failed for PID {pid_to_kill} (rc={result.returncode}): {result.stderr.strip()}")
                    # Fallback: try killing ffmpeg directly if taskkill failed
                    try:
                        subprocess.run(["taskkill", "/F", "/IM", "ffmpeg.exe"], capture_output=True, text=True, check=False, startupinfo=startupinfo)
                        logger.info("Attempted fallback kill of ffmpeg.exe processes.")
                    except Exception as ffmpeg_e:
                        logger.error(f"Failed fallback kill of ffmpeg processes: {ffmpeg_e}")
            else: # Linux/macOS
                # Try SIGTERM first, then SIGKILL
                os.kill(pid_to_kill, signal.SIGTERM)
                time.sleep(0.5) # Give it a moment
                try:
                    os.kill(pid_to_kill, 0) # Check if process still exists
                    # If it exists, SIGTERM failed, use SIGKILL
                    logger.warning(f"Process {pid_to_kill} still alive after SIGTERM, sending SIGKILL.")
                    os.kill(pid_to_kill, signal.SIGKILL)
                    logger.info(f"Sent SIGKILL to PID {pid_to_kill}.")
                except ProcessLookupError:
                    logger.info(f"Process {pid_to_kill} terminated successfully with SIGTERM.")
        except ProcessLookupError:
            logger.warning(f"Process PID {pid_to_kill} not found during termination attempt.")
        except Exception as e:
            logger.error(f"Failed to terminate process PID {pid_to_kill}: {str(e)}")
    else:
        logger.warning("Force stop: No active process PID was recorded. Cannot target specific process.")
        # Fallback: try killing any ffmpeg process if PID wasn't known
        if sys.platform == "win32":
             try:
                 startupinfo = subprocess.STARTUPINFO(); startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW; startupinfo.wShowWindow = subprocess.SW_HIDE
                 subprocess.run(["taskkill", "/F", "/IM", "ffmpeg.exe"], capture_output=True, text=True, check=False, startupinfo=startupinfo)
                 logger.info("Attempted fallback kill of any running ffmpeg.exe processes.")
             except Exception as e: logger.error(f"Failed fallback kill of ffmpeg processes: {e}")

    # Clear the process info AFTER termination attempts complete to avoid race condition
    gui.current_process_info = None

    # --- Cleanup Temporary Files ---
    # Attempt to remove the specific '.temp.mkv' file associated with the killed process
    if input_path_killed and gui.output_folder.get():
        temp_filename = "unknown_temp_file.mkv" # Initialize temp_filename
        try:
            in_path_obj = Path(input_path_killed)
            out_folder_obj = Path(gui.output_folder.get())
            relative_dir = Path(".")
            try: # Calculate relative path for output dir structure
                relative_dir = in_path_obj.parent.relative_to(Path(gui.input_folder.get()))
            except ValueError: logger.debug("Killed file not relative to input base.")

            output_dir_for_file = out_folder_obj / relative_dir
            temp_filename = in_path_obj.stem + ".mkv.temp.mkv"
            temp_file_path = output_dir_for_file / temp_filename

            if temp_file_path.exists():
                logger.info(f"Removing temporary file: {temp_file_path}")
                os.remove(temp_file_path)
                logger.info("Removed temporary file successfully.")
            else:
                logger.debug(f"Specific temp file not found for cleanup: {temp_file_path}")
        except Exception as cleanup_err:
            logger.error(f"Failed to remove specific temp file '{temp_filename}': {cleanup_err}")

    # --- Restore System State & Reset UI ---
    # Restore sleep functionality if it was active
    if hasattr(gui, 'sleep_prevention_active') and gui.sleep_prevention_active:
        allow_sleep_mode()
        logger.info("System sleep prevention disabled after force stop.")
        gui.sleep_prevention_active = False

    # Reset UI and internal state
    gui.conversion_running = False
    if gui.elapsed_timer_id: # Stop the elapsed timer updates
        gui.root.after_cancel(gui.elapsed_timer_id)
        gui.elapsed_timer_id = None

    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Conversion force stopped"))
    update_ui_safely(gui.root, reset_current_file_details, gui) # Reset current file details
    update_ui_safely(gui.root, lambda: gui.start_button.config(state="normal"))
    update_ui_safely(gui.root, lambda: gui.stop_button.config(state="disabled"))
    update_ui_safely(gui.root, lambda: gui.force_stop_button.config(state="disabled"))

    # Schedule general temp folder cleanup (for .ab-av1-* folders) after a short delay
    # Use the output folder path stored on the gui object
    output_dir_to_clean = getattr(gui, 'output_folder_path', None) or gui.output_folder.get()
    if output_dir_to_clean:
        gui.root.after(500, lambda dir=output_dir_to_clean: schedule_temp_folder_cleanup(dir))
    else:
        logger.warning("Cannot schedule general temp folder cleanup: output directory unclear.")

    logger.info("Conversion force stop procedure complete.")


def conversion_complete(gui, final_message="Conversion complete"):
    """Handle conversion completion or stopping, update UI, show summary.

    Args:
        gui: The main GUI instance for updating UI elements.
        final_message: Message to display in the UI and logs (e.g., "Complete", "Stopped").
    """
    logger.info(f"--- {final_message} ---")

    # Schedule final cleanup of general temp folders
    output_dir_to_clean = getattr(gui, 'output_folder_path', None) or gui.output_folder.get()
    if output_dir_to_clean:
        gui.root.after(100, lambda dir=output_dir_to_clean: schedule_temp_folder_cleanup(dir))
    else:
         logger.warning("Cannot schedule final temp folder cleanup: output directory unclear.")

    # Log final statistics
    total_duration = time.time() - gui.total_conversion_start_time if hasattr(gui,'total_conversion_start_time') else 0
    logger.info(f"Total elapsed time: {format_time(total_duration)}")
    logger.info(f"Files processed: {gui.processed_files}, Successful: {gui.successful_conversions}, Errors: {gui.error_count}")
    if gui.vmaf_scores: logger.info(f"VMAF (Avg/Min/Max): {statistics.mean(gui.vmaf_scores):.1f}/{min(gui.vmaf_scores):.1f}/{max(gui.vmaf_scores):.1f}")
    if gui.crf_values: logger.info(f"CRF (Avg/Min/Max): {statistics.mean(gui.crf_values):.1f}/{min(gui.crf_values)}/{max(gui.crf_values)}")
    if gui.size_reductions: logger.info(f"Size Reduction (Avg/Min/Max): {statistics.mean(gui.size_reductions):.1f}%/{min(gui.size_reductions):.1f}%/{max(gui.size_reductions):.1f}%")

    # Calculate overall performance metrics
    data_saved_bytes = gui.total_input_bytes_success - gui.total_output_bytes_success
    input_gb_success = gui.total_input_bytes_success / (1024**3)
    time_per_gb = (gui.total_time_success / input_gb_success) if input_gb_success > 0 else 0
    data_saved_str = format_file_size(data_saved_bytes) if data_saved_bytes > 0 else "N/A"
    time_per_gb_str = format_time(time_per_gb) if time_per_gb > 0 else "N/A"

    # Update final UI elements
    gui.status_label.config(text=final_message)
    reset_current_file_details(gui) # Clear current file section

    # Restore sleep functionality if it was active
    if hasattr(gui, 'sleep_prevention_active') and gui.sleep_prevention_active:
        allow_sleep_mode()
        logger.info("System sleep prevention disabled.")
        gui.sleep_prevention_active = False

    # Reset button states
    gui.start_button.config(state="normal")
    gui.stop_button.config(state="disabled")
    gui.force_stop_button.config(state="disabled")

    # Final state reset
    gui.conversion_running = False
    gui.conversion_thread = None
    gui.stop_event = None
    gui.current_process_info = None
    gui.current_file_path = None  # Clear current file path
    if gui.elapsed_timer_id: # Ensure timer is stopped if still running
        gui.root.after_cancel(gui.elapsed_timer_id)
        gui.elapsed_timer_id = None
    
    # Update statistics to show historical data
    update_statistics_summary(gui)

    # Show summary message box if files were processed
    if gui.processed_files > 0:
        summary_msg = f"{final_message}.\n\n" \
                      f"Files Processed: {gui.processed_files}\n" \
                      f"Successfully Converted: {gui.successful_conversions}\n"
        
        # Show different skip categories
        skipped_not_worth = getattr(gui, 'skipped_not_worth_count', 0)
        skipped_low_resolution = getattr(gui, 'skipped_low_resolution_count', 0)
        
        if skipped_not_worth > 0:
            summary_msg += f"Skipped (Inefficient): {skipped_not_worth}\n"
        
        if skipped_low_resolution > 0:
            summary_msg += f"Skipped (Low Resolution): {skipped_low_resolution}\n"
        
        # Count other skips
        error_count = getattr(gui, 'error_count', 0)
        other_skips = gui.processed_files - gui.successful_conversions - skipped_not_worth - skipped_low_resolution - error_count
        if other_skips > 0:
            summary_msg += f"Skipped (Other): {other_skips}\n"
        
        if error_count > 0:
            summary_msg += f"Errors: {error_count}\n"
        
        summary_msg += f"\nTotal Time: {format_time(total_duration)}\n\n"
        
        # Show conversion stats if available
        if gui.successful_conversions > 0:
            summary_msg += f"--- Avg. Stats (Successful Files) ---\n"
            summary_msg += f"VMAF Score: {f'{statistics.mean(gui.vmaf_scores):.1f}' if gui.vmaf_scores else 'N/A'}\n"
            summary_msg += f"CRF Value: {f'{statistics.mean(gui.crf_values):.1f}' if gui.crf_values else 'N/A'}\n"
            summary_msg += f"Size Reduction: {f'{statistics.mean(gui.size_reductions):.1f}%' if gui.size_reductions else 'N/A'}\n\n" \
                           f"--- Overall Performance ---\n" \
                           f"Total Data Saved: {data_saved_str}\n" \
                           f"Avg. Processing Time: {time_per_gb_str} per GB Input\n"
        
        # Show error details if any
        error_details = getattr(gui, 'error_details', [])
        if error_details:
            summary_msg += "\n--- Error Details ---\n"
            # Group errors by type
            error_types = {}
            for error in error_details:
                error_type = error.get('error_type', 'unknown')
                if error_type not in error_types:
                    error_types[error_type] = []
                error_types[error_type].append(error['filename'])
            
            # Display up to 3 files per error type
            for error_type, filenames in error_types.items():
                summary_msg += f"\n{error_type}: {len(filenames)} file{'s' if len(filenames) > 1 else ''}\n"
                for i, filename in enumerate(filenames[:3]):
                    summary_msg += f"  - {filename}\n"
                if len(filenames) > 3:
                    summary_msg += f"  ... and {len(filenames) - 3} more\n"
            
            summary_msg += f"\nCheck the logs ({gui.log_directory}) for full details."
        
        # Show files skipped due to inefficiency
        skipped_files = getattr(gui, 'skipped_not_worth_files', [])
        if skipped_files:
            summary_msg += "\n--- Files Skipped (Inefficient Conversion) ---\n"
            summary_msg += f"Files where conversion would not save space:\n"
            for i, filename in enumerate(skipped_files[:5]):
                summary_msg += f"  - {filename}\n"
            if len(skipped_files) > 5:
                summary_msg += f"  ... and {len(skipped_files) - 5} more\n"
        
        # Show files skipped due to low resolution
        skipped_low_res_files = getattr(gui, 'skipped_low_resolution_files', [])
        if skipped_low_res_files:
            summary_msg += "\n--- Files Skipped (Low Resolution) ---\n"
            summary_msg += f"Files below {MIN_RESOLUTION_WIDTH}x{MIN_RESOLUTION_HEIGHT}:\n"
            for i, filename in enumerate(skipped_low_res_files[:5]):
                summary_msg += f"  - {filename}\n"
            if len(skipped_low_res_files) > 5:
                summary_msg += f"  ... and {len(skipped_low_res_files) - 5} more\n"
        
        if error_count > 0:
            messagebox.showwarning("Conversion Summary", summary_msg)
        else:
            messagebox.showinfo("Conversion Summary", summary_msg)