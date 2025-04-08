"""
GUI operation functions for the AV1 Video Converter application.
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
from tkinter import filedialog, messagebox
import tkinter as tk # For type hinting if needed

# Project imports
from convert_app.ab_av1_wrapper import check_ab_av1_available, clean_ab_av1_temp_folders
from convert_app.utils import (
    get_video_info, format_time, format_file_size, anonymize_filename,
    check_ffmpeg_availability, update_ui_safely, DEFAULT_VMAF_TARGET,
    DEFAULT_ENCODING_PRESET, append_to_history, get_history_file_path # Import history func
)
from convert_app.video_conversion import process_video

logger = logging.getLogger(__name__)


def browse_input_folder(gui):
    """Open file dialog to choose input folder"""
    folder = filedialog.askdirectory(title="Select Input Folder")
    if folder:
        gui.input_folder.set(folder)
        logger.info(f"Input folder selected: {folder}")
        if not gui.output_folder.get():
            gui.output_folder.set(folder)
            logger.info(f"Output folder automatically set to match input: {folder}")

def browse_output_folder(gui):
    """Open file dialog to choose output folder"""
    folder = filedialog.askdirectory(title="Select Output Folder")
    if folder:
        gui.output_folder.set(folder)
        logger.info(f"Output folder selected: {folder}")

def browse_log_folder(gui):
    """Open file dialog to choose log folder"""
    folder = filedialog.askdirectory(title="Select Log Folder")
    if folder:
        gui.log_folder.set(folder)
        logger.info(f"Log folder selected by user: {folder}")
        messagebox.showinfo("Log Folder Changed", "Log folder preference updated.\nNew logs will be written to the selected folder on next application start.")

def open_log_folder_action(gui):
    """Open the currently used log folder in the file explorer"""
    try:
        # Use the log_directory attribute stored on the GUI object after setup_logging runs
        log_dir = gui.log_directory
        if log_dir and os.path.isdir(log_dir):
            logger.info(f"Opening log folder: {log_dir}")
            if sys.platform == "win32":
                os.startfile(log_dir)
            elif sys.platform == "darwin": # macOS
                subprocess.run(["open", log_dir], check=True)
            else: # Linux and other POSIX
                subprocess.run(["xdg-open", log_dir], check=True)
        else:
            logger.warning("Log directory not set or invalid, cannot open.")
            messagebox.showwarning("Cannot Open Log Folder", "The log folder location is not set or is invalid.")
    except Exception as e:
        log_dir_str = gui.log_directory if hasattr(gui, 'log_directory') else "N/A"
        logger.error(f"Failed to open log folder '{log_dir_str}': {e}")
        messagebox.showerror("Error", f"Could not open log folder:\n{e}")

def open_history_file_action(gui):
    """Open the conversion history json file"""
    history_path = get_history_file_path()
    try:
        if os.path.exists(history_path):
            logger.info(f"Opening history file: {history_path}")
            if sys.platform == "win32":
                os.startfile(history_path)
            elif sys.platform == "darwin":
                subprocess.run(["open", "-t", history_path], check=True) # Open with default text editor
            else:
                # Try common text editors first, then xdg-open
                editors = ["gedit", "kate", "mousepad", "xdg-open"]
                opened = False
                for editor in editors:
                    try:
                        # Use Popen to avoid blocking if editor needs setup
                        subprocess.Popen([editor, history_path])
                        opened = True
                        break
                    except FileNotFoundError:
                        continue
                    except Exception as editor_e: # Catch other potential errors
                        logger.warning(f"Error trying editor {editor}: {editor_e}")
                        continue # Try next editor
                if not opened:
                    logger.warning("Could not find common text editor, falling back to xdg-open.")
                    subprocess.run(["xdg-open", history_path], check=True)
        else:
            logger.info("History file does not exist yet.")
            messagebox.showinfo("History File Not Found", f"The history file ({os.path.basename(history_path)}) has not been created yet.\nIt will be created after the first successful conversion.")
    except Exception as e:
        logger.error(f"Failed to open history file '{history_path}': {e}")
        messagebox.showerror("Error", f"Could not open history file:\n{e}")


def check_ffmpeg(gui):
    """Check if FFmpeg is installed and has SVT-AV1 support, and check for ab-av1"""
    # (Unchanged from previous correction)
    logger.info("Checking FFmpeg and ab-av1 installation...")
    ffmpeg_available, svt_av1_available, version_info, error_message = check_ffmpeg_availability()
    if not ffmpeg_available:
        error_msg = f"Error: ffmpeg not found. {error_message}"; logger.error(error_msg)
        messagebox.showerror("FFmpeg Not Found", "FFmpeg not found or not in PATH.\nPlease install FFmpeg."); return False
    if not svt_av1_available:
        warning_msg = "Warning: Your ffmpeg lacks SVT-AV1 support (libsvtav1)." ; logger.warning(warning_msg)
        if not messagebox.askokcancel("SVT-AV1 Support Missing?", f"{warning_msg}\nab-av1 requires this. Continue anyway?"): return False
    else: logger.info("FFmpeg with SVT-AV1 support detected.")
    if version_info: logger.info(f"FFmpeg version: {version_info.splitlines()[0]}")
    available, path, message = check_ab_av1_available()
    if not available: logger.error(f"ab-av1 check failed: {message}"); messagebox.showerror("ab-av1 Not Found", message); return False
    else: logger.info(f"ab-av1 check successful: {message}")
    logger.info("FFmpeg and ab-av1 checks passed."); return True


def update_progress_bars(gui, quality_percent, encoding_percent):
    """Update the dual progress bars in a thread-safe way"""
    # (Unchanged from previous correction)
    def _update_ui():
        quality_prog_widget = getattr(gui, 'quality_progress', None)
        quality_label_widget = getattr(gui, 'quality_percent_label', None)
        encoding_prog_widget = getattr(gui, 'encoding_progress', None)
        encoding_label_widget = getattr(gui, 'encoding_percent_label', None)
        if not all([quality_prog_widget, quality_label_widget, encoding_prog_widget, encoding_label_widget]): return
        try:
            q_mode = 'determinate'; e_mode = 'determinate'
            if encoding_percent <= 0 and quality_percent < 100: q_mode = 'indeterminate'
            quality_prog_widget.config(value=quality_percent, mode=q_mode)
            quality_label_widget.config(text=f"{math.ceil(quality_percent)}%")
            encoding_prog_widget.config(value=encoding_percent, mode=e_mode)
            encoding_label_widget.config(text=f"{math.ceil(encoding_percent)}%")
        except tk.TclError as e: logger.debug(f"TclError updating progress bars: {e}")
    update_ui_safely(gui.root, _update_ui)


def scan_video_needs_conversion(gui, video_path, output_folder_path, overwrite=False):
    """Scan a video file to determine if it needs conversion"""
    # (Unchanged from previous correction)
    anonymized_input = anonymize_filename(video_path)
    try:
        input_path_obj = Path(video_path); input_folder_obj = Path(gui.input_folder.get()); output_folder_obj = Path(output_folder_path)
        relative_dir = input_path_obj.parent.relative_to(input_folder_obj); output_dir = output_folder_obj / relative_dir
        output_filename = input_path_obj.stem + ".mkv"; output_path = output_dir / output_filename
        anonymized_output = anonymize_filename(str(output_path))
    except ValueError:
        input_path_obj = Path(video_path); output_folder_obj = Path(output_folder_path)
        output_filename = input_path_obj.stem + ".mkv"; output_path = output_folder_obj / output_filename
        anonymized_output = anonymize_filename(str(output_path))
        logging.debug(f"File {anonymized_input} not relative, using direct output: {output_path}")
    except Exception as e: logging.error(f"Error determining output path for {anonymized_input}: {e}"); return True, "Error determining output path"
    if os.path.exists(output_path) and not overwrite: logging.info(f"Skipping {anonymized_input} - output exists: {anonymized_output}"); return False, "Output file exists"
    try:
        video_info = get_video_info(video_path)
        if not video_info: logging.warning(f"Cannot analyze {anonymized_input} - will attempt"); return True, "Analysis failed"
        is_already_av1 = False; video_stream_found = False
        for stream in video_info.get("streams", []):
            if stream.get("codec_type") == "video":
                video_stream_found = True
                if stream.get("codec_name", "").lower() == "av1": is_already_av1 = True; break
        if not video_stream_found: logging.warning(f"No video stream in {anonymized_input} - skipping."); return False, "No video stream found"
        is_mkv_container = video_path.lower().endswith(".mkv")
        if is_already_av1 and is_mkv_container: logging.info(f"Skipping {anonymized_input} - already AV1/MKV"); return False, "Already AV1/MKV"
        return True, "Needs conversion"
    except Exception as e: logging.error(f"Error checking file {anonymized_input}: {str(e)}"); return True, f"Error during check: {str(e)}"


def update_conversion_statistics(gui, info=None):
    """Update the conversion statistics like ETA, VMAF, CRF"""
    # (Unchanged from previous correction)
    if not info or not gui.conversion_running: return
    if "vmaf" in info and info["vmaf"] is not None:
        vmaf_status = f"{info['vmaf']:.1f}"
        if info.get("phase") == "crf-search": vmaf_status += " (Current)"
        if info.get("used_fallback"): vmaf_status += " (Fallback Used)" # Check if fallback was used
        update_ui_safely(gui.root, lambda v=vmaf_status: gui.vmaf_label.config(text=v))
    elif info.get("phase") == "crf-search": update_ui_safely(gui.root, lambda: gui.vmaf_label.config(text=f"{DEFAULT_VMAF_TARGET} (Target)"))
    if "crf" in info and info["crf"] is not None:
        settings_text = f"CRF: {info['crf']}, Preset: {DEFAULT_ENCODING_PRESET}"; update_ui_safely(gui.root, lambda: gui.encoding_settings_label.config(text=settings_text))
    encoding_prog = info.get("progress_encoding", 0)
    if encoding_prog > 0:
        if hasattr(gui, 'current_file_start_time') and gui.current_file_start_time:
             if not hasattr(gui, 'current_file_encoding_start_time') or not gui.current_file_encoding_start_time: gui.current_file_encoding_start_time = time.time()
             elapsed_encoding_time = time.time() - gui.current_file_encoding_start_time
             if encoding_prog > 1 and elapsed_encoding_time > 1:
                 try: total_encoding_time_est = (elapsed_encoding_time / encoding_prog) * 100; eta_seconds = total_encoding_time_est - elapsed_encoding_time; eta_str = format_time(eta_seconds); update_ui_safely(gui.root, lambda: gui.eta_label.config(text=eta_str))
                 except ZeroDivisionError: update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
             else: update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Calculating..."))
    elif info.get("phase") == "crf-search": update_ui_safely(gui.root, lambda: gui.eta_label.config(text="Detecting..."))
    else: update_ui_safely(gui.root, lambda: gui.eta_label.config(text="-"))
    if "output_size" in info and "original_size" in info:
        current_size = info["output_size"]; original_size = info["original_size"]
        if original_size is not None and original_size > 0 and current_size is not None:
            ratio = (current_size / original_size) * 100; size_str = f"{format_file_size(current_size)} ({ratio:.1f}%)"; update_ui_safely(gui.root, lambda: gui.output_size_label.config(text=size_str))


def store_process_id(gui, pid, input_path):
    """Store the current process ID and its associated input file"""
    # (Unchanged from previous correction)
    gui.current_process_info = {"pid": pid, "input_path": input_path}
    logger.info(f"ab-av1 process started with PID: {pid} for file {anonymize_filename(input_path)}")


def start_conversion(gui):
    """Start the conversion process"""
    # (Unchanged from previous correction)
    if gui.conversion_running: logger.warning("Start clicked while running."); return
    input_folder = gui.input_folder.get(); output_folder = gui.output_folder.get()
    if not input_folder or not os.path.isdir(input_folder): messagebox.showerror("Error", "Invalid input folder"); return
    if not output_folder: output_folder = input_folder; gui.output_folder.set(output_folder); logger.info(f"Output set to input: {output_folder}")
    try: os.makedirs(output_folder, exist_ok=True)
    except Exception as e: logger.error(f"Cannot create output folder '{output_folder}': {e}"); messagebox.showerror("Error", f"Cannot create output folder:\n{e}"); return
    selected_extensions = [ext for ext, var in [("mp4", gui.ext_mp4), ("mkv", gui.ext_mkv), ("avi", gui.ext_avi), ("wmv", gui.ext_wmv)] if var.get()]
    if not selected_extensions: messagebox.showerror("Error", "Select file extensions in Settings"); return
    if not check_ffmpeg(gui): return

    overwrite = gui.overwrite.get(); convert_audio = gui.convert_audio.get(); audio_codec = gui.audio_codec.get()
    logger.info("--- Starting Conversion ---")
    logger.info(f"Input: {input_folder}, Output: {output_folder}, Extensions: {', '.join(selected_extensions)}")
    logger.info(f"Overwrite: {overwrite}, Convert Audio: {convert_audio} (Codec: {audio_codec if convert_audio else 'N/A'})")
    logger.info(f"Using -> Preset: {DEFAULT_ENCODING_PRESET}, VMAF Target: {DEFAULT_VMAF_TARGET}")

    gui.status_label.config(text="Starting..."); logging.info("Preparing conversion...")
    gui.start_button.config(state="disabled"); gui.stop_button.config(state="normal"); gui.force_stop_button.config(state="normal")
    gui.conversion_running = True; gui.stop_event = threading.Event(); gui.total_conversion_start_time = time.time()
    gui.vmaf_scores = []; gui.crf_values = []; gui.size_reductions = []
    gui.processed_files = 0; gui.successful_conversions = 0; gui.error_count = 0
    gui.current_process_info = None; gui.total_input_bytes_success = 0; gui.total_output_bytes_success = 0; gui.total_time_success = 0

    update_statistics_summary(gui); reset_current_file_details(gui)
    gui.conversion_thread = threading.Thread(target=sequential_conversion_worker, args=(gui, input_folder, output_folder, overwrite, gui.stop_event, convert_audio, audio_codec), daemon=True)
    gui.conversion_thread.start()


def stop_conversion(gui):
    """Stop the conversion process gracefully after the current file finishes"""
    # (Unchanged from previous correction)
    if gui.conversion_running and gui.stop_event and not gui.stop_event.is_set():
        gui.status_label.config(text="Stopping... (after current file)")
        logging.info("Graceful stop requested (Stop After Current File). Signalling worker.")
        gui.stop_event.set(); gui.stop_button.config(state="disabled")
    elif not gui.conversion_running: logging.info("Stop requested but not running.")
    elif gui.stop_event and gui.stop_event.is_set(): logging.info("Stop already requested.")


def force_stop_conversion(gui, confirm=True):
    """Force stop the conversion process immediately by killing the process and cleaning temp file"""
    # (Unchanged from previous correction)
    if not gui.conversion_running: logging.info("Force stop requested but not running."); return
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

    if pid_to_kill:
        logging.info(f"Attempting to terminate process PID {pid_to_kill}...")
        try:
            startupinfo = None; # Hide console window for taskkill
            if os.name == 'nt': startupinfo = subprocess.STARTUPINFO(); startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW; startupinfo.wShowWindow = subprocess.SW_HIDE
            if sys.platform == "win32":
                result = subprocess.run(["taskkill", "/F", "/PID", str(pid_to_kill)], capture_output=True, text=True, check=False, startupinfo=startupinfo)
                if result.returncode == 0: logging.info(f"Terminated PID {pid_to_kill} via taskkill.")
                else: logging.warning(f"taskkill failed for PID {pid_to_kill} (rc={result.returncode}, maybe exited?): {result.stderr.strip()}")
            else: os.kill(pid_to_kill, signal.SIGKILL); logging.info(f"Sent SIGKILL to PID {pid_to_kill}.")
        except ProcessLookupError: logging.warning(f"PID {pid_to_kill} not found.")
        except Exception as e: logging.error(f"Failed to terminate PID {pid_to_kill}: {str(e)}")

        if input_path_killed and gui.output_folder.get():
             try:
                 in_path_obj = Path(input_path_killed); out_folder_obj = Path(gui.output_folder.get()); relative_dir = Path(".")
                 try: relative_dir = in_path_obj.parent.relative_to(Path(gui.input_folder.get()))
                 except: logging.debug("Killed file not relative to input base.")
                 output_dir = out_folder_obj / relative_dir; temp_filename = in_path_obj.stem + ".mkv.temp.mkv"; temp_file_path = output_dir / temp_filename
                 if temp_file_path.exists():
                      logging.info(f"Removing temporary file: {temp_file_path}")
                      os.remove(temp_file_path); logging.info(f"Removed temporary file.")
                 else: logging.debug(f"Temp file not found for cleanup: {temp_file_path}")
             except Exception as cleanup_err: logging.error(f"Failed to remove temp file {temp_filename}: {cleanup_err}")
        else: logging.warning("Cannot determine temp file path for cleanup.")
    else: logging.warning("Force stop: No active process PID recorded.")

    gui.conversion_running = False
    if gui.elapsed_timer_id: gui.root.after_cancel(gui.elapsed_timer_id); gui.elapsed_timer_id = None
    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Conversion force stopped"))
    update_ui_safely(gui.root, reset_current_file_details, gui)
    update_ui_safely(gui.root, lambda: gui.start_button.config(state="normal"))
    update_ui_safely(gui.root, lambda: gui.stop_button.config(state="disabled"))
    update_ui_safely(gui.root, lambda: gui.force_stop_button.config(state="disabled"))

    output_dir_to_clean = gui.output_folder_path if hasattr(gui, 'output_folder_path') and gui.output_folder_path else os.getcwd()
    gui.root.after(500, lambda dir=output_dir_to_clean: schedule_temp_folder_cleanup(dir))
    logging.info("Conversion force stopped.")


def schedule_temp_folder_cleanup(directory):
    """Schedule cleanup of general temp folders"""
    # (Unchanged from previous correction)
    try:
        logging.info(f"Scheduling cleanup of temp folders in: {directory}")
        cleaned_count = clean_ab_av1_temp_folders(directory)
        if cleaned_count > 0: logging.info(f"Cleaned up {cleaned_count} temp folders in {directory}.")
        else: logging.debug(f"No temp folders found to clean in {directory}.")
    except Exception as e: logging.warning(f"Could not clean up temp folders in {directory}: {str(e)}")


def update_elapsed_time(gui, start_time):
    """Update the elapsed time label"""
    # (Unchanged from previous correction)
    if not gui.conversion_running or (gui.stop_event and gui.stop_event.is_set()): gui.elapsed_timer_id = None; return
    elapsed = time.time() - start_time
    update_ui_safely(gui.root, lambda: gui.elapsed_label.config(text=format_time(elapsed)))
    gui.elapsed_timer_id = gui.root.after(1000, lambda: update_elapsed_time(gui, start_time))


def update_statistics_summary(gui):
    """Update the overall statistics summary labels"""
    # (Syntax corrected)
    vmaf_text = "-"
    crf_text = "-"
    reduction_text = "-"

    if gui.vmaf_scores:
        try:
            avg_vmaf = statistics.mean(gui.vmaf_scores)
            min_vmaf = min(gui.vmaf_scores)
            max_vmaf = max(gui.vmaf_scores)
            vmaf_text = f"Avg: {avg_vmaf:.1f} (Range: {min_vmaf:.1f}-{max_vmaf:.1f})"
        except Exception as e:
            logging.warning(f"Error calculating VMAF stats: {e}")
            vmaf_text = "Error"

    if gui.crf_values:
         try:
            avg_crf = statistics.mean(gui.crf_values)
            min_crf = min(gui.crf_values)
            max_crf = max(gui.crf_values)
            crf_text = f"Avg: {avg_crf:.1f} (Range: {min_crf}-{max_crf})"
         except Exception as e:
            logging.warning(f"Error calculating CRF stats: {e}")
            crf_text = "Error"

    if gui.size_reductions:
        try:
            avg_reduction = statistics.mean(gui.size_reductions)
            min_reduction = min(gui.size_reductions)
            max_reduction = max(gui.size_reductions)
            reduction_text = f"Avg: {avg_reduction:.1f}% (Range: {min_reduction:.1f}%-{max_reduction:.1f}%)"
        except Exception as e:
            logging.warning(f"Error calculating Size Reduction stats: {e}")
            reduction_text = "Error"

    update_ui_safely(gui.root, lambda: gui.vmaf_stats_label.config(text=vmaf_text))
    update_ui_safely(gui.root, lambda: gui.crf_stats_label.config(text=crf_text))
    update_ui_safely(gui.root, lambda: gui.size_stats_label.config(text=reduction_text))


def reset_current_file_details(gui):
    """Reset labels related to the currently processing file"""
    # (Unchanged from previous correction)
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text="No file processing"))
    update_ui_safely(gui.root, update_progress_bars, gui, 0, 0)
    update_ui_safely(gui.root, lambda: gui.orig_format_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.orig_size_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.vmaf_label.config(text=f"{DEFAULT_VMAF_TARGET} (Target)"))
    update_ui_safely(gui.root, lambda: gui.elapsed_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.eta_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.output_size_label.config(text="-"))
    update_ui_safely(gui.root, lambda: gui.encoding_settings_label.config(text="-"))
    gui.current_file_encoding_start_time = None


# --- File Callback Handlers ---

def _handle_starting(gui, filename):
    # (Unchanged from previous correction)
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text=f"Processing: {filename}"))
    update_ui_safely(gui.root, update_progress_bars, gui, 0, 0)
    logger.info(f"Starting conversion of {anonymize_filename(filename)}")

def _handle_file_info(gui, filename, info): pass

def _handle_progress(gui, filename, info):
    # (Unchanged from previous correction)
    quality_prog = info.get("progress_quality", 0); encoding_prog = info.get("progress_encoding", 0)
    update_ui_safely(gui.root, update_progress_bars, gui, quality_prog, encoding_prog)
    message = info.get("message", ""); update_ui_safely(gui.root, lambda: gui.current_file_label.config(text=f"Processing: {filename} - {message}"))
    update_conversion_statistics(gui, info)
    phase = info.get("phase", "crf-search")
    if phase == "encoding" and encoding_prog > 98: logger.debug(f"Encoding nearing completion for {anonymize_filename(filename)} ({encoding_prog:.1f}%)")

def _handle_error(gui, filename, error_info):
    # (Unchanged from previous correction)
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

def _handle_retrying(gui, filename, info):
    """Handles retry attempts with fallback VMAF"""
    # (Updated to show current target)
    message="Retrying"; log_details=""; current_target_vmaf=None
    if isinstance(info, dict):
        message=info.get("message", message)
        if "fallback_vmaf" in info: # This key now holds the target being attempted
            current_target_vmaf = info['fallback_vmaf']
            log_details=f" (Target VMAF: {current_target_vmaf})"
    logging.info(f"{message} for {anonymize_filename(filename)}{log_details}")
    update_ui_safely(gui.root, lambda: gui.current_file_label.config(text=f"Processing: {filename} - {message}"))
    if current_target_vmaf: update_ui_safely(gui.root, lambda v=current_target_vmaf: gui.vmaf_label.config(text=f"{v} (Target)"))

def _handle_completed(gui, filename, info):
    """Handles successful completion, updates stats"""
    # (Unchanged from previous correction)
    anonymized_name = anonymize_filename(filename)
    log_msg = f"Successfully converted {anonymized_name}"
    vmaf_value = info.get("vmaf") if isinstance(info, dict) else None
    crf_value = info.get("crf") if isinstance(info, dict) else None
    original_size = getattr(gui, 'last_input_size', None)
    output_size = getattr(gui, 'last_output_size', None)
    elapsed_time = getattr(gui, 'last_elapsed_time', None)
    if vmaf_value is not None: gui.vmaf_scores.append(vmaf_value); log_msg += f" - VMAF: {vmaf_value:.1f}"
    if crf_value is not None: gui.crf_values.append(crf_value); log_msg += f", CRF: {crf_value}"
    if original_size is not None and output_size is not None:
        if original_size > 0:
            ratio = (output_size / original_size) * 100; size_reduction = 100.0 - ratio
            gui.size_reductions.append(size_reduction); log_msg += f", Size: {ratio:.1f}% ({size_reduction:.1f}% reduction)"
        else: log_msg += f", Size: N/A (Input 0)"
        if elapsed_time is not None: gui.total_input_bytes_success += original_size; gui.total_output_bytes_success += output_size; gui.total_time_success += elapsed_time
        final_size_str = format_file_size(output_size); size_str = f"{final_size_str} ({ratio:.1f}%)" if original_size > 0 else final_size_str
        update_ui_safely(gui.root, lambda s=size_str: gui.output_size_label.config(text=s))
    logging.info(log_msg); update_statistics_summary(gui)

def _handle_skipped(gui, filename, reason):
    # (Unchanged from previous correction)
    logging.info(f"Skipped {anonymize_filename(filename)}: {reason}")


# --- Worker Thread ---

def sequential_conversion_worker(gui, input_folder, output_folder, overwrite, stop_event,
                                 convert_audio, audio_codec):
    # (Updated history logic)
    gui.output_folder_path = output_folder
    logger.info(f"Worker started. Input: '{input_folder}', Output: '{output_folder}'")
    gui.error_count = 0; gui.total_input_bytes_success = 0; gui.total_output_bytes_success = 0; gui.total_time_success = 0

    extensions = [ext for ext, var in [("mp4",gui.ext_mp4),("mkv",gui.ext_mkv),("avi",gui.ext_avi),("wmv",gui.ext_wmv)] if var.get()]
    if not extensions: logger.error("Worker: No extensions selected."); update_ui_safely(gui.root, lambda: conversion_complete(gui, "Error: No extensions")); return
    logger.info(f"Worker: Processing extensions: {', '.join(extensions)}")

    # --- File Scanning ---
    update_ui_safely(gui.root, lambda: gui.status_label.config(text="Scanning files..."))
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

    # --- Preliminary Scan ---
    files_to_process = []; skipped_files_count = 0
    update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0))
    for i, video_path in enumerate(all_video_files):
        if stop_event.is_set(): logger.info("Scan interrupted."); update_ui_safely(gui.root, lambda: conversion_complete(gui, "Scan stopped")); return
        scan_progress = (i + 1) / total_files_found * 100
        update_ui_safely(gui.root, lambda p=scan_progress: gui.overall_progress.config(value=p))
        update_ui_safely(gui.root, lambda i=i, t=total_files_found: gui.status_label.config(text=f"Scanning: {i+1}/{t}"))
        try:
            needs_conversion, reason = scan_video_needs_conversion(gui, video_path, output_folder, overwrite)
            if needs_conversion: files_to_process.append(video_path)
            else: skipped_files_count += 1
        except Exception as e: logger.error(f"Scan error for {anonymize_filename(video_path)}: {e}", exc_info=True); skipped_files_count += 1; logger.info(f"Skipping {anonymize_filename(video_path)} due to scan error.")

    gui.video_files = files_to_process; total_videos_to_process = len(files_to_process)
    logger.info(f"Scan complete: {total_videos_to_process} files to convert, {skipped_files_count} skipped.")
    if not files_to_process: logger.info("No files need conversion."); update_ui_safely(gui.root, lambda: conversion_complete(gui, "No files need conversion")); return

    # --- Sequential Processing ---
    logger.info(f"Starting conversion of {total_videos_to_process} files..."); update_ui_safely(gui.root, lambda: gui.overall_progress.config(value=0))
    gui.processed_files = 0; gui.successful_conversions = 0

    # --- Callback Dispatcher ---
    def file_callback_dispatcher(filename, status, info=None):
        # (Unchanged from previous correction)
        logging.debug(f"Callback: File={filename}, Status={status}, Info={info}")
        try:
            if status == "starting": _handle_starting(gui, filename)
            elif status == "file_info": _handle_file_info(gui, filename, info)
            elif status == "progress": _handle_progress(gui, filename, info)
            elif status == "warning" or status == "error" or status == "failed": _handle_error(gui, filename, info if info else status)
            elif status == "retrying": _handle_retrying(gui, filename, info)
            elif status == "completed": _handle_completed(gui, filename, info)
            elif status == "skipped": _handle_skipped(gui, filename, info if info else "Skipped")
            else: logging.warning(f"Unknown status '{status}' for {filename}")
        except Exception as e: logging.error(f"Error in callback dispatcher '{status}': {e}", exc_info=True)

    # --- Loop ---
    for video_path in files_to_process:
        if stop_event.is_set(): logger.info("Conversion loop interrupted."); break
        file_number = gui.processed_files + 1; filename = os.path.basename(video_path); anonymized_name = anonymize_filename(video_path)
        update_ui_safely(gui.root, lambda: gui.status_label.config(text=f"Converting {file_number}/{total_videos_to_process}: {filename}"))
        update_ui_safely(gui.root, reset_current_file_details, gui)

        original_size = 0; input_vcodec = "?"; input_acodec = "?"; input_duration = 0
        try: # Get pre-conversion info
            video_info = get_video_info(video_path)
            if video_info:
                format_info_text = "-"; size_str = "-"
                for stream in video_info.get("streams", []):
                    if stream.get("codec_type") == "video": input_vcodec = stream.get("codec_name", "?").upper()
                    elif stream.get("codec_type") == "audio": input_acodec = stream.get("codec_name", "?").upper()
                format_info_text = f"{input_vcodec} / {input_acodec}"
                original_size = video_info.get('file_size', 0); size_str = format_file_size(original_size)
                try: input_duration = float(video_info.get('format', {}).get('duration', '0'))
                except: input_duration = 0
                update_ui_safely(gui.root, lambda fi=format_info_text: gui.orig_format_label.config(text=fi))
                update_ui_safely(gui.root, lambda ss=size_str: gui.orig_size_label.config(text=ss))
            else: logging.warning(f"Cannot get pre-conversion info for {anonymized_name}.")
        except Exception as e: logging.error(f"Error getting pre-conversion info for {anonymized_name}: {e}")

        gui.current_file_start_time = time.time(); gui.current_file_encoding_start_time = None
        update_ui_safely(gui.root, update_elapsed_time, gui, gui.current_file_start_time)
        gui.current_process_info = None

        # --- Process Video ---
        process_successful = False; output_file_path = None; elapsed_time_file = 0; output_size = 0; final_crf = None; final_vmaf = None; final_vmaf_target = None; output_acodec = input_acodec
        try:
            result_tuple = process_video(video_path, input_folder, output_folder, overwrite, convert_audio, audio_codec, None, file_callback_dispatcher, lambda pid, path=video_path: store_process_id(gui, pid, path))
            if result_tuple:
                 output_file_path, elapsed_time_file, _, output_size, final_crf, final_vmaf, final_vmaf_target = result_tuple # Unpack extended tuple
                 process_successful = True
                 gui.last_input_size = original_size; gui.last_output_size = output_size; gui.last_elapsed_time = elapsed_time_file
                 if convert_audio and input_acodec.lower() not in ['aac', 'opus']: output_acodec = audio_codec
        except Exception as e: logging.error(f"Critical error in process_video for {anonymized_name}: {e}", exc_info=True); file_callback_dispatcher(filename, "failed", {"message": f"Internal error: {e}", "type": "processing_error"}); process_successful = False

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

        if gui.elapsed_timer_id: pass # Timer stops itself
        overall_progress_percent = (gui.processed_files / total_videos_to_process) * 100
        update_ui_safely(gui.root, lambda p=overall_progress_percent: gui.overall_progress.config(value=p))
        def update_final_status(): # Update overall status label
            base_status=f"Progress: {gui.processed_files}/{total_videos_to_process} files ({gui.successful_conversions} successful)"
            error_suffix=f" - {gui.error_count} errors" if gui.error_count > 0 else ""; gui.status_label.config(text=f"{base_status}{error_suffix}")
        update_ui_safely(gui.root, update_final_status)

    # --- End of Loop ---
    final_status_message = "Conversion complete";
    if stop_event.is_set(): final_status_message = "Conversion stopped by user"
    elif gui.error_count > 0: final_status_message = f"Conversion complete with {gui.error_count} errors"
    logger.info(f"Worker finished. Status: {final_status_message}")
    update_ui_safely(gui.root, lambda msg=final_status_message: conversion_complete(gui, msg))


def conversion_complete(gui, final_message="Conversion complete"):
    """Handle completion/stopping, show combined summary"""
    # (Unchanged from previous correction)
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
    gui.status_label.config(text=final_message); reset_current_file_details(gui)
    gui.conversion_running = False; gui.stop_event = None; gui.current_process_info = None
    gui.start_button.config(state="normal"); gui.stop_button.config(state="disabled"); gui.force_stop_button.config(state="disabled")
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