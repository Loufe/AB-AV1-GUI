# src/gui/conversion_controller.py
"""
Conversion control logic, worker thread, callback handlers, and related functions
for the AV1 Video Converter application.
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
from pathlib import Path # Added import
import math # For ceil
import datetime # For history timestamp

# GUI-related imports
from tkinter import messagebox
import tkinter as tk # For type hinting if needed

# Project imports - Replace 'convert_app' with 'src'
from src.ab_av1_wrapper import check_ab_av1_available, clean_ab_av1_temp_folders
from src.utils import (
    get_video_info, format_time, format_file_size, anonymize_filename,
    check_ffmpeg_availability, update_ui_safely,
    append_to_history, get_history_file_path,
    prevent_sleep_mode, allow_sleep_mode
)
# Import constants from config
from src.config import (
    DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET
)
from src.video_conversion import process_video
# Import newly created GUI modules - Replace 'convert_app' with 'src'
from src.gui.gui_updates import (
    update_statistics_summary, reset_current_file_details, update_progress_bars,
    update_conversion_statistics, update_elapsed_time, update_total_elapsed_time
)
from src.gui.gui_actions import check_ffmpeg # Used by start_conversion


logger = logging.getLogger(__name__)


# --- File Scanning Helper (used by worker) ---

def scan_video_needs_conversion(gui, video_path: str, output_folder_path: str, overwrite: bool = False, video_info_cache: dict = None) -> tuple:
    """Scan a video file to determine if it needs conversion, using a cache.

    Args:
        gui: The main GUI instance containing input folder info
        video_path: Path to the video file to scan
        output_folder_path: Directory where output would be saved
        overwrite: Whether to overwrite existing output files
        video_info_cache: Dictionary to use for caching ffprobe results.

    Returns:
        Tuple of (needs_conversion, reason) where needs_conversion is a boolean
        and reason is a string explaining why conversion is needed or not needed
    """
    anonymized_input = anonymize_filename(video_path)

    try:
        input_path_obj = Path(video_path)
        input_folder_obj = Path(gui.input_folder.get())
        output_folder_obj = Path(output_folder_path)

        relative_dir = input_path_obj.parent.relative_to(input_folder_obj)
        output_dir = output_folder_obj / relative_dir
        output_filename = input_path_obj.stem + ".mkv"
        output_path = output_dir / output_filename
        anonymized_output = anonymize_filename(str(output_path))

    except ValueError:
        # Handle case when input is not relative to input folder
        input_path_obj = Path(video_path)
        output_folder_obj = Path(output_folder_path)
        output_filename = input_path_obj.stem + ".mkv"
        output_path = output_folder_obj / output_filename
        anonymized_output = anonymize_filename(str(output_path))
        logging.debug(f"File {anonymized_input} not relative, using direct output: {output_path}")

    except Exception as e:
        logging.error(f"Error determining output path for {anonymized_input}: {e}")
        return True, "Error determining output path"

    # Check if output exists and we're not overwriting
    if os.path.exists(output_path) and not overwrite:
        # Check if the existing path is the *same* as the input path (in-place conversion attempt)
        if input_path_obj.resolve() == Path(output_path).resolve():
             logging.warning(f"Skipping {anonymized_input} - In-place conversion needs 'Overwrite' enabled.")
             return False, "In-place MKV conversion requires Overwrite"
        else:
            logging.info(f"Skipping {anonymized_input} - output exists: {anonymized_output}")
            return False, "Output file exists"

    # --- Video Info Caching ---
    video_info = None
    cache_was_hit = False
    if video_info_cache is not None and video_path in video_info_cache:
        video_info = video_info_cache[video_path]
        cache_was_hit = True
        logging.debug(f"Cache hit for {anonymized_input}")
    else:
        try:
            video_info = get_video_info(video_path)
            if video_info and video_info_cache is not None: # Store only on success if cache provided
                video_info_cache[video_path] = video_info
                logging.debug(f"Cache miss, stored info for {anonymized_input}")
            # else: video_info remains None if get_video_info failed
        except Exception as e:
            # Handle potential errors during get_video_info itself
            logging.error(f"Error getting video info for caching {anonymized_input}: {e}")
            video_info = None # Ensure video_info is None on error
    # --- End Caching ---

    try:
        if not video_info:
            logging.warning(f"Cannot analyze {anonymized_input} - will attempt conversion (cached: {cache_was_hit})")
            # If analysis failed, assume conversion is needed unless skipped for other reasons
            return True, "Analysis failed"

        is_already_av1 = False
        video_stream_found = False

        for stream in video_info.get("streams", []):
            if stream.get("codec_type") == "video":
                video_stream_found = True
                if stream.get("codec_name", "").lower() == "av1":
                    is_already_av1 = True
                    break

        if not video_stream_found:
            logging.warning(f"No video stream in {anonymized_input} - skipping.")
            return False, "No video stream found"

        # Check container type based on original file extension (MKV)
        # We assume if it's already AV1, it only needs skipping if it's ALSO in an MKV container.
        is_mkv_container = video_path.lower().endswith(".mkv")
        if is_already_av1 and is_mkv_container:
            logging.info(f"Skipping {anonymized_input} - already AV1/MKV")
            return False, "Already AV1/MKV"

        # If it's AV1 but not MKV, or not AV1 at all, it needs conversion
        return True, "Needs conversion (not AV1 or not MKV)"

    except Exception as e:
        logging.error(f"Error checking file {anonymized_input}: {str(e)}")
        # If an unexpected error occurs during the check, assume conversion might be needed
        return True, f"Error during check: {str(e)}"


# --- Process Management & Cleanup ---

def store_process_id(gui, pid: int, input_path: str) -> None:
    """Store the current process ID and its associated input file.

    Args:
        gui: The main GUI instance to store process information on
        pid: Process ID of the conversion process
        input_path: Path to the input file being processed
    """
    gui.current_process_info = {"pid": pid, "input_path": input_path}
    logger.info(f"ab-av1 process started with PID: {pid} for file {anonymize_filename(input_path)}")


def schedule_temp_folder_cleanup(directory: str) -> None:
    """Schedule cleanup of general temp folders after conversion.

    Args:
        directory: Directory where temporary folders might be located
    """
    try:
        logging.info(f"Scheduling cleanup of temp folders in: {directory}")
        cleaned_count = clean_ab_av1_temp_folders(directory)
        if cleaned_count > 0: logging.info(f"Cleaned up {cleaned_count} temp folders in {directory}.")
        else: logging.debug(f"No temp folders found to clean in {directory}.")
    except Exception as e: logging.warning(f"Could not clean up temp folders in {directory}: {str(e)}")


# --- Main Conversion Control Functions ---

def start_conversion(gui) -> None:
    """Start the video conversion process.

    Initializes the conversion worker thread, sets up UI state, and prevents
    the system from sleeping during conversion.

    Args:
        gui: The main GUI instance containing conversion settings and UI elements
    """
    # Check if already running
    if gui.conversion_running:
        logger.warning("Start clicked while running.")
        return

    # Get input and output folders
    input_folder = gui.input_folder.get()
    output_folder = gui.output_folder.get()

    # Validate input folder
    if not input_folder or not os.path.isdir(input_folder):
        messagebox.showerror("Error", "Invalid input folder")
        return

    # If no output folder, use input folder
    if not output_folder:
        output_folder = input_folder
        gui.output_folder.set(output_folder)
        logger.info(f"Output set to input: {output_folder}")
    # Ensure output folder exists after potentially setting it to input
    elif not os.path.isdir(output_folder):
         try:
            os.makedirs(output_folder, exist_ok=True)
         except Exception as e:
             logger.error(f"Cannot create output folder '{output_folder}': {e}")
             messagebox.showerror("Error", f"Cannot create output folder:\n{e}")
             return

    # Get file extensions to process
    selected_extensions = [ext for ext, var in [
        ("mp4", gui.ext_mp4),
        ("mkv", gui.ext_mkv),
        ("avi", gui.ext_avi),
        ("wmv", gui.ext_wmv)
    ] if var.get()]

    # Check if at least one extension is selected
    if not selected_extensions:
        messagebox.showerror("Error", "Select file extensions in Settings")
        return

    # Get overwrite setting
    overwrite = gui.overwrite.get()

    # Check FFmpeg and ab-av1 (imported from gui_actions)
    if not check_ffmpeg(gui):
        return

    # --- Pre-check for MKV in-place conversion without overwrite ---
    if input_folder == output_folder and not overwrite and gui.ext_mkv.get():
        # Check if there are actually MKV files in the input folder
        try:
            input_path_obj = Path(input_folder)
            # Use case-insensitive globbing if possible or check both cases
            has_mkv_files = any(input_path_obj.rglob("*.mkv")) or any(input_path_obj.rglob("*.MKV"))
        except Exception as scan_e:
            # If scan fails, assume there might be MKVs to be safe and show warning
            logger.warning(f"Could not scan for MKV files during pre-check: {scan_e}")
            has_mkv_files = True

        if has_mkv_files:
            logger.warning("Detected potential in-place MKV conversion conflict without overwrite enabled.")
            messagebox.showwarning(
                "Configuration Conflict",
                "Input and Output folders are the same, 'Overwrite' is disabled, and MKV processing is enabled.\n\n"
                "Existing MKV files in the input folder cannot be converted in-place under these settings and will be skipped.\n\n"
                "To convert these MKV files:\n"
                "  - Enable 'Overwrite output file...' in the Settings tab, OR\n"
                "  - Choose a different Output Folder."
            )
            return # Stop the conversion from starting

    # --- End Pre-check ---

    # Get remaining conversion settings
    convert_audio = gui.convert_audio.get()
    audio_codec = gui.audio_codec.get()

    # Log settings (use constants from config)
    logger.info("--- Starting Conversion ---")
    logger.info(f"Input: {input_folder}, Output: {output_folder}, Extensions: {', '.join(selected_extensions)}")
    logger.info(f"Overwrite: {overwrite}, Convert Audio: {convert_audio} (Codec: {audio_codec if convert_audio else 'N/A'})")
    logger.info(f"Using -> Preset: {DEFAULT_ENCODING_PRESET}, VMAF Target: {DEFAULT_VMAF_TARGET}")

    # Update UI
    gui.status_label.config(text="Starting...")
    logging.info("Preparing conversion...")
    gui.start_button.config(state="disabled")
    gui.stop_button.config(state="normal")
    gui.force_stop_button.config(state="normal")

    # Set conversion state
    gui.conversion_running = True
    gui.stop_event = threading.Event()
    gui.total_conversion_start_time = time.time()
    gui.vmaf_scores = []
    gui.crf_values = []
    gui.size_reductions = []
    gui.processed_files = 0
    gui.successful_conversions = 0
    gui.error_count = 0
    gui.current_process_info = None
    gui.total_input_bytes_success = 0
    gui.total_output_bytes_success = 0
    gui.total_time_success = 0

    # Prevent system from sleeping during conversion
    sleep_prevented = prevent_sleep_mode()
    if sleep_prevented:
        logging.info("Sleep prevention enabled - computer will stay awake during conversion")
        gui.sleep_prevention_active = True
    else:
        logging.warning("Could not enable sleep prevention - computer may go to sleep during conversion")
        gui.sleep_prevention_active = False

    # Reset UI elements (using functions from gui_updates)
    update_statistics_summary(gui)
    reset_current_file_details(gui)

    # Start conversion thread
    gui.conversion_thread = threading.Thread(
        target=sequential_conversion_worker,
        args=(gui, input_folder, output_folder, overwrite, gui.stop_event, convert_audio, audio_codec),
        daemon=True
    )
    gui.conversion_thread.start()


def stop_conversion(gui) -> None:
    """Stop the conversion process gracefully after the current file finishes.

    Sets the stop event flag to signal the worker thread to terminate after
    the current file completes processing.

    Args:
        gui: The main GUI instance containing conversion state
    """
    if gui.conversion_running and gui.stop_event and not gui.stop_event.is_set():
        gui.status_label.config(text="Stopping... (after current file)")
        logging.info("Graceful stop requested (Stop After Current File). Signalling worker.")
        gui.stop_event.set()
        gui.stop_button.config(state="disabled")
    elif not gui.conversion_running:
        logging.info("Stop requested but not running.")
    elif gui.stop_event and gui.stop_event.is_set():
        logging.info("Stop already requested.")


def force_stop_conversion(gui, confirm: bool = True) -> None:
    """Force stop the conversion process immediately by killing the process and cleaning temp files.

    This is more aggressive than stop_conversion() and will terminate the current encoding
    process immediately rather than waiting for the current file to complete.

    Args:
        gui: The main GUI instance containing conversion state
        confirm: Whether to show a confirmation dialog before stopping
    """
    if not gui.conversion_running:
        logging.info("Force stop requested but not running."); return

    if confirm:
        if not messagebox.askyesno("Confirm Force Stop", "Terminate current conversion immediately?\nIncomplete files may be left if cleanup fails."):
            logging.info("User cancelled force stop."); return

    logging.warning("Force stop initiated."); gui.status_label.config(text="Force stopping...")
    if gui.stop_event: gui.stop_event.set()

    pid_to_kill = None; input_path_killed = None
    if gui.current_process_info:
        pid_to_kill = gui.current_process_info.get("pid")
        input_path_killed = gui.current_process_info.get("input_path")
        gui.current_process_info = None

    # Try to kill the main process with more aggressive options
    if pid_to_kill:
        logging.info(f"Attempting to terminate process PID {pid_to_kill}...")
        if sys.platform == "win32":
            try:
                # First try to find and kill all child processes
                startupinfo = subprocess.STARTUPINFO()
                startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                startupinfo.wShowWindow = subprocess.SW_HIDE

                # Use taskkill with /T flag to kill process tree
                result = subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid_to_kill)],
                               capture_output=True, text=True, check=False, startupinfo=startupinfo)

                if result.returncode == 0:
                    logging.info(f"Terminated PID {pid_to_kill} and its child processes via taskkill.")
                else:
                    # Log failure details
                    logging.warning(f"taskkill failed for PID {pid_to_kill} (rc={result.returncode}): {result.stderr.strip()}")

                    # If normal termination failed, try to find FFmpeg processes that might be running
                    try:
                        # Find FFmpeg processes that might be related
                        subprocess.run(["taskkill", "/F", "/IM", "ffmpeg.exe"],
                                      capture_output=True, text=True, check=False, startupinfo=startupinfo)
                        logging.info("Attempted to kill any running ffmpeg.exe processes")
                    except Exception as ffmpeg_e:
                        logging.error(f"Failed to kill ffmpeg processes: {ffmpeg_e}")
            except Exception as e:
                logging.error(f"Failed to terminate Windows processes: {str(e)}")
        else:
            # On Unix/Linux systems, try more aggressive process termination
            try:
                # First try SIGTERM
                os.kill(pid_to_kill, signal.SIGTERM)
                time.sleep(0.5) # Give it a moment to terminate gracefully

                # Check if process still exists and send SIGKILL if needed
                try:
                    os.kill(pid_to_kill, 0)  # This will raise an error if process doesn't exist
                    # Process still exists, use SIGKILL
                    os.kill(pid_to_kill, signal.SIGKILL)
                    logging.info(f"Sent SIGKILL to PID {pid_to_kill} after SIGTERM failed.")
                except ProcessLookupError:
                    logging.info(f"Process {pid_to_kill} terminated successfully with SIGTERM.")
            except ProcessLookupError:
                logging.warning(f"PID {pid_to_kill} not found.")
            except Exception as e:
                logging.error(f"Failed to terminate PID {pid_to_kill}: {str(e)}")
    else:
        logging.warning("Force stop: No active process PID recorded.")

        # Still try to kill any ffmpeg processes as a fallback
        if sys.platform == "win32":
            try:
                startupinfo = subprocess.STARTUPINFO()
                startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                startupinfo.wShowWindow = subprocess.SW_HIDE
                subprocess.run(["taskkill", "/F", "/IM", "ffmpeg.exe"],
                              capture_output=True, text=True, check=False, startupinfo=startupinfo)
                logging.info("Attempted to kill any running ffmpeg.exe processes as fallback")
            except Exception as e:
                logging.error(f"Failed to kill ffmpeg processes: {e}")

    # Clean up temporary files
    if input_path_killed and gui.output_folder.get():
        try:
            in_path_obj = Path(input_path_killed)
            out_folder_obj = Path(gui.output_folder.get())
            relative_dir = Path(".")
            try:
                relative_dir = in_path_obj.parent.relative_to(Path(gui.input_folder.get()))
            except ValueError: # Use ValueError for path relativity check
                logging.debug("Killed file not relative to input base.")

            output_dir = out_folder_obj / relative_dir
            temp_filename = in_path_obj.stem + ".mkv.temp.mkv"
            temp_file_path = output_dir / temp_filename

            if temp_file_path.exists():
                logging.info(f"Removing temporary file: {temp_file_path}")
                os.remove(temp_file_path)
                logging.info("Removed temporary file successfully.")
            else:
                logging.debug(f"Temp file not found for cleanup: {temp_file_path}")

            # Also look for any other temp files with similar names
            try:
                pattern = in_path_obj.stem + "*.temp.*"
                for temp_file in output_dir.glob(pattern):
                    logging.info(f"Found additional temp file: {temp_file}, removing...")
                    os.remove(temp_file)
            except Exception as glob_e:
                logging.warning(f"Error searching for additional temp files: {glob_e}")

        except Exception as cleanup_err:
            logging.error(f"Failed to remove temp files: {cleanup_err}")
    else:
        logging.warning("Cannot determine temp file path for cleanup.")


    # Restore sleep functionality if it was active (Phase 2)
    if hasattr(gui, 'sleep_prevention_active') and gui.sleep_prevention_active:
        allow_sleep_mode()
        logging.info("Sleep prevention disabled after force stop.")
        gui.sleep_prevention_active = False # Ensure state is reset

    # Reset UI and state
    gui.conversion_running = False
    if gui.elapsed_timer_id:
        gui.root.after_cancel(gui.elapsed_timer_id)
        gui.elapsed_timer_id = None

    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Conversion force stopped"))
    update_ui_safely(gui.root, reset_current_file_details, gui) # Use imported function
    update_ui_safely(gui.root, lambda: gui.start_button.config(state="normal"))
    update_ui_safely(gui.root, lambda: gui.stop_button.config(state="disabled"))
    update_ui_safely(gui.root, lambda: gui.force_stop_button.config(state="disabled"))

    # Clean up temp folders
    output_dir_to_clean = gui.output_folder_path if hasattr(gui, 'output_folder_path') and gui.output_folder_path else os.getcwd()
    gui.root.after(500, lambda dir=output_dir_to_clean: schedule_temp_folder_cleanup(dir))
    logging.info("Conversion force stopped.")


def conversion_complete(gui, final_message="Conversion complete"):
    """Handle conversion completion or stopping, and show a combined summary.

    Args:
        gui: The main GUI instance for updating UI elements
        final_message: Message to display in the UI and logs
    """
    # Handle completion actions
    logger.info(f"--- {final_message} ---")
    output_dir_to_clean = gui.output_folder_path if hasattr(gui,'output_folder_path') and gui.output_folder_path else os.getcwd()
    gui.root.after(100, lambda dir=output_dir_to_clean: schedule_temp_folder_cleanup(dir))
    total_duration = time.time() - gui.total_conversion_start_time if hasattr(gui,'total_conversion_start_time') else 0
    logging.info(f"Total time: {format_time(total_duration)}")
    logging.info(f"Processed: {gui.processed_files}, Successful: {gui.successful_conversions}, Errors: {gui.error_count}")
    if gui.vmaf_scores: logging.info(f"VMAF (Avg/Min/Max): {statistics.mean(gui.vmaf_scores):.1f}/{min(gui.vmaf_scores):.1f}/{max(gui.vmaf_scores):.1f}")
    if gui.crf_values: logging.info(f"CRF (Avg/Min/Max): {statistics.mean(gui.crf_values):.1f}/{min(gui.crf_values)}/{max(gui.crf_values)}")
    if gui.size_reductions: logging.info(f"Reduction (Avg/Min/Max): {statistics.mean(gui.size_reductions):.1f}%/{min(gui.size_reductions):.1f}%/{max(gui.size_reductions):.1f}%")
    data_saved_bytes = gui.total_input_bytes_success - gui.total_output_bytes_success
    input_gb_success = gui.total_input_bytes_success / (1024**3)
    time_per_gb = (gui.total_time_success / input_gb_success) if input_gb_success > 0 else 0
    data_saved_str = format_file_size(data_saved_bytes) if data_saved_bytes != 0 else "N/A"
    time_per_gb_str = format_time(time_per_gb) if time_per_gb > 0 else "N/A"
    gui.status_label.config(text=final_message)
    reset_current_file_details(gui) # Use imported function

    # Restore sleep functionality
    if hasattr(gui, 'sleep_prevention_active') and gui.sleep_prevention_active:
        allow_sleep_mode()
        logging.info("Sleep prevention disabled - computer may go to sleep normally")
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

    if gui.processed_files > 0:
        summary_msg = f"{final_message}.\n\n" \
                      f"Files Processed: {gui.processed_files}\n" \
                      f"Successful: {gui.successful_conversions}\n" \
                      f"Errors: {gui.error_count}\n" \
                      f"Total Time: {format_time(total_duration)}\n\n" \
                      f"--- Avg. Stats (Successful Files) ---\n"
        summary_msg += f"VMAF Score: {f'{statistics.mean(gui.vmaf_scores):.1f}' if gui.vmaf_scores else 'N/A'}\n"
        summary_msg += f"CRF Value: {f'{statistics.mean(gui.crf_values):.1f}' if gui.crf_values else 'N/A'}\n"
        summary_msg += f"Size Reduction: {f'{statistics.mean(gui.size_reductions):.1f}%' if gui.size_reductions else 'N/A'}\n\n" \
                       f"--- Overall Performance ---\n" \
                       f"Total Data Saved: {data_saved_str}\n" \
                       f"Avg. Processing Time: {time_per_gb_str} per GB Input\n"
        if gui.error_count > 0:
            summary_msg += f"\nNOTE: {gui.error_count} errors occurred. Check logs for details."
            messagebox.showwarning("Conversion Summary", summary_msg)
        else: messagebox.showinfo("Conversion Summary", summary_msg)


# --- Callback Handler Functions ---

def handle_starting(gui, filename) -> None:
    """Handle the start of processing for a file.

    Args:
        gui: The main GUI instance for updating UI elements
        filename: Name of the file being processed
    """
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text=f"Processing: {filename}"))
    update_ui_safely(gui.root, update_progress_bars, gui, 0, 0) # Use imported function
    logger.info(f"Starting conversion of {anonymize_filename(filename)}")

def handle_file_info(gui, filename, info) -> None:
    """Handle initial file information updates.

    Args:
        gui: The main GUI instance for updating UI elements
        filename: Name of the file being processed
        info: Dictionary containing file information
    """
    # Currently used to update the initial file size display if needed
    if info and 'file_size_mb' in info:
         size_str = format_file_size(info['file_size_mb'] * (1024**2))
         update_ui_safely(gui.root, lambda ss=size_str: gui.orig_size_label.config(text=ss))

def handle_progress(gui, filename: str, info: dict) -> None:
    """Handle progress updates from the conversion process.

    Updates progress bars, statistics displays, and ensures UI responsiveness
    during long-running conversions.

    Args:
        gui: The main GUI instance containing progress indicators
        filename: Name of the file being processed
        info: Dictionary containing progress information
    """
    quality_prog = info.get("progress_quality", 0)
    encoding_prog = info.get("progress_encoding", 0)

    # Update progress bars (using imported function)
    update_ui_safely(gui.root, update_progress_bars, gui, quality_prog, encoding_prog)

    # Update message text
    message = info.get("message", "")
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text=f"Processing: {filename} - {message}"))

    # Logging moved to wrapper for consolidation, keep minimal info log here maybe
    # logging.info(f"Progress update for {filename}: {quality_prog}%/{encoding_prog}%, phase={info.get('phase', '?')}")

    # Make sure we include the original file size for calculations if not present
    if "original_size" not in info and hasattr(gui, 'last_input_size') and gui.last_input_size is not None:
        info["original_size"] = gui.last_input_size
        logging.debug(f"Added original_size to info: {gui.last_input_size}")

    # Update statistics display (ETA, size prediction, etc) (using imported function)
    update_conversion_statistics(gui, info)

    # Force GUI updates during encoding phase to keep UI responsive
    phase = info.get("phase", "crf-search")
    if phase == "encoding":
        def force_update():
            try:
                gui.root.update_idletasks()  # Update pending UI tasks
                # Reduce frequency of this debug message if needed
                # logging.debug("Forced GUI update")
            except Exception as e:
                logging.error(f"Error in forced UI update: {e}")
        update_ui_safely(gui.root, force_update)

def handle_error(gui, filename, error_info) -> None:
    """Handle errors that occur during file processing.

    Args:
        gui: The main GUI instance for updating UI elements
        filename: Name of the file being processed
        error_info: Error information, either as a string or dictionary
    """
    message="Unknown"; error_type="unknown"; details=""
    if isinstance(error_info, dict): message=error_info.get("message","?"); error_type=error_info.get("type","?"); details=error_info.get("details","")
    elif isinstance(error_info, str): message=error_info
    anonymized_name=anonymize_filename(filename); log_msg=f"Error converting {anonymized_name} (Type: {error_type}): {message}"
    if details: log_msg+=f" - Details: {details}"
    logging.error(log_msg)
    if isinstance(error_info, dict) and "stack_trace" in error_info: logging.error(f"Stack Trace:\n{error_info['stack_trace']}")
    if hasattr(gui,'error_count'): gui.error_count+=1
    else: gui.error_count=1
    def update_status():
        total_files=len(gui.video_files) if hasattr(gui,'video_files') and gui.video_files else 0
        base_status=f"Progress: {gui.processed_files}/{total_files} files ({gui.successful_conversions} successful)"
        gui.status_label.config(text=f"{base_status} - {gui.error_count} errors")
    update_ui_safely(gui.root, update_status)

def handle_retrying(gui, filename, info) -> None:
    """Handle retry attempts with fallback VMAF targets.

    Args:
        gui: The main GUI instance for updating UI elements
        filename: Name of the file being processed
        info: Dictionary containing retry information including fallback VMAF target
    """
    message="Retrying"; log_details=""; current_target_vmaf=None
    if isinstance(info, dict):
        message=info.get("message", message)
        if "fallback_vmaf" in info: # This key now holds the target being attempted
            current_target_vmaf = info['fallback_vmaf']
            log_details=f" (Target VMAF: {current_target_vmaf})"
    logging.info(f"{message} for {anonymize_filename(filename)}{log_details}")
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text=f"Processing: {filename} - {message}"))
    if current_target_vmaf: update_ui_safely(gui.root, lambda v=current_target_vmaf: gui.vmaf_label.config(text=f"{v} (Target)"))

def handle_completed(gui, filename, info) -> None:
    """Handle successful completion of a file conversion and update stats.

    Args:
        gui: The main GUI instance for updating UI elements
        filename: Name of the file being processed
        info: Dictionary containing completion information such as VMAF and CRF values
    """
    anonymized_name = anonymize_filename(filename)
    log_msg = f"Successfully converted {anonymized_name}"

    # Extract values from info or from gui attributes
    vmaf_value = info.get("vmaf") if isinstance(info, dict) else None
    crf_value = info.get("crf") if isinstance(info, dict) else None
    original_size = getattr(gui, 'last_input_size', None)
    # Get final output size directly from info if available (more reliable)
    output_size = info.get("output_size") if isinstance(info, dict) else getattr(gui, 'last_output_size', None)
    elapsed_time = getattr(gui, 'last_elapsed_time', None)

    # Update VMAF stats
    if vmaf_value is not None:
        gui.vmaf_scores.append(vmaf_value)
        log_msg += f" - VMAF: {vmaf_value:.1f}"

    # Update CRF stats
    if crf_value is not None:
        gui.crf_values.append(crf_value)
        log_msg += f", CRF: {crf_value}"

    # Update size reduction stats
    if original_size is not None and output_size is not None:
        if original_size > 0:
            try:
                ratio = (output_size / original_size) * 100
                size_reduction = 100.0 - ratio

                # Add to the list and log
                if not hasattr(gui, 'size_reductions'):
                    gui.size_reductions = []
                gui.size_reductions.append(size_reduction)
                logging.info(f"Added size reduction to stats: {size_reduction:.1f}% (total entries: {len(gui.size_reductions)})")

                log_msg += f", Size: {ratio:.1f}% ({size_reduction:.1f}% reduction)"
                final_size_str = format_file_size(output_size)
                size_str = f"{final_size_str} ({ratio:.1f}%)"
            except (TypeError, ZeroDivisionError) as e:
                 logging.warning(f"Could not calculate size reduction: {e}")
                 size_str = format_file_size(output_size) if output_size is not None else "-"
                 log_msg += f", Size: {size_str}"
        else:
            size_str = format_file_size(output_size) if output_size is not None else "-"
            log_msg += f", Size: {size_str} (Input 0)"

        # Update the final size label
        update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))

        # Update total bytes and time stats
        if elapsed_time is not None:
            try:
                gui.total_input_bytes_success += original_size
                gui.total_output_bytes_success += output_size
                gui.total_time_success += elapsed_time
            except TypeError as e:
                 logging.warning(f"Type error updating totals: {e}")

    # Log completion and update summary statistics (using imported function)
    logging.info(log_msg)
    update_statistics_summary(gui)

def handle_skipped(gui, filename, reason) -> None:
    """Handle skipped files.

    Args:
        gui: The main GUI instance for updating UI elements
        filename: Name of the file being skipped
        reason: Reason why the file was skipped
    """
    logging.info(f"Skipped {anonymize_filename(filename)}: {reason}")


# --- Worker Thread Implementation ---

def sequential_conversion_worker(gui, input_folder, output_folder, overwrite, stop_event,
                                 convert_audio, audio_codec):
    """Process files sequentially, scanning and converting each eligible video.

    This is the main worker function that runs in a separate thread to handle
    the entire conversion process from scanning to processing.

    Args:
        gui: The main GUI instance with UI elements and conversion state
        input_folder: Root folder to scan for video files
        output_folder: Destination folder for converted files
        overwrite: Whether to overwrite existing output files
        stop_event: Threading event to signal stopping
        convert_audio: Whether to convert audio to a different codec
        audio_codec: Target audio codec if conversion is enabled
    """
    gui.output_folder_path = output_folder
    logger.info(f"Worker started. Input: '{input_folder}', Output: '{output_folder}'")
    gui.error_count = 0; gui.total_input_bytes_success = 0; gui.total_output_bytes_success = 0; gui.total_time_success = 0
    video_info_cache = {} # Initialize cache for this run

    extensions = [ext for ext, var in [("mp4",gui.ext_mp4),("mkv",gui.ext_mkv),("avi",gui.ext_avi),("wmv",gui.ext_wmv)] if var.get()]
    if not extensions: logger.error("Worker: No extensions selected."); update_ui_safely(gui.root, lambda: conversion_complete(gui, "Error: No extensions")); return
    logger.info(f"Worker: Processing extensions: {', '.join(extensions)}")

    # --- File Scanning (Quick Count) ---
    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Searching for files..."))
    all_video_files_set = set()
    try:
        input_path_obj = Path(input_folder)
        if not input_path_obj.is_dir(): raise FileNotFoundError(f"Input folder invalid: {input_folder}")
        for ext in extensions:
            for pattern in [f"*.{ext}", f"*.{ext.upper()}"]:
                 for file_path in input_path_obj.rglob(pattern):
                     if file_path.is_file(): all_video_files_set.add(str(file_path.resolve()))
    except Exception as scan_error: logger.error(f"Scan error: {scan_error}", exc_info=True); update_ui_safely(gui.root, lambda err=scan_error: conversion_complete(gui, f"Error scanning files: {err}")); return

    all_video_files = sorted(list(all_video_files_set)); total_files_found = len(all_video_files)
    logger.info(f"Found {total_files_found} potential files.")
    if not all_video_files: logger.info("No matching files found."); update_ui_safely(gui.root, lambda: conversion_complete(gui, "No matching files found")); return

    # --- Detailed Scan & Eligibility Check (with progress) ---
    files_to_process = []; skipped_files_count = 0
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0)) # Reset progress bar
    update_ui_safely(gui.root, lambda t=total_files_found: gui.status_label.config(text=f"Found {t} potential files. Analyzing eligibility..."))

    for i, video_path in enumerate(all_video_files):
        if stop_event.is_set(): logger.info("Scan interrupted by user."); update_ui_safely(gui.root, lambda: conversion_complete(gui, "Scan stopped")); return

        # Update UI for scan progress
        scan_progress = (i + 1) / total_files_found * 100
        update_ui_safely(gui.root, lambda p=scan_progress: gui.overall_progress.config(value=p))
        update_ui_safely(gui.root, lambda i=i, t=total_files_found: gui.status_label.config(text=f"Analyzing file {i+1}/{t}..."))

        try:
            # Pass the cache dictionary to the scanning function
            needs_conversion, reason = scan_video_needs_conversion(gui, video_path, output_folder, overwrite, video_info_cache)
            if needs_conversion:
                files_to_process.append(video_path)
            else:
                skipped_files_count += 1
                # Log reason if not skipped due to in-place conversion warning (which is logged separately)
                if reason != "In-place MKV conversion requires Overwrite":
                    handle_skipped(gui, os.path.basename(video_path), reason) # Use callback for consistency
        except Exception as e:
            logger.error(f"Unexpected error during detailed scan for {anonymize_filename(video_path)}: {e}", exc_info=True)
            skipped_files_count += 1
            logger.info(f"Skipping {anonymize_filename(video_path)} due to scan error.")
            # Potentially add to error count or trigger handle_error callback if desired

    gui.video_files = files_to_process; total_videos_to_process = len(files_to_process)
    logger.info(f"Scan complete: {total_videos_to_process} files to convert, {skipped_files_count} skipped.")

    # --- Transition to Conversion Phase ---
    if not files_to_process:
        logger.info("No files need conversion."); update_ui_safely(gui.root, lambda: conversion_complete(gui, "No files need conversion")); return

    logger.info(f"Starting conversion of {total_videos_to_process} files...")
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0)) # Reset progress bar for conversion phase
    update_ui_safely(gui.root, lambda t=total_videos_to_process: gui.status_label.config(text=f"Starting conversion of {t} files..."))

    gui.processed_files = 0; gui.successful_conversions = 0

    # Create callback dispatcher function
    def file_callback_dispatcher(filename, status, info=None):
        """Dispatch file status events to appropriate handler functions.

        Args:
            filename: Name of the file being processed
            status: Status code indicating the event type (starting, progress, error, etc.)
            info: Optional dictionary with additional information about the event
        """
        logging.debug(f"Callback: File={filename}, Status={status}, Info={info}")
        try:
            if status == "starting": handle_starting(gui, filename)
            elif status == "file_info": handle_file_info(gui, filename, info)
            elif status == "progress": handle_progress(gui, filename, info)
            elif status == "warning" or status == "error" or status == "failed": handle_error(gui, filename, info if info else status)
            elif status == "retrying": handle_retrying(gui, filename, info)
            elif status == "completed": handle_completed(gui, filename, info)
            elif status == "skipped": handle_skipped(gui, filename, info if info else "Skipped")
            else: logging.warning(f"Unknown status '{status}' for {filename}")
        except Exception as e: logging.error(f"Error in callback dispatcher '{status}': {e}", exc_info=True)

    # --- File Processing Loop ---
    for video_path in files_to_process:
        if stop_event.is_set(): logger.info("Conversion loop interrupted by user."); break
        file_number = gui.processed_files + 1; filename = os.path.basename(video_path); anonymized_name = anonymize_filename(video_path)
        update_ui_safely(gui.root, lambda: gui.status_label.config(text=f"Converting {file_number}/{total_videos_to_process}: {filename}"))
        update_ui_safely(gui.root, reset_current_file_details, gui) # Use imported function

        original_size = 0; input_vcodec = "?"; input_acodec = "?"; input_duration = 0.0 # Default to float
        output_acodec = "?" # Initialize output audio codec

        # --- Use Cached Video Info ---
        video_info = None
        if video_path in video_info_cache:
            video_info = video_info_cache[video_path]
        else:
            # This *shouldn't* happen if scan phase worked, but handle defensively
            logging.warning(f"Cache miss during processing phase for {anonymized_name}. Calling get_video_info again.")
            try:
                video_info = get_video_info(video_path)
                if video_info: video_info_cache[video_path] = video_info # Update cache
            except Exception as e:
                logging.error(f"Failed to get video info during processing for {anonymized_name}: {e}")
                video_info = None # Ensure it's None

        # --- Extract Info & Update UI ---
        if video_info:
            try:
                format_info_text = "-"; size_str = "-"
                for stream in video_info.get("streams", []):
                    if stream.get("codec_type") == "video": input_vcodec = stream.get("codec_name", "?").upper()
                    elif stream.get("codec_type") == "audio": input_acodec = stream.get("codec_name", "?").upper()
                format_info_text = f"{input_vcodec} / {input_acodec}"
                original_size = video_info.get('file_size', 0); size_str = format_file_size(original_size)
                # Ensure duration is extracted correctly as float
                duration_str = video_info.get('format', {}).get('duration', '0')
                try:
                    input_duration = float(duration_str)
                except (ValueError, TypeError):
                    logging.warning(f"Could not convert duration '{duration_str}' to float for {anonymized_name}. Using 0.")
                    input_duration = 0.0

                update_ui_safely(gui.root, lambda fi=format_info_text: gui.orig_format_label.config(text=fi))
                update_ui_safely(gui.root, lambda ss=size_str: gui.orig_size_label.config(text=ss))
                # Store for later use in handle_progress/handle_completed
                gui.last_input_size = original_size
                output_acodec = input_acodec # Set default output codec
            except (ValueError, TypeError, KeyError) as e:
                logging.error(f"Error extracting info from cached/retrieved data for {anonymized_name}: {e}")
                gui.last_input_size = None # Ensure reset on error
                input_duration = 0.0 # Reset duration on error
        else:
            logging.warning(f"Cannot get pre-conversion info for {anonymized_name}.")
            gui.last_input_size = None # Ensure reset if info failed completely
            input_duration = 0.0 # Reset duration if info failed

        gui.current_file_start_time = time.time(); gui.current_file_encoding_start_time = None
        update_ui_safely(gui.root, update_elapsed_time, gui, gui.current_file_start_time) # Use imported function
        gui.current_process_info = None

        # --- Process Video ---
        process_successful = False; output_file_path = None; elapsed_time_file = 0; output_size = 0; final_crf = None; final_vmaf = None; final_vmaf_target = None
        try:
            result_tuple = process_video(
                video_path, input_folder, output_folder, overwrite, convert_audio, audio_codec,
                file_info_callback=file_callback_dispatcher, # Pass the dispatcher here
                pid_callback=lambda pid, path=video_path: store_process_id(gui, pid, path),
                total_duration_seconds=input_duration # Pass duration here
            )
            if result_tuple:
                 output_file_path, elapsed_time_file, _, output_size, final_crf, final_vmaf, final_vmaf_target = result_tuple # Unpack extended tuple
                 process_successful = True
                 # Store actual output size and elapsed time for handle_completed
                 gui.last_output_size = output_size
                 gui.last_elapsed_time = elapsed_time_file
                 # Determine final audio codec based on conversion settings
                 if convert_audio and input_acodec.lower() not in ['aac', 'opus']: output_acodec = audio_codec.lower()
                 # else: output_acodec remains input_acodec (set earlier)
            else:
                 # Ensure these are reset if process_video returns None (indicating failure reported via callback)
                 gui.last_output_size = None
                 gui.last_elapsed_time = None
                 process_successful = False # Explicitly set

        except Exception as e:
            logging.error(f"Critical error in process_video for {anonymized_name}: {e}", exc_info=True);
            file_callback_dispatcher(filename, "failed", {"message": f"Internal error: {e}", "type": "processing_error"});
            process_successful = False
            gui.last_output_size = None
            gui.last_elapsed_time = None


        # --- Post-processing & History ---
        gui.processed_files += 1
        if process_successful:
            gui.successful_conversions += 1
            try: # Append to History
                anonymize_hist = gui.anonymize_history.get()
                # Use anonymize_filename which handles None check now
                input_path_for_hist = anonymize_filename(video_path) if anonymize_hist else video_path
                output_path_for_hist = anonymize_filename(output_file_path) if anonymize_hist else output_file_path
                hist_record = {
                    "timestamp": datetime.datetime.now().isoformat(sep=' ', timespec='seconds'),
                    "input_file": input_path_for_hist,
                    "output_file": output_path_for_hist,
                    "input_size_mb": round(original_size / (1024**2), 2) if original_size is not None else None,
                    "output_size_mb": round(output_size / (1024**2), 2) if output_size is not None else None,
                    "reduction_percent": round(100 - (output_size / original_size * 100), 1) if original_size and output_size and original_size > 0 else None,
                    "duration_sec": round(input_duration, 1) if input_duration is not None else None,
                    "time_sec": round(elapsed_time_file, 1) if elapsed_time_file is not None else None,
                    "input_vcodec": input_vcodec, "input_acodec": input_acodec, "output_acodec": output_acodec,
                    "final_crf": final_crf,
                    "final_vmaf": round(final_vmaf, 2) if final_vmaf is not None else None,
                    "final_vmaf_target": final_vmaf_target
                }
                append_to_history(hist_record)
            except Exception as hist_e: logger.error(f"Failed to append to history for {anonymized_name}: {hist_e}")
            # Call the "completed" callback *after* history is appended, ensuring all data is ready
            # Pass final output size to callback handler
            completed_info = {"vmaf": final_vmaf, "crf": final_crf, "output_size": output_size}
            file_callback_dispatcher(filename, "completed", completed_info)
        else:
            # If not successful, ensure the error was logged via handle_error called from dispatcher
            pass


        if gui.elapsed_timer_id:
             # The timer should cancel itself when stop event is set or conversion_running is false
             pass
        # Update overall progress based on processed files / total needing processing
        overall_progress_percent = (gui.processed_files / total_videos_to_process) * 100
        update_ui_safely(gui.root, lambda p=overall_progress_percent: gui.overall_progress.config(value=p))
        def update_final_status(): # Update overall status label
            base_status=f"Progress: {gui.processed_files}/{total_videos_to_process} files ({gui.successful_conversions} successful)"
            error_suffix=f" - {gui.error_count} errors" if gui.error_count > 0 else ""; gui.status_label.config(text=f"{base_status}{error_suffix}")
        update_ui_safely(gui.root, update_final_status)

    # --- End of Processing Loop ---
    final_status_message = "Conversion complete";
    if stop_event.is_set(): final_status_message = "Conversion stopped by user"
    elif gui.error_count > 0: final_status_message = f"Conversion complete with {gui.error_count} errors"
    logger.info(f"Worker finished. Status: {final_status_message}")
    update_ui_safely(gui.root, lambda msg=final_status_message: conversion_complete(gui, msg))