# src/gui/gui_actions.py
"""
GUI action functions (browsing, opening files/folders, dependency checks)
for the AV1 Video Converter application.
"""
# Standard library imports
import os
import sys
import subprocess
import logging

# GUI-related imports
from tkinter import filedialog, messagebox
import tkinter as tk # For type hinting if needed

# Project imports - Replace 'convert_app' with 'src'
from src.ab_av1_wrapper import check_ab_av1_available
from src.utils import check_ffmpeg_availability, get_history_file_path

logger = logging.getLogger(__name__)


def browse_input_folder(gui) -> None:
    """Open file dialog to choose input folder and update GUI state.

    Args:
        gui: The main GUI instance containing UI elements
    """
    folder = filedialog.askdirectory(title="Select Input Folder")
    if folder:
        gui.input_folder.set(folder)
        logger.info(f"Input folder selected: {folder}")
        if not gui.output_folder.get():
            gui.output_folder.set(folder)
            logger.info(f"Output folder automatically set to match input: {folder}")

def browse_output_folder(gui) -> None:
    """Open file dialog to choose output folder and update GUI state.

    Args:
        gui: The main GUI instance containing UI elements
    """
    folder = filedialog.askdirectory(title="Select Output Folder")
    if folder:
        gui.output_folder.set(folder)
        logger.info(f"Output folder selected: {folder}")

def browse_log_folder(gui) -> None:
    """Open file dialog to choose log folder and update GUI state.

    Args:
        gui: The main GUI instance containing UI elements
    """
    folder = filedialog.askdirectory(title="Select Log Folder")
    if folder:
        gui.log_folder.set(folder)
        logger.info(f"Log folder selected by user: {folder}")
        messagebox.showinfo("Log Folder Changed", "Log folder preference updated.\nNew logs will be written to the selected folder on next application start.")

def open_log_folder_action(gui) -> None:
    """Open the currently used log folder in the file explorer.

    Args:
        gui: The main GUI instance containing log_directory attribute
    """
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

def open_history_file_action(gui) -> None:
    """Open the conversion history json file in the default text editor.

    Args:
        gui: The main GUI instance (not directly used but consistent with other actions)
    """
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


def check_ffmpeg(gui) -> bool:
    """Check if FFmpeg is installed and has SVT-AV1 support, and check for ab-av1.

    Args:
        gui: The main GUI instance for displaying messages

    Returns:
        True if all required components are available, False otherwise
    """
    logger.info("Checking FFmpeg and ab-av1 installation...")
    ffmpeg_available, svt_av1_available, version_info, error_message = check_ffmpeg_availability()

    if not ffmpeg_available:
        error_msg = f"Error: ffmpeg not found. {error_message}"
        logger.error(error_msg)
        messagebox.showerror("FFmpeg Not Found", "FFmpeg not found or not in PATH.\nPlease install FFmpeg.")
        return False

    if not svt_av1_available:
        warning_msg = "Warning: Your ffmpeg lacks SVT-AV1 support (libsvtav1)."
        logger.warning(warning_msg)
        if not messagebox.askokcancel("SVT-AV1 Support Missing?", f"{warning_msg}\nab-av1 requires this. Continue anyway?"):
            return False
    else:
        logger.info("FFmpeg with SVT-AV1 support detected.")

    if version_info:
        logger.info(f"FFmpeg version: {version_info.splitlines()[0]}")

    available, path, message = check_ab_av1_available()
    if not available:
        logger.error(f"ab-av1 check failed: {message}")
        messagebox.showerror("ab-av1 Not Found", message)
        return False
    else:
        logger.info(f"ab-av1 check successful: {message}")

    logger.info("FFmpeg and ab-av1 checks passed.")
    return True