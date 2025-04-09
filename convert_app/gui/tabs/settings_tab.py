"""
Settings tab module for the AV1 Video Converter application.
"""
import tkinter as tk
from tkinter import ttk
import os # Added for path joining
import logging # Added for logging
import sys # For platform check

from convert_app.gui.base import ToolTip
# Import operations functions needed for this tab
from convert_app.gui.operations import browse_log_folder, open_log_folder_action, open_history_file_action

logger = logging.getLogger(__name__)

def create_settings_tab(gui):
    """Create the settings tab"""
    settings_frame = ttk.Frame(gui.settings_tab)
    settings_frame.pack(fill="both", expand=True, padx=10, pady=10)

    # --- General Settings ---
    general_frame = ttk.LabelFrame(settings_frame, text="General")
    general_frame.grid(row=0, column=0, sticky="ew", padx=5, pady=(10, 10))
    general_frame.columnconfigure(0, weight=1) # Allow expansion

    # Overwrite option
    overwrite_check = ttk.Checkbutton(general_frame, text="Overwrite output file if it already exists", variable=gui.overwrite)
    overwrite_check.grid(row=0, column=0, sticky="w", pady=(5, 10), padx=10)
    ToolTip(overwrite_check, "If checked, existing files in the output folder with the same name will be replaced.\nIf unchecked, files that already exist in the output folder will be skipped.")

    # --- File Processing Settings ---
    processing_frame = ttk.LabelFrame(settings_frame, text="File Processing")
    processing_frame.grid(row=1, column=0, sticky="ew", padx=5, pady=(0, 10))

    # File extensions label
    ttk.Label(processing_frame, text="File Extensions to Process:", style="Header.TLabel").grid(row=0, column=0, sticky="w", padx=10, pady=(10, 5))

    # File extensions checkboxes
    ext_frame = ttk.Frame(processing_frame)
    ext_frame.grid(row=1, column=0, sticky="w", padx=10, pady=(0, 10))

    mp4_check = ttk.Checkbutton(ext_frame, text="MP4", variable=gui.ext_mp4, style="ExtButton.TCheckbutton")
    mp4_check.pack(side="left", padx=(0, 10))
    ToolTip(mp4_check, "Process .mp4 video files")

    mkv_check = ttk.Checkbutton(ext_frame, text="MKV", variable=gui.ext_mkv, style="ExtButton.TCheckbutton")
    mkv_check.pack(side="left", padx=(0, 10))
    ToolTip(mkv_check, "Process .mkv video files")

    avi_check = ttk.Checkbutton(ext_frame, text="AVI", variable=gui.ext_avi, style="ExtButton.TCheckbutton")
    avi_check.pack(side="left", padx=(0, 10))
    ToolTip(avi_check, "Process .avi video files")

    wmv_check = ttk.Checkbutton(ext_frame, text="WMV", variable=gui.ext_wmv, style="ExtButton.TCheckbutton")
    wmv_check.pack(side="left", padx=(0, 10))
    ToolTip(wmv_check, "Process .wmv video files")

    # Audio conversion settings label
    ttk.Label(processing_frame, text="Audio Settings:", style="Header.TLabel").grid(row=2, column=0, sticky="w", padx=10, pady=(10, 5))

    # Audio conversion frame
    audio_frame = ttk.Frame(processing_frame)
    audio_frame.grid(row=3, column=0, sticky="w", padx=10, pady=(0, 15))

    audio_check = ttk.Checkbutton(audio_frame, text="Convert non-AAC/Opus audio to", variable=gui.convert_audio) # Clarified text
    audio_check.pack(side="left", padx=(0, 5))
    ToolTip(audio_check, "When enabled, audio tracks not already AAC or Opus will be re-encoded to the selected codec.\nIf disabled, original audio tracks are copied without re-encoding.")

    audio_combo = ttk.Combobox(audio_frame, textvariable=gui.audio_codec, width=10, state="readonly") # Readonly state
    audio_combo['values'] = ('opus', 'aac')
    audio_combo.pack(side="left")
    ToolTip(audio_combo, "Select audio codec for re-encoding. Opus offers better compression. AAC is widely compatible.")


    # --- Logging & History Settings ---
    log_hist_frame = ttk.LabelFrame(settings_frame, text="Logging & History")
    log_hist_frame.grid(row=2, column=0, sticky="ew", padx=5, pady=(0, 10))
    log_hist_frame.columnconfigure(1, weight=1) # Make entry expand

    # Log folder setting
    ttk.Label(log_hist_frame, text="Log Folder:").grid(row=0, column=0, sticky="w", padx=(10, 5), pady=(10, 5))
    log_entry = ttk.Entry(log_hist_frame, textvariable=gui.log_folder)
    log_entry.grid(row=0, column=1, sticky="ew", padx=5, pady=(10, 5))
    log_browse_btn = ttk.Button(log_hist_frame, text="Browse...", command=gui.on_browse_log_folder)
    log_browse_btn.grid(row=0, column=2, sticky="e", padx=(0, 10), pady=(10, 5))

    # Log actions frame
    log_actions_frame = ttk.Frame(log_hist_frame)
    log_actions_frame.grid(row=1, column=0, columnspan=3, sticky="w", padx=10, pady=(0, 5))

    log_open_btn = ttk.Button(log_actions_frame, text="Open Log Folder", command=gui.on_open_log_folder)
    log_open_btn.pack(side="left", padx=(0, 10))
    ToolTip(log_open_btn, "Open the folder containing the application log files.")

    anonymize_log_check = ttk.Checkbutton(log_actions_frame, text="Anonymize Filenames in Logs", variable=gui.anonymize_logs)
    anonymize_log_check.pack(side="left")
    ToolTip(anonymize_log_check, "If checked, replaces specific filenames in logs with generic placeholders (e.g., video_SIZE.mkv) for privacy.")

    # History actions frame
    history_actions_frame = ttk.Frame(log_hist_frame)
    history_actions_frame.grid(row=2, column=0, columnspan=3, sticky="w", padx=10, pady=(5, 10))

    history_open_btn = ttk.Button(history_actions_frame, text="Open History File", command=gui.on_open_history_file)
    history_open_btn.pack(side="left", padx=(0, 10))
    ToolTip(history_open_btn, "Open the conversion_history.json file (if it exists).")

    anonymize_history_check = ttk.Checkbutton(history_actions_frame, text="Anonymize Filenames in History", variable=gui.anonymize_history)
    anonymize_history_check.pack(side="left")
    ToolTip(anonymize_history_check, "If checked, replaces specific filenames in the conversion_history.json file with generic placeholders for privacy.")


    # Make settings frame column expandable
    settings_frame.columnconfigure(0, weight=1)
    # Make rows expandable if needed in future
    # settings_frame.rowconfigure(3, weight=1) # Example