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
import threading
import tkinter as tk
from tkinter import filedialog, messagebox, ttk
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from src.gui.charts import BarChart, LineGraph, PieChart

from src.estimation import estimate_file_time
from src.gui import analysis_scanner, dependency_manager, queue_manager, tree_state_manager
from src.gui.conversion_controller import force_stop_conversion, start_conversion, stop_conversion
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
from src.gui.tree_formatters import (
    clear_sort_state,
    format_compact_time,
    format_efficiency,
    sort_analysis_tree,
    update_sort_indicators,
)
from src.history_index import compute_path_hash, get_history_index
from src.models import ConversionSessionState, FileStatus, OperationType, OutputMode, QueueItem, QueueItemStatus

# Import setup_logging only needed here now - Replace 'convert_app' with 'src'
from src.utils import format_file_size, get_script_directory, scrub_history_paths, scrub_log_files, setup_logging

logger = logging.getLogger(__name__)


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
    analysis_scan_overlay: tk.Frame

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
        # File-level tree mappings (for nested files under folder items)
        self._queue_file_tree_map: dict[str, str] = {}  # file_path -> tree_item_id
        self._tree_file_map: dict[str, tuple[str, str]] = {}  # tree_item_id -> (queue_item_id, file_path)

        # Analysis state
        self.analysis_stop_event: threading.Event | None = None
        self.analysis_thread: threading.Thread | None = None
        self._tree_item_map: dict[str, str] = {}  # Map file_path -> tree_item_id for updates
        self._refresh_timer_id: str | None = None  # Debounce timer for auto-refresh
        self._scan_stop_event: threading.Event | None = None  # Stop event for background scan
        self._scanning: bool = False  # True while background scan is running
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

    def get_tree_item_map(self) -> dict[str, str]:
        """Return the analysis tree item map (file_path -> tree_item_id)."""
        return self._tree_item_map

    def get_queue_tree_id(self, queue_item_id: str) -> str | None:
        """Return the tree item ID for a queue item, or None if not found."""
        return self._queue_tree_map.get(queue_item_id)

    def get_file_tree_id(self, file_path: str) -> str | None:
        """Return the tree item ID for a file within a folder queue item, or None if not found."""
        return self._queue_file_tree_map.get(file_path)

    def get_file_path_for_queue_tree_item(self, tree_id: str) -> str | None:
        """Return the file path for a nested file tree item, or None if not a file row."""
        file_info = self._tree_file_map.get(tree_id)
        return file_info[1] if file_info else None

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
        dependency_manager.check_ab_av1_updates(self)

    def on_check_ffmpeg_updates(self):
        """Check GitHub for the latest FFmpeg version and update the label."""
        dependency_manager.check_ffmpeg_updates(self)

    def on_download_ab_av1(self):
        """Download ab-av1 from GitHub in a background thread."""
        dependency_manager.download_ab_av1_update(self)

    def on_download_ffmpeg(self):
        """Download FFmpeg to vendor folder. Shows dialog if system FFmpeg exists."""
        dependency_manager.download_ffmpeg_update(self)

    def _load_queue_from_config(self) -> list[QueueItem]:
        return queue_manager.load_queue_from_config(self)

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
        return queue_manager.find_existing_queue_item(self, path)

    def _get_selected_extensions(self) -> list[str]:
        return queue_manager.get_selected_extensions(self)

    def _create_queue_item(self, path: str, is_folder: bool, operation_type: OperationType) -> QueueItem:
        return queue_manager.create_queue_item(self, path, is_folder, operation_type)

    def _categorize_queue_items(
        self, items: list[tuple[str, bool]], operation_type: OperationType
    ) -> tuple[list[tuple[str, bool]], list[str], list[tuple[str, bool, QueueItem]], list[tuple[str, str]]]:
        return queue_manager.categorize_queue_items(self, items, operation_type)

    def _calculate_queue_estimates(self, items: list[tuple[str, bool]]) -> tuple[float | None, float | None]:
        return queue_manager.calculate_queue_estimates(self, items)

    def add_items_to_queue(
        self, items: list[tuple[str, bool]], operation_type: OperationType, force_preview: bool = False
    ) -> dict[str, int]:
        return queue_manager.add_items_to_queue(self, items, operation_type, force_preview)

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
            self._queue_items_by_id.pop(item.id, None)

        self.save_queue_to_config()
        self.refresh_queue_tree()
        self.sync_queue_tags_to_analysis_tree()

    def on_clear_queue(self):
        """Clear all items from queue."""
        if self.session.running:
            messagebox.showwarning("Queue Running", "Cannot clear queue while conversion is running.")
            return
        if not self._queue_items:
            return
        if messagebox.askyesno("Clear Queue", "Remove all items from the queue?"):
            self._queue_items.clear()
            self._queue_items_by_id.clear()
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
        self._queue_file_tree_map.clear()
        self._tree_file_map.clear()
        self._queue_items_by_id = {item.id: item for item in self._queue_items}

        # Check if stop has been requested (pending items won't run)
        stopping = self.session.running and self.stop_event is not None and self.stop_event.is_set()

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

            # Format size - for folders, sum file sizes
            if queue_item.is_folder and queue_item.files:
                total_size = sum(f.size_bytes for f in queue_item.files)
                size_display = format_file_size(total_size) if total_size > 0 else "—"
            elif record and record.file_size_bytes:
                size_display = format_file_size(record.file_size_bytes)
            elif os.path.isfile(queue_item.source_path):
                try:
                    size_display = format_file_size(os.path.getsize(queue_item.source_path))
                except OSError:
                    size_display = "—"
            else:
                size_display = "—"

            # Format estimated time
            if queue_item.is_folder and queue_item.files:
                # Sum estimates for all files in folder
                total_seconds = 0.0
                for file_item in queue_item.files:
                    file_estimate = estimate_file_time(file_item.path)
                    if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                        total_seconds += file_estimate.best_seconds
                est_time_display = format_compact_time(total_seconds) if total_seconds > 0 else "—"
            elif record:
                # Use record data for single file estimate
                file_estimate = estimate_file_time(
                    codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
                )
                if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                    est_time_display = format_compact_time(file_estimate.best_seconds)
                else:
                    est_time_display = "—"
            else:
                # Try path-based estimate as fallback
                file_estimate = estimate_file_time(queue_item.source_path)
                if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                    est_time_display = format_compact_time(file_estimate.best_seconds)
                else:
                    est_time_display = "—"

            # Format status (now includes progress info)
            # Determine status tag based on queue item status
            status_tag_map = {
                QueueItemStatus.PENDING: "file_pending",
                QueueItemStatus.CONVERTING: "file_converting",
                QueueItemStatus.COMPLETED: "file_done",
                QueueItemStatus.STOPPED: "file_skipped",
                QueueItemStatus.ERROR: "file_error",
            }
            # Check if this pending item won't run due to stop request
            if stopping and queue_item.status == QueueItemStatus.PENDING:
                status_display = "Will skip"
                item_tag = "file_skipped"
            else:
                status_display = queue_item.format_status_display()
                if queue_item.status == QueueItemStatus.CONVERTING and queue_item.total_files > 0:
                    status_display = f"Converting ({queue_item.processed_files}/{queue_item.total_files})"
                item_tag = status_tag_map.get(queue_item.status)

            # Insert item with order number prefix
            icon = "📁" if queue_item.is_folder else "🎬"
            prefix = "▶ " if queue_item.is_folder else ""
            name = os.path.basename(queue_item.source_path)

            item_id = self.queue_tree.insert(
                "",
                "end",
                text=f"{order}. {prefix}{icon} {name}",
                values=(size_display, est_time_display, operation_display, output_display, status_display),
                tags=(item_tag,) if item_tag else (),
            )
            self._queue_tree_map[queue_item.id] = item_id
            self._tree_queue_map[item_id] = queue_item.id

            # Insert nested file rows for folder items
            if queue_item.is_folder and queue_item.files:
                for file_item in queue_item.files:
                    file_name = os.path.basename(file_item.path)
                    file_size = format_file_size(file_item.size_bytes) if file_item.size_bytes > 0 else "—"

                    # Calculate estimated time for this file
                    file_time_estimate = estimate_file_time(file_item.path)
                    if file_time_estimate.confidence != "none" and file_time_estimate.best_seconds > 0:
                        file_est_time = format_compact_time(file_time_estimate.best_seconds)
                    else:
                        file_est_time = "—"

                    # Determine status tag based on file status
                    status_tag_map = {
                        QueueItemStatus.PENDING: "file_pending",
                        QueueItemStatus.CONVERTING: "file_converting",
                        QueueItemStatus.COMPLETED: "file_done",
                        QueueItemStatus.STOPPED: "file_skipped",
                        QueueItemStatus.ERROR: "file_error",
                    }
                    file_tag = status_tag_map.get(file_item.status, "file_pending")

                    # Format file status display
                    if file_item.status == QueueItemStatus.COMPLETED:
                        file_status = "Done"
                    elif file_item.status == QueueItemStatus.CONVERTING:
                        file_status = "Converting..."
                    elif file_item.status == QueueItemStatus.ERROR:
                        file_status = file_item.error_message or "Error"
                    elif file_item.status == QueueItemStatus.STOPPED:
                        file_status = "Stopped"
                    else:
                        file_status = ""  # Pending files show no status

                    # Override for pending files that won't run due to stop request
                    if stopping and file_item.status == QueueItemStatus.PENDING:
                        file_tag = "file_skipped"
                        file_status = "Will skip"

                    file_tree_id = self.queue_tree.insert(
                        item_id,
                        "end",
                        text=f"    🎬 {file_name}",
                        values=(file_size, file_est_time, "", "", file_status),
                        tags=(file_tag,),
                    )
                    self._queue_file_tree_map[file_item.path] = file_tree_id
                    self._tree_file_map[file_tree_id] = (queue_item.id, file_item.path)

        # Update total row
        total_items = len(self._queue_items)
        total_files = sum(len(item.files) if item.is_folder else 1 for item in self._queue_items)
        # Calculate total estimated time
        total_est_seconds = 0.0
        for queue_item in self._queue_items:
            if queue_item.is_folder and queue_item.files:
                for file_item in queue_item.files:
                    file_estimate = estimate_file_time(file_item.path)
                    if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                        total_est_seconds += file_estimate.best_seconds
            else:
                path_hash = compute_path_hash(queue_item.source_path)
                record = index.get(path_hash)
                if record:
                    file_estimate = estimate_file_time(
                        codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
                    )
                else:
                    file_estimate = estimate_file_time(queue_item.source_path)
                if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                    total_est_seconds += file_estimate.best_seconds
        total_est_time_display = format_compact_time(total_est_seconds) if total_est_seconds > 0 else "—"
        if total_files != total_items:
            status_text = f"{total_items} items ({total_files} files)"
            self.queue_total_tree.item("total", values=("", total_est_time_display, "", "", status_text))
        else:
            self.queue_total_tree.item("total", values=("", total_est_time_display, "", "", f"{total_items} items"))

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
            self._clear_analysis_tree()
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
            self._clear_analysis_tree()
            self._update_add_all_buttons_state()
            return

        # Clear tree and start background scan
        self._clear_analysis_tree()
        self._scanning = True
        self.analysis_total_tree.item("total", text="Scanning...", values=("—", "—", "—", "—"))
        self._update_add_all_buttons_state()  # Disable while scanning

        # Show scanning overlay (lift ensures it's above the tree)
        self.analysis_scan_overlay.place(relx=0, rely=0, relwidth=1, relheight=1)
        self.analysis_scan_overlay.lift()

        # Cancel any existing scan
        if self._scan_stop_event:
            self._scan_stop_event.set()

        # Start incremental background scan
        self._scan_stop_event = threading.Event()
        threading.Thread(
            target=self._incremental_scan_thread, args=(folder, extensions, self._scan_stop_event), daemon=True
        ).start()

    def _incremental_scan_thread(self, folder: str, extensions: list[str], stop_event: threading.Event):
        """Delegate to extracted analysis_scanner module."""
        analysis_scanner.incremental_scan_thread(self, folder, extensions, stop_event)

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

    def finish_incremental_scan(self, stopped: bool):
        """Clean up after incremental scan completes (runs on UI thread)."""
        self._scanning = False

        # Hide scanning overlay
        self.analysis_scan_overlay.place_forget()

        if not stopped:
            # Prune empty folders after scan completes
            self._prune_empty_folders()

        # Update total row with any cached data
        self.update_total_from_tree()

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
        """Delegate to extracted analysis_scanner module."""
        analysis_scanner.run_ffprobe_analysis(self, file_paths, output_folder, input_folder, anonymize)

    def batch_update_tree_rows(self, file_paths: list[str]) -> None:
        """Update multiple tree rows efficiently without per-file folder updates.

        Updates file rows and collects affected folders, then updates folder
        aggregates once at the end. Much faster than calling _update_tree_row
        for each file individually.

        Args:
            file_paths: List of file paths to update.
        """
        tree_state_manager.batch_update_tree_rows(self, file_paths)

    def on_ffprobe_complete(self):
        """Handle ffprobe analysis completion (called on main thread)."""
        self.analyze_button.config(state="normal")

        # Update total row
        self.update_total_from_tree()

        # Enable Add All buttons
        self._update_add_all_buttons_state()

        # Apply default sort by efficiency (highest first) if user hasn't sorted yet
        if self._sort_col is None:
            self.sort_analysis_tree("efficiency", descending=True)

    def _update_add_all_buttons_state(self):
        """Enable/disable the Add All buttons based on whether there are files in the tree."""
        has_files = bool(self._tree_item_map)
        state = "normal" if has_files else "disabled"
        self.add_all_analyze_button.config(state=state)
        self.add_all_convert_button.config(state=state)

    def _add_all_to_queue(self, operation_type: OperationType):
        """Add all discovered files to the queue with the specified operation type."""
        file_paths = list(self._tree_item_map.keys())
        if not file_paths:
            messagebox.showinfo("No Files", "No files to add. Run a scan first.")
            return

        # Build items list (all files, not folders)
        items = [(path, False) for path in file_paths]

        # Use preview dialog for bulk operations
        result = self.add_items_to_queue(items, operation_type, force_preview=True)

        total_added = result["added"] + result["conflict_added"] + result["conflict_replaced"]
        if total_added > 0:
            self.tab_control.select(self.convert_tab)

    def on_add_all_analyze(self):
        """Add all discovered files to the queue for CRF analysis."""
        self._add_all_to_queue(OperationType.ANALYZE)

    def on_add_all_convert(self):
        """Add all discovered files to the queue for conversion."""
        self._add_all_to_queue(OperationType.CONVERT)

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
            return None  # No tooltip for generic estimates

        return "Not yet analyzed. Click Analyze to scan."

    def _update_tree_row(self, file_path: str):
        """Update a single file row with data from history index.

        Args:
            file_path: Path to the file that was analyzed.
        """
        tree_state_manager.update_tree_row(self, file_path)

    def update_folder_aggregates(self, folder_id: str, item_to_path: dict[str, str] | None = None):
        return tree_state_manager.update_folder_aggregates(self, folder_id, item_to_path)

    def _get_queued_file_paths(self) -> set[str]:
        return tree_state_manager.get_queued_file_paths(self)

    def sync_queue_tags_to_analysis_tree(self):
        return tree_state_manager.sync_queue_tags_to_analysis_tree(self)

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
            current_tags = [t for t in current_tags if t not in ("in_queue", "partial_queue", "done", "skip", "av1")]

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

            # Update all ancestor folder aggregates
            parent_id = self.analysis_tree.parent(item_id)
            while parent_id:
                self.update_folder_aggregates(parent_id)
                parent_id = self.analysis_tree.parent(parent_id)

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
        time_str = format_compact_time(total_time) if total_time > 0 else "—"
        if any_estimate and time_str != "—":
            time_str = f"~{time_str}"
        eff_str = format_efficiency(total_savings, total_time)

        # Update the total row
        self.analysis_total_tree.item("total", text=name_text, values=(size_str, savings_str, time_str, eff_str))

    def update_total_from_tree(self) -> int:
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

    def sort_analysis_tree(self, col: str, descending: bool | None = None):
        """Sort the analysis tree by the specified column.

        Sorting is done within each parent (preserves hierarchy).
        Folders sort before files when sorting by Name.
        Toggle direction on repeated clicks (when descending is None).

        Args:
            col: Column to sort by ("#0", "size", "savings", "time", or "efficiency")
            descending: If specified, force this direction. If None, toggle on repeat click.
        """
        sort_analysis_tree(self, col, descending)

    def _update_sort_indicators(self):
        """Update column headers to show sort direction indicator."""
        update_sort_indicators(self)

    def _clear_sort_state(self):
        """Clear sort state and reset column headers."""
        clear_sort_state(self)

    def _clear_analysis_tree(self):
        """Clear analysis tree, item map, and sort state."""
        for item in self.analysis_tree.get_children():
            self.analysis_tree.delete(item)
        self._tree_item_map.clear()
        self._clear_sort_state()
