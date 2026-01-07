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

from src.gui import (
    analysis_controller,
    analysis_scanner,
    analysis_tree,
    dependency_manager,
    queue_controller,
    queue_manager,
    queue_tree,
)
from src.gui.constants import COLOR_BACKGROUND, COLOR_TEXT_MUTED, FONT_BODY, FONT_BODY_BOLD, FONT_SMALL, FONT_TAB
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
from src.gui.tabs.history_tab import create_history_tab
from src.gui.tabs.settings_tab import create_settings_tab
from src.gui.tabs.statistics_tab import create_statistics_tab
from src.gui.tree_formatters import clear_sort_state, sort_analysis_tree, update_sort_indicators

# Import from extracted modules
from src.logging_setup import get_script_directory, setup_logging
from src.models import ConversionSessionState, OperationType, QueueItem
from src.utils import scrub_history_paths, scrub_log_files

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
    clear_completed_button: ttk.Button
    start_button: ttk.Button
    stop_button: ttk.Button
    force_stop_button: ttk.Button
    status_label: ttk.Label
    total_elapsed_label: ttk.Label
    total_remaining_label: ttk.Label
    queue_tree: ttk.Treeview
    queue_total_tree: ttk.Treeview
    current_file_label: ttk.Label
    quality_progress: ttk.Progressbar
    quality_percent_label: ttk.Label
    encoding_progress: ttk.Progressbar
    encoding_percent_label: ttk.Label
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
    analysis_scan_badge: tk.Label

    # Settings tab widgets (created in create_settings_tab)
    app_frame: ttk.Frame
    app_version_label: ttk.Label
    app_check_btn: ttk.Button | None
    app_update_label: ttk.Label
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

    # History tab widgets (created in create_history_tab)
    history_tree: ttk.Treeview
    history_status_label: ttk.Label

    def __init__(self, root):
        """Initialize the main window and all components."""
        self.root = root
        self.root.title("AB-AV1 GUI")
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

            logger.info("=== Starting AB-AV1 GUI ===")  # Now log start message

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
        self.history_tab = ttk.Frame(self.tab_control)
        self.settings_tab = ttk.Frame(self.tab_control)
        self.tab_control.add(self.convert_tab, text="Queue")
        self.tab_control.add(self.analysis_tab, text="Analysis")
        self.tab_control.add(self.statistics_tab, text="Statistics")
        self.tab_control.add(self.history_tab, text="History")
        self.tab_control.add(self.settings_tab, text="Settings")
        self.tab_control.pack(expand=1, fill="both")

        create_convert_tab(self)
        create_analysis_tab(self)
        create_statistics_tab(self)
        create_history_tab(self)
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
        self.style.configure("TFrame", background=COLOR_BACKGROUND)
        self.style.configure("TLabel", font=FONT_BODY, background=COLOR_BACKGROUND)
        self.style.configure("Header.TLabel", font=FONT_BODY_BOLD, background=COLOR_BACKGROUND)
        self.style.configure("ExtButton.TCheckbutton", font=FONT_SMALL)
        self.style.configure("TLabelframe", background=COLOR_BACKGROUND, padding=5)
        self.style.configure("TLabelframe.Label", font=FONT_BODY_BOLD, background=COLOR_BACKGROUND)
        self.style.configure("TNotebook.Tab", font=FONT_TAB, padding=(10, 4))
        self.style.configure("Treeview.Heading", font=FONT_BODY_BOLD)

        # Add custom style for range text - dark gray color
        self.style.configure("Range.TLabel", font=FONT_BODY, background=COLOR_BACKGROUND, foreground=COLOR_TEXT_MUTED)

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

        # History tab filter state
        self.history_show_converted = tk.BooleanVar(value=True)
        self.history_show_analyzed = tk.BooleanVar(value=True)
        self.history_show_skipped = tk.BooleanVar(value=True)
        self._history_tree_map: dict[str, str] = {}  # path_hash -> tree_item_id
        self._history_sort_col: str | None = None
        self._history_sort_reverse: bool = True  # Default: date descending
        self._history_filter_timer: str | None = None  # Debounce timer ID

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
            logger.info("=== AB-AV1 GUI Exiting ===")
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

    def on_check_app_updates(self):
        """Check GitHub for the latest AB-AV1-GUI version and update the label."""
        dependency_manager.check_app_updates(self)

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
        queue_controller.on_add_folder_to_queue(self)

    def on_add_files_to_queue(self):
        """Add individual files to the conversion queue."""
        queue_controller.on_add_files_to_queue(self)

    def add_items_to_queue(
        self, items: list[tuple[str, bool]], operation_type: OperationType, force_preview: bool = False
    ) -> dict[str, int]:
        return queue_manager.add_items_to_queue(self, items, operation_type, force_preview)

    def add_to_queue(self, path: str, is_folder: bool, operation_type: OperationType = OperationType.CONVERT) -> str:
        """Add a single item to the queue (convenience wrapper)."""
        return queue_controller.add_to_queue(self, path, is_folder, operation_type)

    def on_remove_from_queue(self):
        """Remove selected items from queue."""
        queue_controller.on_remove_from_queue(self)

    def on_clear_queue(self):
        """Clear all items from queue."""
        queue_controller.on_clear_queue(self)

    def on_clear_completed(self):
        """Clear completed/errored/stopped items from queue."""
        queue_controller.on_clear_completed(self)

    def on_item_suffix_changed(self):
        """Handle suffix change for selected item."""
        queue_controller.on_item_suffix_changed(self)

    def on_browse_item_output_folder(self):
        """Browse for item-specific output folder."""
        queue_controller.on_browse_item_output_folder(self)

    def refresh_queue_tree(self):
        """Refresh the queue tree view from _queue_items."""
        queue_tree.refresh_queue_tree(self)

    def sync_queue_order_from_tree(self):
        """Sync _queue_items order from the tree view after drag-drop reordering."""
        queue_tree.sync_queue_order_from_tree(self)

    # --- Analysis Tab Handlers ---

    def _on_folder_or_extension_changed(self, *args):
        """Auto-refresh analysis tree when folder or extensions change."""
        analysis_controller.on_folder_or_extension_changed(self, *args)

    def _refresh_analysis_tree(self):
        """Start background scan to populate tree incrementally."""
        analysis_controller.refresh_analysis_tree(self)

    def _incremental_scan_thread(self, folder: str, extensions: list[str], stop_event: threading.Event):
        """Delegate to extracted analysis_scanner module."""
        analysis_scanner.incremental_scan_thread(self, folder, extensions, stop_event)

    def _prune_empty_folders(self) -> int:
        """Remove folders with no children from the tree (runs on UI thread)."""
        return analysis_controller.prune_empty_folders(self)

    def finish_incremental_scan(self, stopped: bool):
        """Clean up after incremental scan completes (runs on UI thread)."""
        analysis_controller.finish_incremental_scan(self, stopped)

    def on_analyze_folders(self):
        """Run ffprobe analysis on files already in the tree."""
        analysis_controller.on_analyze_folders(self)

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
        analysis_tree.batch_update_tree_rows(self, file_paths)

    def on_ffprobe_complete(self):
        """Handle ffprobe analysis completion (called on main thread)."""
        analysis_controller.on_ffprobe_complete(self)

    def _update_add_all_buttons_state(self):
        """Enable/disable the Add All buttons based on whether there are files in the tree."""
        analysis_controller.update_add_all_buttons_state(self)

    def _add_all_to_queue(self, operation_type: OperationType):
        """Add all discovered files to the queue with the specified operation type."""
        analysis_controller.add_all_to_queue(self, operation_type)

    def on_add_all_analyze(self):
        """Add all discovered files to the queue for CRF analysis."""
        analysis_controller.on_add_all_analyze(self)

    def on_add_all_convert(self):
        """Add all discovered files to the queue for conversion."""
        analysis_controller.on_add_all_convert(self)

    def get_file_path_for_tree_item(self, item_id: str) -> str | None:
        """Look up the file path for a given tree item ID."""
        return analysis_controller.get_file_path_for_tree_item(self, item_id)

    def get_analysis_tree_tooltip(self, item_id: str) -> str | None:
        """Generate tooltip text for an analysis tree item."""
        return analysis_controller.get_analysis_tree_tooltip(self, item_id)

    def _update_tree_row(self, file_path: str):
        """Update a single file row with data from history index.

        Args:
            file_path: Path to the file that was analyzed.
        """
        analysis_tree.update_tree_row(self, file_path)

    def update_folder_aggregates(self, folder_id: str, item_to_path: dict[str, str] | None = None):
        return analysis_tree.update_folder_aggregates(self, folder_id, item_to_path)

    def _get_queued_file_paths(self) -> set[str]:
        return analysis_tree.get_queued_file_paths(self)

    def sync_queue_tags_to_analysis_tree(
        self,
        added_paths: set[str] | None = None,
        removed_paths: set[str] | None = None,
    ):
        return analysis_tree.sync_queue_tags_to_analysis_tree(self, added_paths, removed_paths)

    def update_analysis_tree_for_completed_file(self, file_path: str, status: str):
        """Update analysis tree entry when a file completes conversion."""
        analysis_controller.update_analysis_tree_for_completed_file(self, file_path, status)

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
        """Update the fixed total row at the bottom of the analysis tree."""
        analysis_controller.update_total_row(
            self, total_files, convertible, done_count, skip_count, total_size, total_savings, total_time, any_estimate
        )

    def update_total_from_tree(self) -> int:
        """Compute and update totals from files in the tree using history index."""
        return analysis_controller.update_total_from_tree(self)

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
        analysis_controller.clear_analysis_tree(self)
