# src/gui/main_window.py
"""
Main window module for the AV1 Video Converter application.
"""

# Standard library imports
import json  # For settings persistence
import logging
import multiprocessing
import os  # Added import
import sys
import tkinter as tk
from tkinter import messagebox, ttk

from src.gui.conversion_controller import force_stop_conversion, start_conversion, stop_conversion
from src.gui.gui_actions import (
    browse_input_folder,
    browse_log_folder,
    browse_output_folder,
    check_ffmpeg,
    open_history_file_action,
    open_log_folder_action,
)
from src.gui.gui_updates import update_statistics_summary

# Project imports - Replace 'convert_app' with 'src'
from src.gui.tabs.main_tab import create_main_tab
from src.gui.tabs.settings_tab import create_settings_tab

# Import setup_logging only needed here now - Replace 'convert_app' with 'src'
from src.utils import get_script_directory, scrub_history_paths, scrub_log_files, setup_logging

logger = logging.getLogger(__name__)

# Place config file next to script/executable
CONFIG_FILE = os.path.join(get_script_directory(), "av1_converter_config.json")


class VideoConverterGUI:
    """Main application window for the AV1 Video Converter application."""

    # Button widgets (created in create_main_tab)
    start_button: ttk.Button
    stop_button: ttk.Button
    force_stop_button: ttk.Button

    def __init__(self, root):
        """Initialize the main window and all components."""
        self.root = root
        self.root.title("AV1 Video Converter")
        self.root.geometry("800x675")  # Increased from 650 to 675
        self.root.minsize(700, 575)  # Increased from 550 to 575

        # Set application icon (Phase 1)
        try:
            icon_path = os.path.join(get_script_directory(), "app_icon.ico")
            if os.path.exists(icon_path):
                self.root.iconbitmap(icon_path)
                logger.info(f"Set window icon from: {icon_path}")
            else:
                logger.warning(f"Icon file not found: {icon_path}")
        except tk.TclError as e:
            # Handle cases where iconbitmap might not be supported (e.g., some Linux WMs)
            logger.warning(f"Failed to set window icon (TclError, platform compatibility?): {e}")
        except Exception:
            logger.exception("Unexpected error setting window icon")

        # Load settings first
        self.config = self.load_settings()

        # Initialize tk variables based on loaded config (needed *before* logging setup uses them)
        self.initialize_variables()

        # Setup logging using initialized variables (which hold config values or defaults)
        try:
            log_dir_pref = self.log_folder.get()  # Get value from tk.StringVar
            anonymize_pref = self.anonymize_logs.get()  # Get value from tk.BooleanVar
            # Store the *actual* directory used by logging setup
            self.log_directory = setup_logging(log_directory=log_dir_pref, anonymize=anonymize_pref)

            # Update the log_folder StringVar to reflect the actual directory used
            if self.log_directory:
                self.log_folder.set(self.log_directory)
                logger.info(f"Updated log folder display to actual path: {self.log_directory}")
            else:
                # If setup_logging failed to return a valid dir, clear the field maybe?
                # Clearing the field makes it clear that the configured path isn't being used.
                self.log_folder.set("")  # Clear the entry field if logging dir failed
                logger.warning("Log directory could not be determined or created. GUI display cleared.")

            logger.info("=== Starting AV1 Video Converter ===")  # Now log start message

        except Exception as e:
            # Handle errors during logging setup itself
            messagebox.showerror("Logging Error", f"Failed to initialize logging:\n{e}\n\nApplication cannot start.")
            # Attempt to clean up tk window if it exists
            try:
                root.destroy()
            except Exception as e:
                logger.debug(f"Error destroying root window: {e}")
            sys.exit(1)

        # Register exit handler AFTER logging is potentially set up
        self.root.protocol("WM_DELETE_WINDOW", self.on_exit)

        logger.info("Initializing VideoConverterGUI...")
        self.setup_styles()
        # initialize_variables() moved earlier

        self.tab_control = ttk.Notebook(self.root)
        self.main_tab = ttk.Frame(self.tab_control)
        self.settings_tab = ttk.Frame(self.tab_control)
        self.tab_control.add(self.main_tab, text="Convert")
        self.tab_control.add(self.settings_tab, text="Settings")
        self.tab_control.pack(expand=1, fill="both")

        create_main_tab(self)
        create_settings_tab(self)
        self.initialize_conversion_state()
        self.initialize_button_states()

        # Update statistics panel with historical data
        update_statistics_summary(self)

        check_ffmpeg(self)  # Check dependencies after UI is built

    def load_settings(self):
        """Load settings from JSON config file"""
        try:
            if os.path.exists(CONFIG_FILE):
                with open(CONFIG_FILE, encoding="utf-8") as f:
                    config = json.load(f)
                    logger.info(f"Loaded settings from {CONFIG_FILE}")
                    return config  # Changed print to logger
            else:
                logger.info(f"Config file {CONFIG_FILE} not found, using defaults.")
                return {}  # Changed print to logger
        except Exception:
            logger.exception(f"Error loading settings from {CONFIG_FILE}. Using defaults.")
            return {}  # Changed print to logger

    def save_settings(self):
        """Save settings to JSON config file"""
        try:
            # IMPORTANT: When saving, use the value from the StringVar, which the user might have changed
            # via the Browse button, even if the initial logging setup used a default/different path.
            # The preference saved should reflect what the user sees/sets in the GUI.
            log_folder_to_save = self.log_folder.get()

            current_config = {
                "input_folder": self.input_folder.get(),
                "output_folder": self.output_folder.get(),
                "overwrite": self.overwrite.get(),
                "ext_mp4": self.ext_mp4.get(),
                "ext_mkv": self.ext_mkv.get(),
                "ext_avi": self.ext_avi.get(),
                "ext_wmv": self.ext_wmv.get(),
                "convert_audio": self.convert_audio.get(),
                "audio_codec": self.audio_codec.get(),
                "log_folder": log_folder_to_save,  # Save the potentially user-modified path
                "anonymize_logs": self.anonymize_logs.get(),
                "anonymize_history": self.anonymize_history.get(),
                "delete_original_after_conversion": self.delete_original_var.get(),  # Added
            }
            temp_config_file = CONFIG_FILE + ".tmp"
            with open(temp_config_file, "w", encoding="utf-8") as f:
                json.dump(current_config, f, indent=4)
            os.replace(temp_config_file, CONFIG_FILE)
            logger.info(f"Saved settings to {CONFIG_FILE} (Log folder saved: '{log_folder_to_save}')")
        except Exception:
            logger.exception(f"Error saving settings to {CONFIG_FILE}")

    def setup_styles(self):
        """Set up the GUI styles"""
        self.style = ttk.Style()
        try:
            logger.debug(f"Using theme: {self.style.theme_use()}")
        except tk.TclError:
            logger.warning("Could not detect theme, using default.")
        self.style.configure("TFrame", background="#f0f0f0")
        self.style.configure("TButton", font=("Arial", 10))
        self.style.configure("TLabel", font=("Arial", 10), background="#f0f0f0")
        self.style.configure("Header.TLabel", font=("Arial", 10, "bold"), background="#f0f0f0")
        self.style.configure("ExtButton.TCheckbutton", font=("Arial", 9))
        self.style.configure("TLabelframe", background="#f0f0f0", padding=5)
        self.style.configure("TLabelframe.Label", font=("Arial", 10, "bold"), background="#f0f0f0")

        # Add custom style for range text - dark gray color
        self.style.configure("Range.TLabel", font=("Arial", 10), background="#f0f0f0", foreground="#606060")

    def initialize_variables(self):
        """Initialize the GUI variables, using loaded config"""
        # Initialize StringVars/BooleanVars *first* based on config or defaults
        self.input_folder = tk.StringVar(value=self.config.get("input_folder", ""))
        self.output_folder = tk.StringVar(value=self.config.get("output_folder", ""))
        # Initialize log_folder based on config. It will be overwritten shortly after
        # logging setup confirms the actual path, or cleared if invalid.
        self.log_folder = tk.StringVar(value=self.config.get("log_folder", ""))
        self.overwrite = tk.BooleanVar(value=self.config.get("overwrite", False))
        self.ext_mp4 = tk.BooleanVar(value=self.config.get("ext_mp4", True))
        self.ext_mkv = tk.BooleanVar(value=self.config.get("ext_mkv", True))
        self.ext_avi = tk.BooleanVar(value=self.config.get("ext_avi", True))
        self.ext_wmv = tk.BooleanVar(value=self.config.get("ext_wmv", True))
        self.convert_audio = tk.BooleanVar(value=self.config.get("convert_audio", True))
        self.anonymize_logs = tk.BooleanVar(value=self.config.get("anonymize_logs", True))
        self.anonymize_history = tk.BooleanVar(value=self.config.get("anonymize_history", True))
        self.audio_codec = tk.StringVar(value=self.config.get("audio_codec", "opus"))
        self.delete_original_var = tk.BooleanVar(value=self.config.get("delete_original_after_conversion", False))
        # Non-saved variables
        self.vmaf_scores = []
        self.crf_values = []
        self.size_reductions = []
        try:
            self.cpu_count = max(1, multiprocessing.cpu_count())
        except NotImplementedError:
            self.cpu_count = 1
            logger.warning("Could not detect CPU count.")

    def initialize_conversion_state(self):
        """Initialize conversion state variables.

        All gui.* attributes that are set during conversion should be initialized here
        to avoid AttributeError and make the codebase more predictable.
        """
        # Core conversion state
        self.conversion_running = False
        self.conversion_thread = None
        self.stop_event = None
        self.sleep_prevention_active = False

        # File lists
        self.video_files: list[str] = []
        self.pending_files: list[str] = []

        # Progress counters
        self.processed_files = 0
        self.successful_conversions = 0
        self.error_count = 0

        # Timing
        self.elapsed_timer_id = None
        self.total_conversion_start_time: float | None = None
        self.current_file_start_time: float | None = None
        self.current_file_encoding_start_time: float | None = None

        # Current file tracking
        self.output_folder_path = ""
        self.current_process_info: dict | None = None
        self.current_file_path: str | None = None

        # Statistics accumulators
        self.total_input_bytes_success = 0
        self.total_output_bytes_success = 0
        self.total_time_success = 0

        # ETA calculation state
        self.last_encoding_progress: float = 0
        self.last_eta_seconds: float | None = None
        self.last_eta_timestamp: float | None = None

        # Per-file state for callbacks
        self.last_input_size: int | None = None
        self.last_output_size: int | None = None
        self.last_elapsed_time: float | None = None

        # Error and skip tracking
        self.error_details: list[str] = []
        self.skipped_not_worth_count = 0
        self.skipped_not_worth_files: list[str] = []
        self.skipped_low_resolution_count = 0
        self.skipped_low_resolution_files: list[str] = []

    def initialize_button_states(self):
        """Initialize button states. Called after create_main_tab() creates buttons."""
        self.start_button.config(state="normal")
        self.stop_button.config(state="disabled")
        self.force_stop_button.config(state="disabled")

    def on_exit(self):
        """Handle application exit: confirm, save settings, cleanup"""
        confirm_exit = True
        if hasattr(self, "conversion_running") and self.conversion_running:
            confirm_exit = messagebox.askyesno("Confirm Exit", "Conversion running. Exit will stop it.\nAre you sure?")
        if confirm_exit:
            logger.info("=== AV1 Video Converter Exiting ===")
            self.save_settings()
            if hasattr(self, "conversion_running") and self.conversion_running:
                logger.info("Signalling conversion thread to stop...")
                force_stop_conversion(self, confirm=False)
            self._cleanup_threads()
            self.root.after(100, self._complete_exit)
        else:
            logger.info("User cancelled application exit.")

    def _cleanup_threads(self):
        """Ensure all threads are properly cleaned up before exit"""
        if hasattr(self, "elapsed_timer_id") and self.elapsed_timer_id:
            try:
                self.root.after_cancel(self.elapsed_timer_id)
                self.elapsed_timer_id = None
                logger.debug("Cancelled timer")
            except Exception:
                logger.exception("Error cancelling timer")
        if hasattr(self, "stop_event") and self.stop_event:
            self.stop_event.set()
            logger.debug("Stop event set.")
        if hasattr(self, "conversion_thread") and self.conversion_thread and self.conversion_thread.is_alive():
            try:
                logger.info("Waiting briefly for thread...")
                self.conversion_thread.join(timeout=1.0)
                if self.conversion_thread.is_alive():
                    logger.warning("Thread did not terminate quickly")
                else:
                    logger.debug("Thread terminated")
            except Exception:
                logger.exception("Error joining thread")
        self.conversion_thread = None
        self.conversion_running = False
        self.stop_event = None

    def _complete_exit(self):
        """Complete the exit process: shutdown logging, destroy window, force process exit"""
        logger.info("Destroying main window and exiting process.")
        try:
            logging.shutdown()
        except Exception as log_e:
            print(f"Error shutting down logging: {log_e}")
        try:
            self.root.destroy()
        except tk.TclError:
            pass
        except Exception as e:
            print(f"Error destroying root window: {e}")
        # Use os._exit(0) for a more forceful exit if needed after cleanup attempts
        logger.info("Forcing process exit.")
        os._exit(0)  # Changed print to logger before exit

    # Method references for GUI callbacks - now pointing to imported functions
    def on_browse_input_folder(self):
        browse_input_folder(self)

    def on_browse_output_folder(self):
        browse_output_folder(self)

    def on_browse_log_folder(self):
        browse_log_folder(self)

    def on_open_log_folder(self):
        open_log_folder_action(self)

    def on_open_history_file(self):
        open_history_file_action(self)

    def on_scrub_history(self):
        """Scrub all file paths in the history file after confirmation."""
        confirmed = messagebox.askyesno(
            "Scrub History - Irreversible",
            "This will permanently replace all file paths in your conversion history "
            "with anonymized hashes.\n\n"
            "The original filenames and paths will be unrecoverable.\n\n"
            "Technical data (sizes, durations, VMAF scores, etc.) will be preserved.\n\n"
            "Are you sure you want to continue?",
            icon="warning",
        )
        if not confirmed:
            return

        total, modified = scrub_history_paths()
        if total == 0:
            messagebox.showinfo("Scrub History", "No history records found.")
        elif modified == 0:
            messagebox.showinfo("Scrub History", f"All {total} records were already anonymized.")
        elif modified == total:
            messagebox.showinfo("Scrub History", f"Anonymized all {total} history records.")
        else:
            unchanged = total - modified
            messagebox.showinfo(
                "Scrub History",
                f"Anonymized {modified} history records.\n\n"
                f"{unchanged} record(s) were already anonymized.",
            )

    def on_scrub_logs(self):
        """Scrub all file paths in existing log files after confirmation."""
        confirmed = messagebox.askyesno(
            "Scrub Logs - Irreversible",
            "This will permanently replace all file paths in your log files "
            "with anonymized hashes.\n\n"
            "The original filenames and paths will be unrecoverable.\n\n"
            "Are you sure you want to continue?",
            icon="warning",
        )
        if not confirmed:
            return

        log_dir = getattr(self, "log_directory", None)
        total, modified = scrub_log_files(log_dir)
        if total == 0:
            messagebox.showinfo("Scrub Logs", "No log files found.")
        elif modified == 0:
            messagebox.showinfo("Scrub Logs", f"All {total} log files were already anonymized or contained no paths.")
        elif modified == total:
            messagebox.showinfo("Scrub Logs", f"Anonymized all {total} log files.")
        else:
            unchanged = total - modified
            messagebox.showinfo(
                "Scrub Logs",
                f"Anonymized {modified} log files.\n\n"
                f"{unchanged} file(s) were already anonymized or contained no paths.",
            )

    def on_start_conversion(self):
        start_conversion(self)

    def on_stop_conversion(self):
        stop_conversion(self)

    def on_force_stop_conversion(self, confirm=True):
        force_stop_conversion(self, confirm=confirm)
