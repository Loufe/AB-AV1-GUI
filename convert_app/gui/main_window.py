"""
Main window module for the AV1 Video Converter application.
"""
# Standard library imports
import tkinter as tk
from tkinter import ttk, messagebox
import multiprocessing
import threading
import logging
import os
import sys
import time
import json # For settings persistence

# Project imports
from convert_app.gui.tabs.main_tab import create_main_tab
from convert_app.gui.tabs.settings_tab import create_settings_tab
from convert_app.gui.operations import (
    browse_input_folder, browse_output_folder,
    check_ffmpeg, start_conversion, stop_conversion,
    force_stop_conversion
)
# Import setup_logging only needed here now
from convert_app.utils import setup_logging, get_script_directory

logger = logging.getLogger(__name__)

# Place config file next to script/executable
CONFIG_FILE = os.path.join(get_script_directory(), "av1_converter_config.json")

class VideoConverterGUI:
    """Main application window for the AV1 Video Converter application."""

    def __init__(self, root):
        """Initialize the main window and all components."""
        self.root = root
        self.root.title("AV1 Video Converter")
        self.root.geometry("800x650")
        self.root.minsize(700, 550)

        # Load settings first
        self.config = self.load_settings()

        # Initialize tk variables based on loaded config (needed *before* logging setup uses them)
        self.initialize_variables()

        # Setup logging using initialized variables (which hold config values or defaults)
        try:
            log_dir_pref = self.log_folder.get() # Get value from tk.StringVar
            anonymize_pref = self.anonymize_logs.get() # Get value from tk.BooleanVar
            # Store the *actual* directory used by logging setup
            self.log_directory = setup_logging(log_directory=log_dir_pref, anonymize=anonymize_pref)

            # --- *** ADDED THIS BLOCK *** ---
            # Update the log_folder StringVar to reflect the actual directory used
            if self.log_directory:
                self.log_folder.set(self.log_directory)
                logger.info(f"Using log directory: {self.log_directory}")
            else:
                # If setup_logging failed to return a valid dir, clear the field maybe?
                self.log_folder.set("") # Clear the entry field if logging dir failed
                logger.warning("Log directory could not be determined or created. File logging disabled.")
            # --- *** END ADDED BLOCK *** ---

            logger.info("=== Starting AV1 Video Converter ===") # Now log start message

        except Exception as e:
            # Handle errors during logging setup itself
            messagebox.showerror("Logging Error", f"Failed to initialize logging:\n{e}\n\nApplication cannot start.")
            # Attempt to clean up tk window if it exists
            try: root.destroy()
            except: pass
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
        check_ffmpeg(self) # Check dependencies after UI is built

    def load_settings(self):
        """Load settings from JSON config file"""
        try:
            if os.path.exists(CONFIG_FILE):
                with open(CONFIG_FILE, 'r', encoding='utf-8') as f:
                    config = json.load(f); print(f"Loaded settings from {CONFIG_FILE}"); return config
            else: print(f"Config file {CONFIG_FILE} not found, using defaults."); return {}
        except Exception as e: print(f"Error loading settings from {CONFIG_FILE}: {e}. Using defaults."); return {}

    def save_settings(self):
        """Save settings to JSON config file"""
        try:
            current_config = {
                'input_folder': self.input_folder.get(), 'output_folder': self.output_folder.get(),
                'overwrite': self.overwrite.get(), 'ext_mp4': self.ext_mp4.get(),
                'ext_mkv': self.ext_mkv.get(), 'ext_avi': self.ext_avi.get(), 'ext_wmv': self.ext_wmv.get(),
                'convert_audio': self.convert_audio.get(), 'audio_codec': self.audio_codec.get(),
                'log_folder': self.log_folder.get(), 'anonymize_logs': self.anonymize_logs.get(),
                'anonymize_history': self.anonymize_history.get()
            }
            temp_config_file = CONFIG_FILE + ".tmp"
            with open(temp_config_file, 'w', encoding='utf-8') as f: json.dump(current_config, f, indent=4)
            os.replace(temp_config_file, CONFIG_FILE)
            logger.info(f"Saved settings to {CONFIG_FILE}")
        except Exception as e: logger.error(f"Error saving settings to {CONFIG_FILE}: {e}")

    def setup_styles(self):
        """Set up the GUI styles"""
        self.style = ttk.Style();
        try: logger.debug(f"Using theme: {self.style.theme_use()}")
        except tk.TclError: logging.warning("Could not detect theme, using default.")
        self.style.configure("TFrame", background="#f0f0f0"); self.style.configure("TButton", font=("Arial", 10))
        self.style.configure("TLabel", font=("Arial", 10), background="#f0f0f0"); self.style.configure("Header.TLabel", font=("Arial", 10, "bold"), background="#f0f0f0")
        self.style.configure("ExtButton.TCheckbutton", font=("Arial", 9)); self.style.configure("TLabelframe", background="#f0f0f0", padding=5)
        self.style.configure("TLabelframe.Label", font=("Arial", 10, "bold"), background="#f0f0f0")

    def initialize_variables(self):
        """Initialize the GUI variables, using loaded config"""
        # Initialize StringVars/BooleanVars *first* based on config or defaults
        self.input_folder = tk.StringVar(value=self.config.get('input_folder', ''))
        self.output_folder = tk.StringVar(value=self.config.get('output_folder', ''))
        # Initialize log_folder based on config, but it will be updated after setup_logging confirms the actual path
        self.log_folder = tk.StringVar(value=self.config.get('log_folder', ''))
        self.overwrite = tk.BooleanVar(value=self.config.get('overwrite', False))
        self.ext_mp4 = tk.BooleanVar(value=self.config.get('ext_mp4', True))
        self.ext_mkv = tk.BooleanVar(value=self.config.get('ext_mkv', True))
        self.ext_avi = tk.BooleanVar(value=self.config.get('ext_avi', True))
        self.ext_wmv = tk.BooleanVar(value=self.config.get('ext_wmv', True))
        self.convert_audio = tk.BooleanVar(value=self.config.get('convert_audio', True))
        self.anonymize_logs = tk.BooleanVar(value=self.config.get('anonymize_logs', True))
        self.anonymize_history = tk.BooleanVar(value=self.config.get('anonymize_history', True))
        self.audio_codec = tk.StringVar(value=self.config.get('audio_codec', "opus"))
        # Non-saved variables
        self.vmaf_scores = []; self.crf_values = []; self.size_reductions = []
        try: self.cpu_count = max(1, multiprocessing.cpu_count())
        except NotImplementedError: self.cpu_count = 1; logger.warning("Could not detect CPU count.")

    def initialize_conversion_state(self):
        """Initialize conversion state variables"""
        self.conversion_running = False; self.conversion_thread = None; self.stop_event = None
        self.video_files = []; self.processed_files = 0; self.successful_conversions = 0
        self.elapsed_timer_id = None; self.output_folder_path = ""; self.current_process_info = None
        self.error_count = 0; self.total_input_bytes_success = 0; self.total_output_bytes_success = 0; self.total_time_success = 0

    def initialize_button_states(self):
        """Initialize button states"""
        if hasattr(self, 'start_button'): self.start_button.config(state="normal")
        if hasattr(self, 'stop_button'): self.stop_button.config(state="disabled")
        if hasattr(self, 'force_stop_button'): self.force_stop_button.config(state="disabled")

    def on_exit(self):
        """Handle application exit: confirm, save settings, cleanup"""
        confirm_exit = True
        if hasattr(self, 'conversion_running') and self.conversion_running:
            confirm_exit = messagebox.askyesno("Confirm Exit", "Conversion running. Exit will stop it.\nAre you sure?")
        if confirm_exit:
            logger.info("=== AV1 Video Converter Exiting ==="); self.save_settings()
            if hasattr(self, 'conversion_running') and self.conversion_running:
                logger.info("Signalling conversion thread to stop..."); self.force_stop_conversion(confirm=False)
            self._cleanup_threads(); self.root.after(100, self._complete_exit)
        else: logger.info("User cancelled application exit.")

    def _cleanup_threads(self):
        """Ensure all threads are properly cleaned up before exit"""
        if hasattr(self, 'elapsed_timer_id') and self.elapsed_timer_id:
            try: self.root.after_cancel(self.elapsed_timer_id); self.elapsed_timer_id = None; logger.debug("Cancelled timer")
            except Exception as e: logger.error(f"Error cancelling timer: {str(e)}")
        if hasattr(self, 'stop_event') and self.stop_event: self.stop_event.set(); logger.debug("Stop event set.")
        if hasattr(self, 'conversion_thread') and self.conversion_thread and self.conversion_thread.is_alive():
            try:
                logger.info("Waiting briefly for thread..."); self.conversion_thread.join(timeout=1.0)
                if self.conversion_thread.is_alive(): logger.warning("Thread did not terminate quickly")
                else: logger.debug("Thread terminated")
            except Exception as e: logger.error(f"Error joining thread: {str(e)}")
        self.conversion_thread = None; self.conversion_running = False; self.stop_event = None

    def _complete_exit(self):
        """Complete the exit process: shutdown logging, destroy window, force process exit"""
        logger.info("Destroying main window and exiting process.")
        try: logging.shutdown()
        except Exception as log_e: print(f"Error shutting down logging: {log_e}")
        try: self.root.destroy()
        except tk.TclError: pass
        except Exception as e: print(f"Error destroying root window: {e}")
        print("Forcing process exit."); os._exit(0)

    # Method references
    def browse_input_folder(self): browse_input_folder(self)
    def browse_output_folder(self): browse_output_folder(self)
    def start_conversion(self): start_conversion(self)
    def stop_conversion(self): stop_conversion(self)
    def force_stop_conversion(self, confirm=True): force_stop_conversion(self, confirm=confirm)