# src/gui/main_window.py
"""
Main window module for the AV1 Video Converter application.
"""

# Standard library imports
import json  # For settings persistence
import logging
import multiprocessing
import os  # Added import
import shutil
import sys
import threading
import tkinter as tk
import uuid
import webbrowser
from collections import deque
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from pathlib import Path
from tkinter import filedialog, messagebox, ttk
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from src.gui.charts import BarChart, LineGraph, PieChart

from src.ab_av1.checker import check_ab_av1_latest_github, get_ab_av1_version
from src.cache_helpers import mtimes_match
from src.config import MIN_FILES_FOR_PERCENT_UPDATES, TREE_UPDATE_BATCH_SIZE
from src.estimation import estimate_file_time
from src.folder_analysis import _analyze_file
from src.gui.conversion_controller import force_stop_conversion, start_conversion, stop_conversion
from src.gui.dialogs import FFmpegDownloadDialog
from src.gui.gui_actions import (
    browse_input_folder,
    browse_log_folder,
    browse_output_folder,
    check_ffmpeg,
    open_history_file_action,
    open_log_folder_action,
)

# Project imports - Replace 'convert_app' with 'src'
from src.gui.tabs.analysis_tab import create_analysis_tab
from src.gui.tabs.convert_tab import create_convert_tab
from src.gui.tabs.settings_tab import create_settings_tab
from src.gui.tabs.statistics_tab import create_statistics_tab
from src.gui.widgets.add_to_queue_dialog import AddToQueuePreviewDialog, QueuePreviewData
from src.history_index import compute_path_hash, get_history_index
from src.models import ConversionSessionState, FileStatus, OperationType, OutputMode, QueueItem, QueueItemStatus

# Import setup_logging only needed here now - Replace 'convert_app' with 'src'
from src.utils import (
    check_ffmpeg_availability,
    check_ffmpeg_latest_btbn,
    check_ffmpeg_latest_gyan,
    format_file_size,
    get_script_directory,
    parse_ffmpeg_version,
    scrub_history_paths,
    scrub_log_files,
    setup_logging,
    update_ui_safely,
)
from src.vendor_manager import download_ab_av1, download_ffmpeg

logger = logging.getLogger(__name__)

# Efficiency formatting threshold (GB/hr)
_EFFICIENCY_DECIMAL_THRESHOLD = 10  # Show without decimals above this value


# Place config file next to script/executable
CONFIG_FILE = os.path.join(get_script_directory(), "av1_converter_config.json")


class VideoConverterGUI:
    """Main application window for the AV1 Video Converter application."""

    # Convert tab widgets (created in create_convert_tab)
    add_folder_button: ttk.Button
    add_files_button: ttk.Button
    remove_queue_button: ttk.Button
    clear_queue_button: ttk.Button
    start_button: ttk.Button
    stop_button: ttk.Button
    force_stop_button: ttk.Button
    overall_progress: ttk.Progressbar
    status_label: ttk.Label
    total_elapsed_label: ttk.Label
    total_remaining_label: ttk.Label
    queue_tree: ttk.Treeview
    queue_total_tree: ttk.Treeview
    queue_properties_frame: ttk.LabelFrame
    item_output_mode: tk.StringVar
    item_mode_combo: ttk.Combobox
    item_suffix: tk.StringVar
    item_suffix_entry: ttk.Entry
    item_output_folder: tk.StringVar
    item_folder_entry: ttk.Entry
    item_folder_browse_button: ttk.Button
    item_source_label: ttk.Label
    current_file_label: ttk.Label
    quality_progress: ttk.Progressbar
    quality_percent_label: ttk.Label
    encoding_progress: ttk.Progressbar
    encoding_percent_label: ttk.Label
    orig_format_label: ttk.Label
    orig_size_label: ttk.Label
    vmaf_label: ttk.Label
    encoding_settings_label: ttk.Label
    elapsed_label: ttk.Label
    eta_label: ttk.Label
    output_size_label: ttk.Label

    # Analysis tab widgets (created in create_analysis_tab)
    analyze_button: ttk.Button
    add_all_analyze_button: ttk.Button
    add_all_convert_button: ttk.Button
    analysis_tree: ttk.Treeview
    analysis_total_tree: ttk.Treeview

    # Settings tab widgets (created in create_settings_tab)
    ab_av1_frame: ttk.Frame
    ab_av1_version_label: ttk.Label
    ab_av1_download_btn: ttk.Button | None
    ab_av1_check_btn: ttk.Button | None
    ab_av1_update_btn: ttk.Button | None
    ab_av1_update_label: ttk.Label
    ffmpeg_frame: ttk.Frame
    ffmpeg_version_label: ttk.Label
    ffmpeg_download_btn: ttk.Button | None
    ffmpeg_check_btn: ttk.Button | None
    ffmpeg_update_btn: ttk.Button | None
    ffmpeg_update_label: ttk.Label | None
    ffmpeg_source: str | None

    # Statistics tab widgets (created in create_statistics_tab)
    histogram_canvas: tk.Canvas
    histogram_chart: "BarChart"
    codec_canvas: tk.Canvas
    codec_chart: "PieChart"
    savings_canvas: tk.Canvas
    savings_chart: "LineGraph"
    refresh_stats_button: ttk.Button
    stats_status_label: ttk.Label
    stats_total_files_label: ttk.Label
    vmaf_stats_label: ttk.Label
    vmaf_range_label: ttk.Label
    crf_stats_label: ttk.Label
    crf_range_label: ttk.Label
    total_saved_label: ttk.Label
    size_stats_label: ttk.Label
    size_range_label: ttk.Label
    throughput_stats_label: ttk.Label
    stats_date_range_label: ttk.Label

    def __init__(self, root):
        """Initialize the main window and all components."""
        self.root = root
        self.root.title("AV1 Video Converter")
        self.root.geometry("950x750")  # Increased for Analysis tab
        self.root.minsize(850, 650)  # Increased for Analysis tab

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

        # Initialize conversion session state (always exists, never None)
        self.session = ConversionSessionState()

        # Thread/Event primitives stay on self, not in session dataclass
        self.conversion_thread: threading.Thread | None = None
        self.stop_event: threading.Event | None = None

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
        self.convert_tab = ttk.Frame(self.tab_control)
        self.analysis_tab = ttk.Frame(self.tab_control)
        self.statistics_tab = ttk.Frame(self.tab_control)
        self.settings_tab = ttk.Frame(self.tab_control)
        self.tab_control.add(self.convert_tab, text="Queue")
        self.tab_control.add(self.analysis_tab, text="Analysis")
        self.tab_control.add(self.statistics_tab, text="Statistics")
        self.tab_control.add(self.settings_tab, text="Settings")
        self.tab_control.pack(expand=1, fill="both")

        create_convert_tab(self)
        create_analysis_tab(self)
        create_statistics_tab(self)
        create_settings_tab(self)
        self.initialize_conversion_state()
        self.initialize_button_states()

        check_ffmpeg(self)  # Check dependencies after UI is built

        # Schedule initial analysis tree population (deferred so UI is fully built)
        self.root.after(0, self._refresh_analysis_tree)

        # Populate queue tree from restored items
        self.root.after(0, self.refresh_queue_tree)

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
                "default_output_mode": self.default_output_mode.get(),
                "default_suffix": self.default_suffix.get(),
                "default_output_folder": self.default_output_folder.get(),
                "hw_decode_enabled": self.hw_decode_enabled.get(),
                "queue_items": [item.to_dict() for item in self._queue_items],
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
        self.style.configure("TLabel", font=("Arial", 10), background="#f0f0f0")
        self.style.configure("Header.TLabel", font=("Arial", 10, "bold"), background="#f0f0f0")
        self.style.configure("ExtButton.TCheckbutton", font=("Arial", 9))
        self.style.configure("TLabelframe", background="#f0f0f0", padding=5)
        self.style.configure("TLabelframe.Label", font=("Arial", 10, "bold"), background="#f0f0f0")
        self.style.configure("TNotebook.Tab", font=("Arial", 11), padding=(10, 4))

        # Add custom style for range text - dark gray color
        self.style.configure("Range.TLabel", font=("Arial", 10), background="#f0f0f0", foreground="#606060")

        # Custom Treeview style without expand/collapse indicator (we use arrows in text instead)
        self.style.layout(
            "Analysis.Treeview.Item",
            [
                (
                    "Treeitem.padding",
                    {
                        "sticky": "nswe",
                        "children": [
                            ("Treeitem.image", {"side": "left", "sticky": ""}),
                            (
                                "Treeitem.focus",
                                {
                                    "side": "left",
                                    "sticky": "",
                                    "children": [("Treeitem.text", {"side": "left", "sticky": ""})],
                                },
                            ),
                        ],
                    },
                )
            ],
        )

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
        self.hw_decode_enabled = tk.BooleanVar(value=self.config.get("hw_decode_enabled", True))

        # CPU count for display purposes
        try:
            self.cpu_count = max(1, multiprocessing.cpu_count())
        except NotImplementedError:
            self.cpu_count = 1
            logger.warning("Could not detect CPU count.")

        # Queue/Output defaults
        self.default_output_mode = tk.StringVar(value=self.config.get("default_output_mode", "replace"))
        self.default_suffix = tk.StringVar(value=self.config.get("default_suffix", "_av1"))
        self.default_output_folder = tk.StringVar(value=self.config.get("default_output_folder", ""))

        # Queue state (will be restored from config on startup)
        self._queue_items: list[QueueItem] = self._load_queue_from_config()
        self._queue_tree_map: dict[str, str] = {}  # queue_item.id -> tree_item_id
        self._queue_items_by_id: dict[str, QueueItem] = {item.id: item for item in self._queue_items}
        self._tree_queue_map: dict[str, str] = {}  # tree_item_id -> queue_item.id (reverse)

        # Analysis state
        self.analysis_stop_event: threading.Event | None = None
        self.analysis_thread: threading.Thread | None = None
        self._tree_item_map: dict[str, str] = {}  # Map file_path -> tree_item_id for updates
        self._refresh_timer_id: str | None = None  # Debounce timer for auto-refresh
        self._scan_stop_event: threading.Event | None = None  # Stop event for background scan
        self._sort_col: str | None = None  # Current sort column
        self._sort_reverse: bool = False  # Current sort direction

        # Add traces to auto-refresh analysis tree when settings change
        self.input_folder.trace_add("write", self._on_folder_or_extension_changed)
        self.ext_mp4.trace_add("write", self._on_folder_or_extension_changed)
        self.ext_mkv.trace_add("write", self._on_folder_or_extension_changed)
        self.ext_avi.trace_add("write", self._on_folder_or_extension_changed)
        self.ext_wmv.trace_add("write", self._on_folder_or_extension_changed)

    def get_queue_items(self) -> list[QueueItem]:
        """Return the list of queue items."""
        return self._queue_items

    def get_queue_tree_id(self, queue_item_id: str) -> str | None:
        """Return the tree item ID for a queue item, or None if not found."""
        return self._queue_tree_map.get(queue_item_id)

    def get_queue_source_path_for_tree_item(self, tree_id: str) -> str | None:
        """Return the source path for a queue tree item, or None if not found."""
        item = self.get_queue_item_for_tree_item(tree_id)
        return item.source_path if item else None

    def get_queue_item_for_tree_item(self, tree_id: str) -> QueueItem | None:
        """Return the QueueItem for a queue tree item, or None if not found."""
        queue_id = self._tree_queue_map.get(tree_id)
        return self._queue_items_by_id.get(queue_id) if queue_id else None

    def get_queue_item_by_id(self, queue_id: str) -> QueueItem | None:
        """Return the QueueItem for a given queue ID, or None if not found."""
        return self._queue_items_by_id.get(queue_id)

    def initialize_conversion_state(self):
        """Reset conversion session state for a new conversion.

        Creates a fresh ConversionSessionState with output folder pre-filled.
        Thread/Event primitives stay on self, not in the session dataclass.
        """
        # Reset session to fresh state with output folder
        self.session = ConversionSessionState(output_folder_path=self.output_folder.get())

        # Thread primitives stay on self
        self.conversion_thread = None
        self.stop_event = threading.Event()

    def initialize_button_states(self):
        """Initialize button states. Called after create_main_tab() creates buttons."""
        self.start_button.config(state="normal")
        self.stop_button.config(state="disabled")
        self.force_stop_button.config(state="disabled")

    def on_exit(self):
        """Handle application exit: confirm, save settings, cleanup"""
        confirm_exit = True
        if self.session.running:
            confirm_exit = messagebox.askyesno("Confirm Exit", "Queue is running. Exit will stop it.\nAre you sure?")
        if confirm_exit:
            logger.info("=== AV1 Video Converter Exiting ===")
            self.save_settings()
            if self.session.running:
                logger.info("Signalling conversion thread to stop...")
                force_stop_conversion(self, confirm=False)
            self._cleanup_threads()
            self.root.after(100, self._complete_exit)
        else:
            logger.info("User cancelled application exit.")

    def _cleanup_threads(self):
        """Ensure all threads are properly cleaned up before exit"""
        if self.session.elapsed_timer_id:
            try:
                self.root.after_cancel(self.session.elapsed_timer_id)
                self.session.elapsed_timer_id = None
                logger.debug("Cancelled timer")
            except Exception:
                logger.exception("Error cancelling timer")
        if self.stop_event:
            self.stop_event.set()
            logger.debug("Stop event set.")
        if self.conversion_thread and self.conversion_thread.is_alive():
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
        self.session.running = False
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

    def on_browse_default_output_folder(self):
        """Handle browse button for default output folder."""
        initial_dir = self.default_output_folder.get() or os.path.expanduser("~")
        folder = filedialog.askdirectory(initialdir=initial_dir, title="Select Default Output Folder")
        if folder:
            self.default_output_folder.set(folder)
            self.save_settings()

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
                f"Anonymized {modified} history records.\n\n{unchanged} record(s) were already anonymized.",
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

    def on_check_ab_av1_updates(self):
        """Check GitHub for the latest ab-av1 version and update the label."""
        # Disable check button immediately
        if self.ab_av1_check_btn:
            self.ab_av1_check_btn.config(state="disabled")

        # Reset label state
        self._reset_update_label()
        self.ab_av1_update_label.config(text="Checking...", foreground="gray")
        self.root.update_idletasks()

        local_version = get_ab_av1_version()
        latest_version, _release_url, message = check_ab_av1_latest_github()

        # Check button stays disabled permanently after use

        if latest_version is None:
            self.ab_av1_update_label.config(text=message, foreground="red")
            return

        if local_version is None:
            self.ab_av1_update_label.config(text=f"Latest: {latest_version}", foreground="gray")
            return

        # Compare versions
        if local_version == latest_version:
            self.ab_av1_update_label.config(text=f"Up to date ({latest_version})", foreground="green")
        else:
            # Update available - create Update button dynamically
            self.ab_av1_update_label.config(text=f"Update available: {latest_version}", foreground="blue")

            # Create Update button if it doesn't exist
            if not hasattr(self, "ab_av1_update_btn") or self.ab_av1_update_btn is None:
                self.ab_av1_update_btn = ttk.Button(self.ab_av1_frame, text="Update", command=self.on_download_ab_av1)
                self.ab_av1_update_btn.pack(side="left", padx=(5, 0))

    def _reset_update_label(self):
        """Reset the update label to non-clickable state."""
        self.ab_av1_update_label.config(cursor="", font=("TkDefaultFont", 9))
        self.ab_av1_update_label.unbind("<Button-1>")

    def on_check_ffmpeg_updates(self):
        """Check GitHub for the latest FFmpeg version and update the label."""
        if not self.ffmpeg_update_label or not self.ffmpeg_source:
            return

        # Disable check button immediately
        if self.ffmpeg_check_btn:
            self.ffmpeg_check_btn.config(state="disabled")

        # Reset label state
        self._reset_ffmpeg_update_label()
        self.ffmpeg_update_label.config(text="Checking...", foreground="gray")
        self.root.update_idletasks()

        # Get local version
        _, _, version_string, _ = check_ffmpeg_availability()
        local_version, _, _ = parse_ffmpeg_version(version_string)

        # Check GitHub based on detected source
        if self.ffmpeg_source == "gyan.dev":
            latest_version, release_url, message = check_ffmpeg_latest_gyan()
        elif self.ffmpeg_source == "BtbN":
            latest_version, release_url, message = check_ffmpeg_latest_btbn()
        else:
            self.ffmpeg_update_label.config(text="Unknown source", foreground="red")
            # Check button stays disabled permanently after use
            return

        # Check button stays disabled permanently after use

        if latest_version is None:
            self.ffmpeg_update_label.config(text=message, foreground="red")
            return

        # For BtbN, we can't compare versions (date-based tags), so just show latest and link
        if self.ffmpeg_source == "BtbN":
            # Extract display date from tag like "autobuild-2025-12-18-12-50" -> "2025-12-18"
            if "autobuild" in latest_version:
                display_tag = latest_version.replace("autobuild-", "").rsplit("-", 2)[0]
            else:
                display_tag = latest_version
            self.ffmpeg_update_label.config(
                text=f"Latest: {display_tag}", foreground="blue", cursor="hand2", font=("TkDefaultFont", 9, "underline")
            )
            if release_url:
                self.ffmpeg_update_label.bind("<Button-1>", lambda e: webbrowser.open(release_url))
            return

        # For gyan.dev, we can compare semantic versions
        if local_version is None:
            self.ffmpeg_update_label.config(text=f"Latest: {latest_version}", foreground="gray")
            return

        if local_version == latest_version:
            self.ffmpeg_update_label.config(text=f"Up to date ({latest_version})", foreground="green")
        else:
            # Update available - create Update button dynamically
            self.ffmpeg_update_label.config(text=f"Update available: {latest_version}", foreground="blue")

            # Create Update button if it doesn't exist
            if not hasattr(self, "ffmpeg_update_btn") or self.ffmpeg_update_btn is None:
                self.ffmpeg_update_btn = ttk.Button(self.ffmpeg_frame, text="Update", command=self.on_download_ffmpeg)
                self.ffmpeg_update_btn.pack(side="left", padx=(5, 0))

    def _reset_ffmpeg_update_label(self):
        """Reset the FFmpeg update label to non-clickable state."""
        if self.ffmpeg_update_label:
            self.ffmpeg_update_label.config(cursor="", font=("TkDefaultFont", 9))
            self.ffmpeg_update_label.unbind("<Button-1>")

    def on_download_ab_av1(self):
        """Download ab-av1 from GitHub in a background thread."""
        # Disable button and show progress
        if self.ab_av1_download_btn:
            self.ab_av1_download_btn.config(state="disabled")
        if hasattr(self, "ab_av1_update_btn") and self.ab_av1_update_btn:
            self.ab_av1_update_btn.config(state="disabled")
        self.ab_av1_update_label.config(text="Downloading...", foreground="gray")
        self.root.update_idletasks()

        def download_thread():
            def progress_callback(downloaded, total):
                if total > 0:
                    pct = int(downloaded * 100 / total)
                    self.root.after(0, lambda: self.ab_av1_update_label.config(text=f"Downloading... {pct}%"))

            success, message = download_ab_av1(progress_callback)

            def update_ui():
                if success:
                    # Update version label
                    new_version = get_ab_av1_version() or "Installed"
                    self.ab_av1_version_label.config(text=new_version)
                    self.ab_av1_update_label.config(text="Download complete!", foreground="green")

                    # Destroy the Update button if it exists (user is now up to date)
                    if hasattr(self, "ab_av1_update_btn") and self.ab_av1_update_btn:
                        self.ab_av1_update_btn.destroy()
                        self.ab_av1_update_btn = None

                    # Re-enable download button if it exists
                    if self.ab_av1_download_btn:
                        self.ab_av1_download_btn.config(state="normal")
                else:
                    self.ab_av1_update_label.config(text=message, foreground="red")
                    # Re-enable buttons on failure
                    if self.ab_av1_download_btn:
                        self.ab_av1_download_btn.config(state="normal")
                    if hasattr(self, "ab_av1_update_btn") and self.ab_av1_update_btn:
                        self.ab_av1_update_btn.config(state="normal")

            self.root.after(0, update_ui)

        threading.Thread(target=download_thread, daemon=True).start()

    def on_download_ffmpeg(self):
        """Download FFmpeg to vendor folder. Shows dialog if system FFmpeg exists."""
        # Check for existing system FFmpeg
        existing_ffmpeg = shutil.which("ffmpeg")

        # If system FFmpeg exists, show informational dialog
        if existing_ffmpeg:
            existing_dir = Path(existing_ffmpeg).parent
            dialog = FFmpegDownloadDialog(self.root, existing_dir)
            if not dialog.show():
                return  # User cancelled

        # Disable button and show progress
        if self.ffmpeg_download_btn:
            self.ffmpeg_download_btn.config(state="disabled")
        if hasattr(self, "ffmpeg_update_btn") and self.ffmpeg_update_btn:
            self.ffmpeg_update_btn.config(state="disabled")
        if self.ffmpeg_update_label:
            self.ffmpeg_update_label.config(text="Downloading...", foreground="gray")
        self.root.update_idletasks()

        def download_thread():
            def progress_callback(downloaded, total):
                if total > 0:
                    mb_downloaded = downloaded / (1024 * 1024)
                    mb_total = total / (1024 * 1024)
                    text = f"Downloading... {mb_downloaded:.0f}/{mb_total:.0f} MB"
                    label = self.ffmpeg_update_label
                    if label:
                        self.root.after(0, lambda t=text, lbl=label: lbl.config(text=t))

            success, message = download_ffmpeg(progress_callback)

            def update_ui():
                if success:
                    # Update version label
                    _, _, version_string, _ = check_ffmpeg_availability()
                    version, _, _ = parse_ffmpeg_version(version_string)
                    display = f"{version} (vendor)" if version else "Installed"
                    self.ffmpeg_version_label.config(text=display)
                    if self.ffmpeg_update_label:
                        self.ffmpeg_update_label.config(text="Download complete!", foreground="green")
                    # Update ffmpeg_source to gyan.dev
                    self.ffmpeg_source = "gyan.dev"

                    # Destroy the Update button if it exists (user is now up to date)
                    if hasattr(self, "ffmpeg_update_btn") and self.ffmpeg_update_btn:
                        self.ffmpeg_update_btn.destroy()
                        self.ffmpeg_update_btn = None

                    # Re-enable download button if it exists
                    if self.ffmpeg_download_btn:
                        self.ffmpeg_download_btn.config(state="normal")
                else:
                    if self.ffmpeg_update_label:
                        self.ffmpeg_update_label.config(text=message, foreground="red")
                    # Re-enable buttons on failure
                    if self.ffmpeg_download_btn:
                        self.ffmpeg_download_btn.config(state="normal")
                    if hasattr(self, "ffmpeg_update_btn") and self.ffmpeg_update_btn:
                        self.ffmpeg_update_btn.config(state="normal")

            self.root.after(0, update_ui)

        threading.Thread(target=download_thread, daemon=True).start()

    def _load_queue_from_config(self) -> list[QueueItem]:
        """Load queue items from config, filtering out completed/invalid entries."""
        raw_items = self.config.get("queue_items", [])
        items = []
        for data in raw_items:
            try:
                item = QueueItem.from_dict(data)
                # Reset interrupted items (CONVERTING, STOPPED) to PENDING for retry
                if item.status in (QueueItemStatus.CONVERTING, QueueItemStatus.STOPPED):
                    item.status = QueueItemStatus.PENDING
                    # Reset outcome counters for retry
                    item.files_succeeded = 0
                    item.files_skipped = 0
                    item.files_failed = 0
                    item.last_error = None
                # Only restore PENDING items if file exists (skip completed/error items)
                if item.status == QueueItemStatus.PENDING and os.path.exists(item.source_path):
                    items.append(item)
            except (KeyError, ValueError):
                continue  # Skip invalid entries
        return items

    def save_queue_to_config(self):
        """Save current queue state (called on add/remove/modify)."""
        self.save_settings()

    def on_start_conversion(self):
        start_conversion(self)

    def on_stop_conversion(self):
        stop_conversion(self)

    def on_force_stop_conversion(self, confirm=True):
        force_stop_conversion(self, confirm=confirm)

    # --- Queue Tab Handlers ---

    def on_add_folder_to_queue(self):
        """Add a folder to the conversion queue."""
        folder = filedialog.askdirectory(title="Select Folder to Convert")
        if not folder:
            return
        self.add_to_queue(folder, is_folder=True)

    def on_add_files_to_queue(self):
        """Add individual files to the conversion queue."""
        files = filedialog.askopenfilenames(
            title="Select Video Files",
            filetypes=[("Video files", "*.mp4 *.mkv *.avi *.wmv *.mov *.webm"), ("All files", "*.*")],
        )
        for f in files:
            self.add_to_queue(f, is_folder=False)

    def _find_existing_queue_item(self, path: str) -> QueueItem | None:
        """Find an existing queue item by path."""
        for item in self._queue_items:
            if item.source_path == path:
                return item
        return None

    def _create_queue_item(self, path: str, is_folder: bool, operation_type: OperationType) -> QueueItem:
        """Create a new QueueItem with default settings."""
        default_mode = self.default_output_mode.get()
        return QueueItem(
            id=str(uuid.uuid4()),
            source_path=path,
            is_folder=is_folder,
            output_mode=OutputMode(default_mode),
            output_suffix=self.default_suffix.get() if default_mode == "suffix" else None,
            output_folder=self.default_output_folder.get() if default_mode == "separate_folder" else None,
            operation_type=operation_type,
        )

    def _categorize_queue_items(
        self, items: list[tuple[str, bool]], operation_type: OperationType
    ) -> tuple[list[tuple[str, bool]], list[str], list[tuple[str, bool, QueueItem]]]:
        """Categorize items for queue preview.

        Returns:
            Tuple of (to_add, duplicates, conflicts) where:
            - to_add: Items that can be added directly
            - duplicates: Paths already in queue with same operation
            - conflicts: (path, is_folder, existing_item) for different operation
        """
        to_add: list[tuple[str, bool]] = []
        duplicates: list[str] = []
        conflicts: list[tuple[str, bool, QueueItem]] = []

        for path, is_folder in items:
            existing = self._find_existing_queue_item(path)
            if not existing:
                to_add.append((path, is_folder))
            elif existing.operation_type == operation_type:
                duplicates.append(path)
            else:
                conflicts.append((path, is_folder, existing))

        return to_add, duplicates, conflicts

    def _calculate_queue_estimates(self, items: list[tuple[str, bool]]) -> tuple[float | None, float | None]:
        """Calculate time estimate and potential savings for items.

        Returns:
            Tuple of (estimated_time_seconds, estimated_savings_percent)
        """
        total_time = 0.0
        total_original_size = 0
        total_saved_bytes = 0.0
        has_time_estimates = False

        index = get_history_index()

        for path, is_folder in items:
            if is_folder:
                continue  # Skip folders for now, would need to scan

            # Try to get time estimate
            estimate = estimate_file_time(path)
            if estimate.confidence != "none":
                total_time += estimate.best_seconds
                has_time_estimates = True

            # Try to get savings estimate from history
            path_hash = compute_path_hash(path)
            record = index.get(path_hash)
            if record and record.file_size_bytes:
                # Use Layer 2 data if available, fall back to Layer 1
                reduction_pct = record.predicted_size_reduction or record.estimated_reduction_percent
                if reduction_pct:
                    total_original_size += record.file_size_bytes
                    total_saved_bytes += record.file_size_bytes * (reduction_pct / 100)

        # Calculate overall reduction percentage (weighted average)
        total_savings = None
        if total_original_size > 0 and total_saved_bytes > 0:
            total_savings = (total_saved_bytes / total_original_size) * 100

        return (total_time if has_time_estimates else None, total_savings)

    def add_items_to_queue(
        self, items: list[tuple[str, bool]], operation_type: OperationType, force_preview: bool = False
    ) -> dict[str, int]:
        """Add items to queue with appropriate UI feedback.

        This is the main entry point for all queue additions.

        Args:
            items: List of (path, is_folder) tuples
            operation_type: OperationType.CONVERT or OperationType.ANALYZE
            force_preview: If True, always show preview dialog (for "Add All")
                          If False, only show dialog if there are conflicts

        Returns:
            Dict with counts: {"added", "duplicate", "conflict_added", "conflict_replaced", "cancelled"}
        """
        counts = {"added": 0, "duplicate": 0, "conflict_added": 0, "conflict_replaced": 0, "cancelled": 0}

        if not items:
            return counts

        # Categorize items
        to_add, duplicates, conflicts = self._categorize_queue_items(items, operation_type)
        counts["duplicate"] = len(duplicates)

        # Determine if we need to show the preview dialog
        show_dialog = force_preview or len(conflicts) > 0

        if show_dialog:
            # Calculate estimates for preview
            estimated_time, estimated_savings = self._calculate_queue_estimates(to_add)

            # Build preview data
            preview_data = QueuePreviewData(
                to_add=to_add,
                duplicates=duplicates,
                conflicts=conflicts,
                operation_type=operation_type,
                estimated_time_sec=estimated_time,
                estimated_savings_percent=estimated_savings,
            )

            # Show dialog
            dialog = AddToQueuePreviewDialog(self.root, preview_data)
            result = dialog.result

            if result["action"] == "cancel":
                counts["cancelled"] = len(to_add) + len(conflicts)
                return counts

            conflict_resolution = result["conflict_resolution"]
        else:
            # No dialog needed, just add
            conflict_resolution = "skip"

        # Add the non-conflicting items
        for path, is_folder in to_add:
            item = self._create_queue_item(path, is_folder, operation_type)
            self._queue_items.append(item)
            counts["added"] += 1

        # Handle conflicts based on resolution choice
        if conflicts:
            if conflict_resolution == "keep_both":
                for path, is_folder, _ in conflicts:
                    item = self._create_queue_item(path, is_folder, operation_type)
                    self._queue_items.append(item)
                    counts["conflict_added"] += 1
            elif conflict_resolution == "replace":
                for path, is_folder, existing in conflicts:
                    new_item = self._create_queue_item(path, is_folder, operation_type)
                    idx = self._queue_items.index(existing)
                    self._queue_items[idx] = new_item
                    counts["conflict_replaced"] += 1
            # else: skip - conflicts are not added

        # Save and refresh if anything was added or modified
        if counts["added"] > 0 or counts["conflict_added"] > 0 or counts["conflict_replaced"] > 0:
            self.save_queue_to_config()
            self.refresh_queue_tree()
            self.sync_queue_tags_to_analysis_tree()

        return counts

    def add_to_queue(self, path: str, is_folder: bool, operation_type: OperationType = OperationType.CONVERT) -> str:
        """Add a single item to the queue (convenience wrapper).

        For selective adds without conflicts, this is silent.
        For conflicts, shows the preview dialog.

        Returns:
            "added", "duplicate", "conflict_added", "conflict_replaced", or "cancelled"
        """
        result = self.add_items_to_queue([(path, is_folder)], operation_type, force_preview=False)

        if result["added"] > 0:
            return "added"
        if result["duplicate"] > 0:
            return "duplicate"
        if result["conflict_added"] > 0:
            return "conflict_added"
        if result["conflict_replaced"] > 0:
            return "conflict_replaced"
        return "cancelled"

    def on_remove_from_queue(self):
        """Remove selected items from queue."""
        # Get selected items
        selected = self.queue_tree.selection()
        if not selected:
            return

        # Collect queue items to remove using O(1) lookups
        items_to_remove = []
        for tree_id in selected:
            item = self.get_queue_item_for_tree_item(tree_id)
            if item:
                items_to_remove.append(item)

        # Remove from queue
        for item in items_to_remove:
            self._queue_items.remove(item)

        self.save_queue_to_config()
        self.refresh_queue_tree()
        self.sync_queue_tags_to_analysis_tree()

    def on_clear_queue(self):
        """Clear all items from queue."""
        if not self._queue_items:
            return
        if messagebox.askyesno("Clear Queue", "Remove all items from the queue?"):
            self._queue_items.clear()
            self.save_queue_to_config()
            self.refresh_queue_tree()
            self.sync_queue_tags_to_analysis_tree()

    def on_queue_selection_changed(self):
        """Handle selection change in queue tree."""
        selected = self.queue_tree.selection()
        if not selected:
            # Disable controls and show placeholder when nothing selected
            self.item_mode_combo.config(state="disabled")
            self.item_suffix_entry.config(state="disabled")
            self.item_folder_entry.config(state="disabled")
            self.item_folder_browse_button.config(state="disabled")
            self.item_output_mode.set("")
            self.item_suffix.set("")
            self.item_output_folder.set("")
            self.item_source_label.config(text="Select an item to configure")
            return

        # Get the queue item for the selected tree item
        queue_item = self.get_queue_item_for_tree_item(selected[0])
        if not queue_item:
            return

        # Check if this is an ANALYZE-only operation
        is_analyze = queue_item.operation_type == OperationType.ANALYZE

        # Update properties panel with selected item's values
        if is_analyze:
            # Disable output-related fields for ANALYZE operations (no output file produced)
            self.item_output_mode.set("—")
            self.item_suffix.set("")
            self.item_output_folder.set("")
            self.item_source_label.config(text=f"{queue_item.source_path} (Analysis only - no output file)")
            # Disable the widgets
            self.item_mode_combo.config(state="disabled")
            self.item_suffix_entry.config(state="disabled")
            self.item_folder_entry.config(state="disabled")
            self.item_folder_browse_button.config(state="disabled")
        else:
            # Enable and populate for CONVERT operations
            self.item_mode_combo.config(state="readonly")
            self.item_suffix_entry.config(state="normal")
            self.item_folder_entry.config(state="normal")
            self.item_folder_browse_button.config(state="normal")
            self.item_output_mode.set(queue_item.output_mode.value)
            self.item_suffix.set(queue_item.output_suffix or self.default_suffix.get())
            self.item_output_folder.set(queue_item.output_folder or self.default_output_folder.get())
            self.item_source_label.config(text=queue_item.source_path)

    def on_item_output_mode_changed(self):
        """Handle output mode change for selected item."""
        selected = self.queue_tree.selection()
        if not selected:
            return

        queue_item = self.get_queue_item_for_tree_item(selected[0])
        if queue_item:
            queue_item.output_mode = OutputMode(self.item_output_mode.get())
            self.save_queue_to_config()
            self.refresh_queue_tree()

    def on_item_suffix_changed(self):
        """Handle suffix change for selected item."""
        selected = self.queue_tree.selection()
        if not selected:
            return

        queue_item = self.get_queue_item_for_tree_item(selected[0])
        if queue_item:
            queue_item.output_suffix = self.item_suffix.get()
            self.save_queue_to_config()
            self.refresh_queue_tree()

    def on_browse_item_output_folder(self):
        """Browse for item-specific output folder."""
        folder = filedialog.askdirectory(title="Select Output Folder")
        if not folder:
            return

        self.item_output_folder.set(folder)

        # Update selected item
        selected = self.queue_tree.selection()
        if not selected:
            return

        queue_item = self.get_queue_item_for_tree_item(selected[0])
        if queue_item:
            queue_item.output_folder = folder
            self.save_queue_to_config()
            self.refresh_queue_tree()

    def refresh_queue_tree(self):
        """Refresh the queue tree view from _queue_items."""
        # Clear existing items
        for item in self.queue_tree.get_children():
            self.queue_tree.delete(item)
        self._queue_tree_map.clear()
        self._tree_queue_map.clear()
        self._queue_items_by_id = {item.id: item for item in self._queue_items}

        index = get_history_index()

        # Add each queue item with order number
        for order, queue_item in enumerate(self._queue_items, start=1):
            # Get history record for metadata
            path_hash = compute_path_hash(queue_item.source_path)
            record = index.get(path_hash)

            # Determine operation display based on operation type and Layer 2 data
            if queue_item.operation_type == OperationType.ANALYZE:
                operation_display = "Analyze"
            elif queue_item.operation_type == OperationType.CONVERT:
                # Check if file has Layer 2 data (CRF search results)
                has_layer2 = record and record.best_crf is not None and record.best_vmaf_achieved is not None
                operation_display = "Convert" if has_layer2 else "Analyze+Convert"
            else:
                operation_display = "Unknown"

            # Format output mode display
            if queue_item.operation_type == OperationType.ANALYZE:
                output_display = "—"
            elif queue_item.output_mode == OutputMode.REPLACE:
                output_display = "Replace"
            elif queue_item.output_mode == OutputMode.SUFFIX:
                suffix = queue_item.output_suffix or self.default_suffix.get()
                output_display = f"{suffix}"
            else:
                folder_name = os.path.basename(queue_item.output_folder or self.default_output_folder.get() or "")
                output_display = f"→ {folder_name}/" if folder_name else "Separate"

            # Format size
            if record and record.file_size_bytes:
                size_display = format_file_size(record.file_size_bytes)
            elif os.path.isfile(queue_item.source_path):
                try:
                    size_display = format_file_size(os.path.getsize(queue_item.source_path))
                except OSError:
                    size_display = "—"
            else:
                size_display = "—"  # Folders show dash until scanned

            # Format estimated time
            est_time_display = "—"  # TODO: Could use estimation.py in future

            # Format status (now includes progress info)
            status_display = queue_item.format_status_display()
            if queue_item.status == QueueItemStatus.CONVERTING and queue_item.total_files > 0:
                status_display = f"Converting ({queue_item.processed_files}/{queue_item.total_files})"

            # Insert item with order number prefix
            icon = "📁" if queue_item.is_folder else "🎬"
            prefix = "▶ " if queue_item.is_folder else ""
            name = os.path.basename(queue_item.source_path)

            item_id = self.queue_tree.insert(
                "",
                "end",
                text=f"{order}. {prefix}{icon} {name}",
                values=(size_display, est_time_display, operation_display, output_display, status_display),
            )
            self._queue_tree_map[queue_item.id] = item_id
            self._tree_queue_map[item_id] = queue_item.id

        # Update total row
        total_items = len(self._queue_items)
        self.queue_total_tree.item("total", values=("", "", "", "", f"{total_items} items"))

    def sync_queue_order_from_tree(self):
        """Sync _queue_items order from the tree view after drag-drop reordering."""
        # Rebuild _queue_items in tree order using O(1) lookups
        new_order = []
        for tree_id in self.queue_tree.get_children():
            queue_id = self._tree_queue_map.get(tree_id)
            if queue_id:
                item = self._queue_items_by_id.get(queue_id)
                if item:
                    new_order.append(item)

        if len(new_order) == len(self._queue_items):
            self._queue_items = new_order
            self.save_queue_to_config()
            # Refresh to update order numbers
            self.refresh_queue_tree()

    # --- Analysis Tab Handlers ---

    def _on_folder_or_extension_changed(self, *args):
        """Auto-refresh analysis tree when folder or extensions change.

        Uses a debounce timer to avoid excessive refreshes during rapid changes.
        """
        # Cancel pending refresh if any
        if self._refresh_timer_id:
            self.root.after_cancel(self._refresh_timer_id)

        # Schedule refresh after 500ms delay (debounce)
        self._refresh_timer_id = self.root.after(500, self._refresh_analysis_tree)

    def _refresh_analysis_tree(self):
        """Start background scan to populate tree incrementally."""
        self._refresh_timer_id = None

        folder = self.input_folder.get()
        if not folder or not os.path.isdir(folder):
            # Clear tree if no valid folder
            for item in self.analysis_tree.get_children():
                self.analysis_tree.delete(item)
            self._update_add_all_buttons_state()
            return

        # Get selected extensions
        extensions = []
        if self.ext_mp4.get():
            extensions.append("mp4")
        if self.ext_mkv.get():
            extensions.append("mkv")
        if self.ext_avi.get():
            extensions.append("avi")
        if self.ext_wmv.get():
            extensions.append("wmv")

        if not extensions:
            # Clear tree if no extensions selected
            for item in self.analysis_tree.get_children():
                self.analysis_tree.delete(item)
            self._update_add_all_buttons_state()
            return

        # Clear tree and start background scan
        for item in self.analysis_tree.get_children():
            self.analysis_tree.delete(item)
        self._tree_item_map.clear()
        self._update_total_row(0, 0, 0, 0, 0, 0, 0.0)  # Reset total row
        self._update_add_all_buttons_state()  # Disable while scanning

        # Cancel any existing scan
        if self._scan_stop_event:
            self._scan_stop_event.set()

        # Start incremental background scan
        self._scan_stop_event = threading.Event()
        threading.Thread(
            target=self._incremental_scan_thread, args=(folder, extensions, self._scan_stop_event), daemon=True
        ).start()

    def _incremental_scan_thread(self, folder: str, extensions: list[str], stop_event: threading.Event):
        """Scan folder and populate tree incrementally from background thread.

        Uses breadth-first traversal - shows all top-level folders first,
        then their children, etc. This gives immediate visual feedback.

        Also checks HistoryIndex cache - if a file was previously analyzed,
        displays cached values immediately instead of "—".
        """
        root_folder = str(Path(folder).resolve())
        ext_set = {f".{ext.lower()}" for ext in extensions}
        file_count = 0
        folder_count = 0
        index = get_history_index()

        def scan_directory(dirpath: str) -> tuple[list[str], list[tuple[str, int, float]]]:
            """Scan a directory for subdirs and video files with stats.

            Returns:
                (subdirs, file_infos) where file_infos is list of (filename, size, mtime)
            """
            subdirs = []
            file_infos = []
            try:
                with os.scandir(dirpath) as entries:
                    for entry in entries:
                        if stop_event.is_set():
                            return [], []
                        if entry.is_dir(follow_symlinks=False):
                            subdirs.append(entry.path)
                        elif entry.is_file() and os.path.splitext(entry.name)[1].lower() in ext_set:
                            try:
                                stat = entry.stat()
                                file_infos.append((entry.name, stat.st_size, stat.st_mtime))
                            except OSError:
                                file_infos.append((entry.name, 0, 0))
            except (PermissionError, OSError):
                pass
            return sorted(subdirs, key=str.lower), sorted(file_infos, key=lambda x: x[0].lower())

        try:
            # BFS queue: (dirpath, parent_dirpath or None for root)
            queue: deque[tuple[str, str | None]] = deque()
            queue.append((root_folder, None))

            # Track folder tree IDs - populated by UI callbacks
            folder_tree_ids: dict[str, str] = {}
            folder_tree_ids[root_folder] = ""  # Root maps to tree root

            while queue and not stop_event.is_set():
                dirpath, parent_dirpath = queue.popleft()

                # Scan directory in background thread
                subdirs, file_infos = scan_directory(dirpath)
                if stop_event.is_set():
                    break

                # Get parent tree ID
                parent_tree_id = folder_tree_ids.get(parent_dirpath or root_folder, "")

                # Queue subdirectories for BFS
                for subdir in subdirs:
                    queue.append((subdir, dirpath))

                # Pre-compute cached values for each file (in background thread)
                # This avoids doing index lookups on the UI thread
                file_display_data = []
                for filename, file_size, file_mtime in file_infos:
                    file_path = os.path.join(dirpath, filename)
                    size_str = format_file_size(file_size)
                    savings_str = "—"
                    time_str = "—"
                    eff_str = "—"
                    tag = ""  # No tag by default

                    # Check cache (use tolerance for mtime due to float precision in JSON)
                    record = index.lookup_file(file_path)
                    if record and record.file_size_bytes == file_size and mtimes_match(record.file_mtime, file_mtime):
                        # Cache hit - use cached values
                        if record.status == FileStatus.CONVERTED:
                            savings_str = "Done"
                            time_str = "—"
                            tag = "done"
                        elif record.status == FileStatus.NOT_WORTHWHILE:
                            savings_str = "Skip"
                            time_str = "—"
                            tag = "skip"
                        else:
                            # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
                            has_layer2 = record.predicted_size_reduction is not None
                            reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent
                            if reduction_percent and record.file_size_bytes:
                                est_savings = int(record.file_size_bytes * reduction_percent / 100)
                                savings_str = format_file_size(est_savings)
                                if not has_layer2:
                                    savings_str = f"~{savings_str}"
                                file_time = estimate_file_time(
                                    codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
                                ).best_seconds
                                time_str = self._format_compact_time(file_time) if file_time > 0 else "—"
                                eff_str = self._format_efficiency(est_savings, file_time)

                    file_display_data.append((filename, file_path, size_str, savings_str, time_str, eff_str, tag))

                # Prepare UI update
                is_root = dirpath == root_folder
                folder_name = os.path.basename(dirpath) if not is_root else None

                # Use event to wait for UI update to complete
                done_event = threading.Event()
                new_folder_id: list[str] = [""]  # Mutable container to get result back

                def add_to_tree(
                    dp=dirpath,
                    pid=parent_tree_id,
                    fname=folder_name,
                    fdata=file_display_data,
                    is_rt=is_root,
                    result=new_folder_id,
                    done=done_event,
                ):
                    nonlocal file_count, folder_count
                    try:
                        if is_rt:
                            # Root folder: add files at tree root, no folder node
                            folder_id = ""
                            for filename, file_path, size_str, savings_str, time_str, eff_str, tag in fdata:
                                item_id = self.analysis_tree.insert(
                                    "",
                                    "end",
                                    text=f"🎬 {filename}",
                                    values=(size_str, savings_str, time_str, eff_str),
                                    tags=(tag,) if tag else (),
                                )
                                self._tree_item_map[file_path] = item_id
                                file_count += 1
                        else:
                            # Non-root: create folder node and add files
                            folder_id = self.analysis_tree.insert(
                                pid, "end", text=f"▶ 📁 {fname}", values=("—", "—", "—", "—"), open=False
                            )
                            folder_count += 1
                            for filename, file_path, size_str, savings_str, time_str, eff_str, tag in fdata:
                                item_id = self.analysis_tree.insert(
                                    folder_id,
                                    "end",
                                    text=f"🎬 {filename}",
                                    values=(size_str, savings_str, time_str, eff_str),
                                    tags=(tag,) if tag else (),
                                )
                                self._tree_item_map[file_path] = item_id
                                file_count += 1
                            # Update folder aggregate from its files
                            if fdata:
                                self._update_folder_aggregates(folder_id)
                        result[0] = folder_id
                    finally:
                        done.set()

                update_ui_safely(self.root, add_to_tree)
                done_event.wait(timeout=5.0)  # Wait for UI thread

                # Store folder ID for children to use
                folder_tree_ids[dirpath] = new_folder_id[0]

            # Final status
            if stop_event.is_set():
                self._finish_incremental_scan(stopped=True)
                return

            update_ui_safely(self.root, lambda: self._finish_incremental_scan(stopped=False))

        except PermissionError:
            logger.exception("Permission denied during scan")
            if not stop_event.is_set():
                update_ui_safely(self.root, lambda: self._finish_incremental_scan(stopped=False))
        except OSError:
            logger.exception("OS error during scan")
            if not stop_event.is_set():
                update_ui_safely(self.root, lambda: self._finish_incremental_scan(stopped=False))
        except Exception:
            logger.exception("Error during incremental scan")
            if not stop_event.is_set():
                update_ui_safely(self.root, lambda: self._finish_incremental_scan(stopped=False))

    def _prune_empty_folders(self) -> int:
        """Remove folders with no children from the tree (runs on UI thread).

        Runs repeatedly until no more empty folders are found, handling
        the case where removing a child folder makes its parent empty.

        Returns:
            Total number of folders removed.
        """
        total_removed = 0

        while True:
            removed_this_pass = 0
            items_to_check = list(self.analysis_tree.get_children(""))

            while items_to_check:
                item_id = items_to_check.pop()
                children = self.analysis_tree.get_children(item_id)

                if children:
                    # Has children - check them too
                    items_to_check.extend(children)
                else:
                    # No children - if it's a folder, remove it
                    text = self.analysis_tree.item(item_id, "text")
                    if "📁" in text:
                        self.analysis_tree.delete(item_id)
                        removed_this_pass += 1

            total_removed += removed_this_pass
            if removed_this_pass == 0:
                break  # No more empty folders

        return total_removed

    def _finish_incremental_scan(self, stopped: bool):
        """Clean up after incremental scan completes (runs on UI thread)."""
        if not stopped:
            # Prune empty folders after scan completes
            self._prune_empty_folders()

        # Update total row with any cached data
        self._update_total_from_tree()

        # Sync queue tags to show which files are in the conversion queue
        self.sync_queue_tags_to_analysis_tree()

        # Enable Add All buttons if there are files
        self._update_add_all_buttons_state()

    def on_analyze_folders(self):
        """Run ffprobe analysis on files already in the tree.

        The tree is populated by the incremental scan when the tab opens or
        folder changes. This button runs ffprobe to get file metadata and
        estimate potential savings.
        """
        output_folder = self.output_folder.get()
        if not output_folder:
            messagebox.showwarning("Invalid Folder", "Please select an output folder for analysis.")
            return

        # Get file paths from existing tree
        file_paths = list(self._tree_item_map.keys())

        if not file_paths:
            messagebox.showinfo(
                "No Files", "No files to analyze. Select a folder with video files and wait for the scan to complete."
            )
            return

        # Disable buttons immediately to prevent double-clicks
        self.analyze_button.config(state="disabled")
        self.add_all_analyze_button.config(state="disabled")
        self.add_all_convert_button.config(state="disabled")

        input_folder = self.input_folder.get()
        anonymize = self.anonymize_history.get()
        logger.info(f"Starting ffprobe analysis of {len(file_paths)} files in: {input_folder}")

        # Cancel any pending auto-refresh timer
        if self._refresh_timer_id:
            self.root.after_cancel(self._refresh_timer_id)
            self._refresh_timer_id = None

        # Cancel any running analysis or incremental scan
        if self.analysis_stop_event:
            self.analysis_stop_event.set()
        if self._scan_stop_event:
            self._scan_stop_event.set()

        # Run ffprobe analysis in background thread
        # Pass input_folder and anonymize as parameters (captured on main thread)
        self.analysis_stop_event = threading.Event()
        self.analysis_thread = threading.Thread(
            target=self._run_ffprobe_analysis, args=(file_paths, output_folder, input_folder, anonymize), daemon=True
        )
        self.analysis_thread.start()

    def _run_ffprobe_analysis(self, file_paths: list[str], output_folder: str, input_folder: str, anonymize: bool):
        """Run ffprobe analysis on files in parallel.

        This analyzes files already in the tree using ffprobe to get metadata
        and estimate potential savings. Updates tree rows as results come in.

        Files with valid cache entries return quickly (no ffprobe needed).
        Cache checking is done inside each parallel worker, so there's no
        blocking pre-filter step.

        Args:
            file_paths: List of file paths to analyze.
            output_folder: Output folder for checking if files are already converted.
            input_folder: Input folder path (captured from main thread).
            anonymize: Whether to anonymize history (captured from main thread).
        """
        index = get_history_index()
        root_path = Path(input_folder).resolve()
        output_path = Path(output_folder).resolve()

        total_files = len(file_paths)
        files_completed = 0
        cache_hits = 0
        max_workers = min(8, max(4, total_files // 10 + 1))

        def analyze_one_file(file_path: str) -> tuple[str | None, bool]:
            """Analyze a single file (runs in thread pool).

            Checks cache first - if valid, skips ffprobe.

            Returns:
                Tuple of (file_path or None, was_cache_hit).
            """
            if self.analysis_stop_event and self.analysis_stop_event.is_set():
                return None, False

            # Check cache first - if valid, skip ffprobe
            try:
                stat = os.stat(file_path)
                cached = index.lookup_file(file_path)
                if cached and cached.file_size_bytes == stat.st_size and mtimes_match(cached.file_mtime, stat.st_mtime):
                    return file_path, True  # Cache hit - no ffprobe needed
            except OSError:
                pass  # Let _analyze_file handle the error

            # Cache miss - run full analysis with ffprobe
            try:
                _analyze_file(file_path, root_path, output_path, index, anonymize)
                return file_path, False
            except Exception:
                logger.exception(f"Error analyzing {os.path.basename(file_path)}")
                return None, False

        with ThreadPoolExecutor(max_workers=max_workers) as executor:
            futures = {executor.submit(analyze_one_file, fp): fp for fp in file_paths}
            pending = set(futures.keys())

            while pending:
                # Check stop event before waiting for futures
                if self.analysis_stop_event and self.analysis_stop_event.is_set():
                    logger.info("Analysis interrupted by user")
                    executor.shutdown(wait=True, cancel_futures=True)
                    index.save()
                    update_ui_safely(self.root, self._on_ffprobe_complete)
                    return

                # Wait for futures with timeout to allow periodic stop checks
                done, pending = wait(pending, timeout=0.5, return_when=FIRST_COMPLETED)

                # Collect completed files for batch UI update
                completed_paths: list[str] = []

                for future in done:
                    file_path, was_cached = future.result()
                    files_completed += 1
                    if was_cached:
                        cache_hits += 1
                    if file_path:
                        completed_paths.append(file_path)

                # Single batched UI update for all completed files in this round
                if completed_paths:
                    paths_snapshot = list(completed_paths)
                    update_ui_safely(self.root, lambda paths=paths_snapshot: self._batch_update_tree_rows(paths))

                # Update totals and save less frequently (every batch or 5% progress)
                batch_interval = TREE_UPDATE_BATCH_SIZE
                pct_interval = max(1, total_files // 20)  # 5% increments
                if files_completed % batch_interval == 0 or (
                    total_files > MIN_FILES_FOR_PERCENT_UPDATES and files_completed % pct_interval == 0
                ):
                    update_ui_safely(self.root, self._update_total_from_tree)
                    index.save()

        # Save index after all files processed (handles remainder)
        index.save()

        # Log cache efficiency
        if cache_hits > 0:
            logger.info(f"Analysis complete: {cache_hits}/{total_files} from cache")

        # Analysis complete
        update_ui_safely(self.root, self._on_ffprobe_complete)

    def _batch_update_tree_rows(self, file_paths: list[str]) -> None:
        """Update multiple tree rows efficiently without per-file folder updates.

        Updates file rows and collects affected folders, then updates folder
        aggregates once at the end. Much faster than calling _update_tree_row
        for each file individually.

        Args:
            file_paths: List of file paths to update.
        """
        if not file_paths:
            return

        index = get_history_index()
        affected_folders: set[str] = set()

        for file_path in file_paths:
            item_id = self._tree_item_map.get(file_path)
            if not item_id:
                continue

            record = index.lookup_file(file_path)
            if not record:
                continue

            # Calculate display values
            size_str = format_file_size(record.file_size_bytes) if record.file_size_bytes else "—"
            tag = ""
            # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
            has_layer2 = record.predicted_size_reduction is not None
            reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent
            if record.status == FileStatus.SCANNED and reduction_percent and record.file_size_bytes:
                file_savings = int(record.file_size_bytes * reduction_percent / 100)
                savings_str = format_file_size(file_savings)
                if not has_layer2:
                    savings_str = f"~{savings_str}"
                file_time = estimate_file_time(
                    codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
                ).best_seconds
                time_str = self._format_compact_time(file_time) if file_time > 0 else "—"
                eff_str = self._format_efficiency(file_savings, file_time)
            elif record.status == FileStatus.CONVERTED:
                savings_str = "Done"
                time_str = "—"
                eff_str = "—"
                tag = "done"
            elif record.status == FileStatus.NOT_WORTHWHILE:
                savings_str = "Skip"
                time_str = "—"
                eff_str = "—"
                tag = "skip"
            else:
                savings_str = "—"
                time_str = "—"
                eff_str = "—"

            # Update tree item
            self.analysis_tree.item(
                item_id, values=(size_str, savings_str, time_str, eff_str), tags=(tag,) if tag else ()
            )

            # Track parent folder for batch aggregate update
            parent_id = self.analysis_tree.parent(item_id)
            if parent_id:
                affected_folders.add(parent_id)

        # Update folder aggregates once for all affected folders
        # Pre-build reverse map once for all folder updates
        if affected_folders:
            item_to_path = {item_id: path for path, item_id in self._tree_item_map.items()}
            for folder_id in affected_folders:
                self._update_folder_aggregates(folder_id, item_to_path)

    def _on_ffprobe_complete(self):
        """Handle ffprobe analysis completion (called on main thread)."""
        self.analyze_button.config(state="normal")

        # Update total row
        self._update_total_from_tree()

        # Enable Add All buttons
        self._update_add_all_buttons_state()

    def _update_add_all_buttons_state(self):
        """Enable/disable the Add All buttons based on whether there are files in the tree."""
        has_files = bool(self._tree_item_map)
        state = "normal" if has_files else "disabled"
        self.add_all_analyze_button.config(state=state)
        self.add_all_convert_button.config(state=state)

    def on_add_all_analyze(self):
        """Add all discovered files to the queue for CRF analysis."""
        file_paths = list(self._tree_item_map.keys())
        if not file_paths:
            messagebox.showinfo("No Files", "No files to add. Run a scan first.")
            return

        # Build items list (all files, not folders)
        items = [(path, False) for path in file_paths]

        # Use preview dialog for bulk operations
        result = self.add_items_to_queue(items, OperationType.ANALYZE, force_preview=True)

        total_added = result["added"] + result["conflict_added"] + result["conflict_replaced"]
        if total_added > 0:
            self.tab_control.select(self.convert_tab)

    def on_add_all_convert(self):
        """Add all discovered files to the queue for conversion."""
        file_paths = list(self._tree_item_map.keys())
        if not file_paths:
            messagebox.showinfo("No Files", "No files to add. Run a scan first.")
            return

        # Build items list (all files, not folders)
        items = [(path, False) for path in file_paths]

        # Use preview dialog for bulk operations
        result = self.add_items_to_queue(items, OperationType.CONVERT, force_preview=True)

        total_added = result["added"] + result["conflict_added"] + result["conflict_replaced"]
        if total_added > 0:
            self.tab_control.select(self.convert_tab)

    def get_file_path_for_tree_item(self, item_id: str) -> str | None:
        """Look up the file path for a given tree item ID.

        Args:
            item_id: The tree item ID to look up.

        Returns:
            The file path, or None if not found.
        """
        for path, tid in self._tree_item_map.items():
            if tid == item_id:
                return path
        return None

    def get_analysis_tree_tooltip(self, item_id: str) -> str | None:
        """Generate tooltip text for an analysis tree item.

        Args:
            item_id: The tree item ID to generate tooltip for.

        Returns:
            Tooltip text, or None if no tooltip should be shown.
        """
        # Check if it's a folder (has children)
        if self.analysis_tree.get_children(item_id):
            return None  # No tooltip for folders

        # Get file path for this item
        file_path = self.get_file_path_for_tree_item(item_id)
        if not file_path:
            return None

        # Look up record from history index
        index = get_history_index()
        record = index.lookup_file(file_path)

        if not record:
            return "Not yet analyzed. Click Analyze to scan."

        # Generate tooltip based on status
        if record.status == FileStatus.CONVERTED:
            # Show conversion details
            lines = ["Already converted"]
            if record.reduction_percent is not None:
                lines[0] += f": {record.reduction_percent:.0f}% smaller"
            if record.final_crf is not None and record.final_vmaf is not None:
                lines.append(f"CRF {record.final_crf}, VMAF {record.final_vmaf:.1f}")
            return "\n".join(lines)

        if record.status == FileStatus.NOT_WORTHWHILE:
            # Show skip reason
            if record.skip_reason:
                return f"Skipped: {record.skip_reason}"
            if record.min_vmaf_attempted:
                return f"Skipped: VMAF {record.min_vmaf_attempted} unattainable"
            return "Skipped: Quality target unattainable"

        # FileStatus.SCANNED - check analysis level
        if record.predicted_size_reduction is not None:
            # Layer 2 complete (CRF search done)
            lines = ["Ready to convert (CRF search complete)"]
            if record.best_crf is not None and record.best_vmaf_achieved is not None:
                lines.append(f"CRF {record.best_crf} → VMAF {record.best_vmaf_achieved:.1f}")
            return "\n".join(lines)

        if record.estimated_reduction_percent is not None:
            # Layer 1 only (ffprobe estimate)
            if record.estimated_from_similar and record.estimated_from_similar > 0:
                return f"Estimate based on {record.estimated_from_similar} similar file(s)"
            return "Estimate based on typical reduction"

        return "Not yet analyzed. Click Analyze to scan."

    def _update_tree_row(self, file_path: str):
        """Update a single file row with data from history index.

        Args:
            file_path: Path to the file that was analyzed.
        """
        # Find the tree item by file_path
        item_id = self._tree_item_map.get(file_path)
        if not item_id:
            return

        # Get file data from history index
        index = get_history_index()
        record = index.lookup_file(file_path)

        if not record:
            return

        has_layer2 = record.predicted_size_reduction is not None
        # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
        reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent

        # Calculate display values based on record status
        size_str = format_file_size(record.file_size_bytes) if record.file_size_bytes else "—"
        tag = ""
        if record.status == FileStatus.SCANNED and reduction_percent and record.file_size_bytes:
            # File needs conversion - show estimates
            file_savings = int(record.file_size_bytes * reduction_percent / 100)
            savings_str = format_file_size(file_savings)
            if not has_layer2:
                savings_str = f"~{savings_str}"

            file_time = estimate_file_time(
                codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
            ).best_seconds
            time_str = self._format_compact_time(file_time) if file_time > 0 else "—"
            eff_str = self._format_efficiency(file_savings, file_time)
        elif record.status == FileStatus.CONVERTED:
            savings_str = "Done"
            time_str = "—"
            eff_str = "—"
            tag = "done"
        elif record.status == FileStatus.NOT_WORTHWHILE:
            savings_str = "Skip"
            time_str = "—"
            eff_str = "—"
            tag = "skip"
        else:
            # No data yet
            savings_str = "—"
            time_str = "—"
            eff_str = "—"

        # Update tree item
        self.analysis_tree.item(item_id, values=(size_str, savings_str, time_str, eff_str), tags=(tag,) if tag else ())

        # Update parent folder aggregates
        parent_id = self.analysis_tree.parent(item_id)
        if parent_id:
            self._update_folder_aggregates(parent_id)

    def _update_folder_aggregates(self, folder_id: str, item_to_path: dict[str, str] | None = None):
        """Recalculate and update folder aggregate values from history index.

        Args:
            folder_id: The tree item ID of the folder to update.
            item_to_path: Optional pre-built reverse map of item_id -> file_path.
                         If not provided, builds one (slower for batch updates).
        """
        if item_to_path is None:
            item_to_path = {item_id: path for path, item_id in self._tree_item_map.items()}

        # Get all children (files)
        children = self.analysis_tree.get_children(folder_id)

        # Sum up size, savings and time from all files using history index
        total_size = 0
        total_savings = 0
        total_time = 0
        any_estimate = False  # Track if any file lacks CRF search (layer 2) data

        index = get_history_index()

        for child_id in children:
            file_path = item_to_path.get(child_id)
            if not file_path:
                continue

            # Look up file data from history index
            record = index.lookup_file(file_path)
            if not record:
                continue

            # Sum size for all files
            if record.file_size_bytes:
                total_size += record.file_size_bytes

            # Check if file needs conversion and has estimates
            # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
            reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent
            if record.status == FileStatus.SCANNED and reduction_percent:
                # Track if this file only has ffprobe-level analysis (no CRF search)
                if record.predicted_size_reduction is None:
                    any_estimate = True

                # Calculate savings from reduction percentage
                if record.file_size_bytes:
                    file_savings = int(record.file_size_bytes * reduction_percent / 100)
                    total_savings += file_savings

                # Get time estimate
                file_time = estimate_file_time(
                    codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
                ).best_seconds
                total_time += file_time

        # Update folder display (efficiency = aggregate savings / aggregate time)
        size_str = format_file_size(total_size) if total_size > 0 else "—"
        savings_str = format_file_size(total_savings) if total_savings > 0 else "—"
        if any_estimate and savings_str != "—":
            savings_str = f"~{savings_str}"
        time_str = self._format_compact_time(total_time) if total_time > 0 else "—"
        eff_str = self._format_efficiency(total_savings, total_time)
        self.analysis_tree.item(folder_id, values=(size_str, savings_str, time_str, eff_str))

    def _format_compact_time(self, seconds: float) -> str:
        """Format time in a compact way for the analysis tree.

        Args:
            seconds: Time in seconds

        Returns:
            Formatted string like "2h 15m", "45m", "12m"
        """
        if seconds <= 0:
            return "—"

        hours = int(seconds // 3600)
        minutes = int((seconds % 3600) // 60)

        if hours > 0:
            return f"{hours}h {minutes}m"
        if minutes > 0:
            return f"{minutes}m"
        return "< 1m"

    def _format_efficiency(self, savings_bytes: int, time_seconds: float) -> str:
        """Format efficiency (savings per time) for display.

        Args:
            savings_bytes: Estimated savings in bytes
            time_seconds: Estimated conversion time in seconds

        Returns:
            Formatted string like "2.5 GB/h", "12 GB/h", or "—"
        """
        if savings_bytes <= 0 or time_seconds <= 0:
            return "—"

        # Calculate GB saved per hour
        gb_per_hr = (savings_bytes / 1_073_741_824) / (time_seconds / 3600)

        # No decimals for >= 10 GB/h
        if gb_per_hr >= _EFFICIENCY_DECIMAL_THRESHOLD:
            return f"{gb_per_hr:.0f} GB/h"

        # Show one decimal for smaller values
        return f"{gb_per_hr:.1f} GB/h"

    def _get_queued_file_paths(self) -> set[str]:
        """Get set of all file paths currently in the conversion queue.

        For folder queue items, finds all video files under that folder
        that are in the analysis tree. For file queue items, returns the path directly.

        Returns:
            Set of file paths that are in pending/converting queue items.
        """
        queued_paths: set[str] = set()

        for item in self._queue_items:
            if item.status not in ("pending", "converting"):
                continue

            if item.is_folder:
                # Find all files in _tree_item_map that are under this folder
                folder_prefix = item.source_path + os.sep
                for file_path in self._tree_item_map:
                    if file_path.startswith(folder_prefix) or file_path == item.source_path:
                        queued_paths.add(file_path)
            else:
                # Single file
                queued_paths.add(item.source_path)

        return queued_paths

    def sync_queue_tags_to_analysis_tree(self):
        """Synchronize queue status to analysis tree item tags.

        Applies 'in_queue' tag to files in the queue, 'partial_queue' to folders
        with some (but not all) files queued, and removes queue tags from items
        no longer in queue.
        """
        if not hasattr(self, "analysis_tree") or not self._tree_item_map:
            return

        queued_paths = self._get_queued_file_paths()

        # Track which folders need updating and their child stats
        folder_stats: dict[str, tuple[int, int]] = {}  # folder_id -> (queued_count, total_count)

        # Update file tags
        for file_path, item_id in self._tree_item_map.items():
            try:
                if not self.analysis_tree.exists(item_id):
                    continue

                current_tags = list(self.analysis_tree.item(item_id, "tags") or ())

                # Remove existing queue tags
                current_tags = [t for t in current_tags if t not in ("in_queue", "partial_queue")]

                # Add queue tag if file is in queue
                is_queued = file_path in queued_paths
                if is_queued:
                    current_tags.append("in_queue")

                self.analysis_tree.item(item_id, tags=tuple(current_tags))

                # Track parent folder stats
                parent_id = self.analysis_tree.parent(item_id)
                if parent_id:
                    queued, total = folder_stats.get(parent_id, (0, 0))
                    folder_stats[parent_id] = (queued + (1 if is_queued else 0), total + 1)

            except tk.TclError:
                continue

        # Update folder tags based on child stats
        for folder_id, (queued_count, total_count) in folder_stats.items():
            try:
                if not self.analysis_tree.exists(folder_id):
                    continue

                current_tags = list(self.analysis_tree.item(folder_id, "tags") or ())
                current_tags = [t for t in current_tags if t not in ("in_queue", "partial_queue")]

                if queued_count == total_count and total_count > 0:
                    # All children queued
                    current_tags.append("in_queue")
                elif queued_count > 0:
                    # Some children queued
                    current_tags.append("partial_queue")

                self.analysis_tree.item(folder_id, tags=tuple(current_tags))
            except tk.TclError:
                continue

    def update_analysis_tree_for_completed_file(self, file_path: str, status: str):
        """Update analysis tree entry when a file completes conversion.

        Args:
            file_path: Full path to the completed file.
            status: Completion status - "done" for successful, "skip" for not worthwhile.
        """
        if not hasattr(self, "analysis_tree"):
            return

        item_id = self._tree_item_map.get(file_path)
        if not item_id:
            return

        try:
            if not self.analysis_tree.exists(item_id):
                return

            # Get current tags and remove queue-related ones
            current_tags = list(self.analysis_tree.item(item_id, "tags") or ())
            current_tags = [t for t in current_tags if t not in ("in_queue", "partial_queue", "done", "skip")]

            # Add the completion status tag
            current_tags.append(status)

            # Update the tree item
            if status == "done":
                self.analysis_tree.item(item_id, tags=tuple(current_tags))
                self.analysis_tree.set(item_id, "savings", "Done")
            elif status == "skip":
                self.analysis_tree.item(item_id, tags=tuple(current_tags))
                self.analysis_tree.set(item_id, "savings", "Skip")

            # Clear time and efficiency for completed files
            self.analysis_tree.set(item_id, "time", "—")
            self.analysis_tree.set(item_id, "efficiency", "—")

            # Update parent folder aggregates
            parent_id = self.analysis_tree.parent(item_id)
            if parent_id:
                self._update_folder_aggregates(parent_id)

            # Sync queue tags since this file is no longer "in queue" effectively
            self.sync_queue_tags_to_analysis_tree()

        except tk.TclError:
            pass

    def _update_total_row(
        self,
        total_files: int,
        convertible: int,
        done_count: int,
        skip_count: int,
        total_size: int,
        total_savings: int,
        total_time: float,
        any_estimate: bool = False,
    ) -> None:
        """Update the fixed total row at the bottom of the analysis tree.

        Args:
            total_files: Total number of files in the tree.
            convertible: Number of files that can be converted.
            done_count: Number of already converted files.
            skip_count: Number of files skipped (not worthwhile).
            total_size: Total size of all files in bytes.
            total_savings: Estimated total savings in bytes.
            total_time: Estimated total time in seconds.
            any_estimate: If True, at least one file lacks CRF search data.
        """
        # Build breakdown string with only non-zero counts
        parts = []
        if convertible > 0:
            parts.append(f"{convertible} convertible")
        if done_count > 0:
            parts.append(f"{done_count} done")
        if skip_count > 0:
            parts.append(f"{skip_count} skipped")

        name_text = f"Total: {', '.join(parts)} / {total_files} files" if parts else f"Total ({total_files} files)"

        size_str = format_file_size(total_size) if total_size > 0 else "—"
        savings_str = format_file_size(total_savings) if total_savings > 0 else "—"
        if any_estimate and savings_str != "—":
            savings_str = f"~{savings_str}"
        time_str = self._format_compact_time(total_time) if total_time > 0 else "—"
        eff_str = self._format_efficiency(total_savings, total_time)

        # Update the total row
        self.analysis_total_tree.item("total", text=name_text, values=(size_str, savings_str, time_str, eff_str))

    def _update_total_from_tree(self) -> int:
        """Compute and update totals from files in the tree using history index.

        Iterates through all files in _tree_item_map, looks up their records
        in the history index, and sums up savings/time for convertible files.

        Returns:
            Number of convertible files found.
        """
        index = get_history_index()
        total_files = len(self._tree_item_map)
        convertible = 0
        done_count = 0
        skip_count = 0
        total_size = 0
        total_savings = 0
        total_time = 0.0
        any_estimate = False  # Track if any file lacks CRF search (layer 2) data

        for file_path in self._tree_item_map:
            record = index.lookup_file(file_path)
            if not record:
                continue
            # Sum size for all files
            if record.file_size_bytes:
                total_size += record.file_size_bytes
            if record.status == FileStatus.CONVERTED:
                done_count += 1
            elif record.status == FileStatus.NOT_WORTHWHILE:
                skip_count += 1
            else:
                # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
                reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent
                if record.status == FileStatus.SCANNED and reduction_percent:
                    convertible += 1
                    # Track if this file only has ffprobe-level analysis (no CRF search)
                    if record.predicted_size_reduction is None:
                        any_estimate = True
                    if record.file_size_bytes:
                        total_savings += int(record.file_size_bytes * reduction_percent / 100)
                    file_time = estimate_file_time(
                        codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
                    ).best_seconds
                    total_time += file_time

        self._update_total_row(
            total_files, convertible, done_count, skip_count, total_size, total_savings, total_time, any_estimate
        )
        return convertible

    def _parse_size_to_bytes(self, size_str: str) -> float:
        """Parse formatted size string to bytes for sorting.

        Args:
            size_str: Formatted size like "~1.2 GB", "500 MB", or "—"

        Returns:
            Size in bytes, or float('inf') for "—"
        """
        if size_str == "—":
            return float("inf")

        # Remove ~ prefix if present
        size_str = size_str.lstrip("~").strip()

        # Parse value and unit
        parts = size_str.split()
        expected_parts = 2
        if len(parts) != expected_parts:
            return float("inf")

        try:
            value = float(parts[0])
            unit = parts[1].upper()

            # Convert to bytes
            multipliers = {"B": 1, "KB": 1024, "MB": 1024**2, "GB": 1024**3, "TB": 1024**4}
            return value * multipliers.get(unit, 1)
        except (ValueError, KeyError):
            return float("inf")

    def _parse_time_to_seconds(self, time_str: str) -> float:
        """Parse formatted time string to seconds for sorting.

        Args:
            time_str: Formatted time like "2h 15m", "45m", or "—"

        Returns:
            Time in seconds, or float('inf') for "—"
        """
        if time_str in {"—", "< 1m"}:
            return float("inf") if time_str == "—" else 30  # Treat "< 1m" as 30 seconds

        total_seconds = 0.0
        # Parse patterns like "2h 15m" or "45m"
        parts = time_str.split()
        for part in parts:
            try:
                if part.endswith("h"):
                    total_seconds += float(part[:-1]) * 3600
                elif part.endswith("m"):
                    total_seconds += float(part[:-1]) * 60
            except ValueError:
                continue

        return total_seconds if total_seconds > 0 else float("inf")

    def _parse_efficiency_to_value(self, eff_str: str) -> float:
        """Parse formatted efficiency string to numeric value for sorting.

        Args:
            eff_str: Formatted efficiency like "2.5 GB/h", "12 GB/h", or "—"

        Returns:
            Efficiency in GB/hr, or float('-inf') for "—" (sorts last when descending)
        """
        if eff_str == "—":
            return float("-inf")  # Sort "—" last when sorting by efficiency (descending)

        try:
            parts = eff_str.split()
            expected_parts = 2
            if len(parts) != expected_parts:
                return float("-inf")

            value = float(parts[0])
            unit = parts[1]

            if unit == "GB/h":
                return value
            return float("-inf")
        except (ValueError, IndexError):
            return float("-inf")

    def sort_analysis_tree(self, col: str):
        """Sort the analysis tree by the specified column.

        Sorting is done within each parent (preserves hierarchy).
        Folders sort before files when sorting by Name.
        Toggle direction on repeated clicks.

        Args:
            col: Column to sort by ("#0" for Name, "savings", "time", or "efficiency")
        """
        # Toggle direction if same column clicked
        if self._sort_col == col:
            self._sort_reverse = not self._sort_reverse
        else:
            self._sort_col = col
            self._sort_reverse = False

        def get_sort_key(item_id: str) -> tuple:
            """Get sort key for an item.

            Returns tuple: (is_file, sort_value)
            - Folders always sort before files (is_file=False for folders)
            - Sort value depends on column
            """
            # Check if item is a file (has parent) or folder (no parent or root)
            parent = self.analysis_tree.parent(item_id)
            is_file = bool(parent)

            if col == "#0":
                # Sort by name
                text = self.analysis_tree.item(item_id, "text")
                # Remove arrows, icons, and leading spaces
                name = text.replace("▶", "").replace("▼", "").replace("📁", "").replace("🎬", "").strip()
                return (is_file, name.lower())
            if col == "size":
                # Sort by file size (values[0])
                values = self.analysis_tree.item(item_id, "values")
                if values and len(values) >= 1:
                    size_bytes = self._parse_size_to_bytes(values[0])
                    return (is_file, size_bytes)
                return (is_file, float("inf"))
            if col == "savings":
                # Sort by estimated savings (values[1])
                values = self.analysis_tree.item(item_id, "values")
                if values and len(values) >= 2:  # noqa: PLR2004 - column index bounds check
                    size_bytes = self._parse_size_to_bytes(values[1])
                    return (is_file, size_bytes)
                return (is_file, float("inf"))
            if col == "time":
                # Sort by estimated time (values[2])
                values = self.analysis_tree.item(item_id, "values")
                if values and len(values) >= 3:  # noqa: PLR2004 - column index bounds check
                    time_seconds = self._parse_time_to_seconds(values[2])
                    return (is_file, time_seconds)
                return (is_file, float("inf"))
            if col == "efficiency":
                # Sort by efficiency (values[3], higher is better, so negate for default ascending sort)
                values = self.analysis_tree.item(item_id, "values")
                if values and len(values) >= 4:  # noqa: PLR2004 - column index bounds check
                    eff_value = self._parse_efficiency_to_value(values[3])
                    # Negate so higher efficiency sorts first in ascending order
                    return (is_file, -eff_value)
                return (is_file, float("inf"))
            return (is_file, "")

        def sort_children(parent_id: str):
            """Sort children of a parent node recursively."""
            children = list(self.analysis_tree.get_children(parent_id))
            if not children:
                return

            # Sort children
            children_sorted = sorted(children, key=get_sort_key, reverse=self._sort_reverse)

            # Reorder in tree
            for index, item_id in enumerate(children_sorted):
                self.analysis_tree.move(item_id, parent_id, index)

            # Recursively sort children of each child (for folders)
            for child_id in children_sorted:
                if self.analysis_tree.get_children(child_id):  # Has children (is a folder)
                    sort_children(child_id)

        # Sort root level items and their children recursively
        sort_children("")

        logger.debug(f"Sorted analysis tree by {col}, reverse={self._sort_reverse}")
