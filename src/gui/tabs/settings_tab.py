# src/gui/tabs/settings_tab.py
"""
Settings tab module for the AV1 Video Converter application.
"""

import logging
from tkinter import ttk

from src.ab_av1.checker import get_ab_av1_version
from src.gui.base import ToolTip
from src.utils import check_ffmpeg_availability, parse_ffmpeg_version

logger = logging.getLogger(__name__)


def create_settings_tab(gui):
    """Create the settings tab"""
    settings_frame = ttk.Frame(gui.settings_tab)
    settings_frame.pack(fill="both", expand=True, padx=10, pady=10)
    settings_frame.columnconfigure(0, weight=1)  # Allow LabelFrames to expand horizontally

    # --- General Settings ---
    general_frame = ttk.LabelFrame(settings_frame, text="General")
    general_frame.grid(row=0, column=0, sticky="ew", padx=5, pady=(10, 10))
    general_frame.columnconfigure(0, weight=1)  # Allow expansion

    # Overwrite option
    overwrite_check = ttk.Checkbutton(
        general_frame, text="Overwrite output file if it already exists", variable=gui.overwrite
    )
    overwrite_check.grid(row=0, column=0, sticky="w", pady=(5, 10), padx=10)
    ToolTip(
        overwrite_check,
        "If checked, existing files in the output folder with the same name will be replaced.\n"
        "If unchecked, files that already exist in the output folder will be skipped.",
    )

    # --- File Processing Settings ---
    processing_frame = ttk.LabelFrame(settings_frame, text="File Processing")
    processing_frame.grid(row=1, column=0, sticky="ew", padx=5, pady=(0, 10))

    # File extensions label
    ttk.Label(processing_frame, text="File Extensions to Process:", style="Header.TLabel").grid(
        row=0, column=0, sticky="w", padx=10, pady=(10, 5)
    )

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
    ttk.Label(processing_frame, text="Audio Settings:", style="Header.TLabel").grid(
        row=2, column=0, sticky="w", padx=10, pady=(10, 5)
    )

    # Audio conversion frame
    audio_frame = ttk.Frame(processing_frame)
    audio_frame.grid(row=3, column=0, sticky="w", padx=10, pady=(0, 15))

    audio_check = ttk.Checkbutton(
        audio_frame, text="Convert non-AAC/Opus audio to", variable=gui.convert_audio
    )  # Clarified text
    audio_check.pack(side="left", padx=(0, 5))
    ToolTip(
        audio_check,
        "When enabled, audio tracks not already AAC or Opus will be re-encoded to the selected codec.\n"
        "If disabled, original audio tracks are copied without re-encoding.",
    )

    audio_combo = ttk.Combobox(audio_frame, textvariable=gui.audio_codec, width=10, state="readonly")  # Readonly state
    audio_combo["values"] = ("opus", "aac")
    audio_combo.pack(side="left")
    ToolTip(
        audio_combo, "Select audio codec for re-encoding. Opus offers better compression. AAC is widely compatible."
    )

    # --- Logging & History Settings ---
    log_hist_frame = ttk.LabelFrame(settings_frame, text="Logging & History")
    log_hist_frame.grid(row=2, column=0, sticky="ew", padx=5, pady=(0, 10))
    log_hist_frame.columnconfigure(1, weight=1)  # Make entry expand

    # Log folder setting
    ttk.Label(log_hist_frame, text="Log Folder:").grid(row=0, column=0, sticky="w", padx=(10, 5), pady=(10, 5))
    log_entry = ttk.Entry(log_hist_frame, textvariable=gui.log_folder)
    log_entry.grid(row=0, column=1, sticky="ew", padx=5, pady=(10, 5))
    # Use the method reference from the main GUI instance
    log_browse_btn = ttk.Button(log_hist_frame, text="Browse...", command=gui.on_browse_log_folder)
    log_browse_btn.grid(row=0, column=2, sticky="e", padx=(0, 10), pady=(10, 5))

    # Log actions frame
    log_actions_frame = ttk.Frame(log_hist_frame)
    log_actions_frame.grid(row=1, column=0, columnspan=3, sticky="w", padx=10, pady=(0, 5))

    # Use the method reference from the main GUI instance
    log_open_btn = ttk.Button(log_actions_frame, text="Open Log Folder", command=gui.on_open_log_folder)
    log_open_btn.pack(side="left", padx=(0, 10))
    ToolTip(log_open_btn, "Open the folder containing the application log files.")

    anonymize_log_check = ttk.Checkbutton(
        log_actions_frame, text="Anonymize Filenames in Logs", variable=gui.anonymize_logs
    )
    anonymize_log_check.pack(side="left", padx=(0, 10))
    ToolTip(
        anonymize_log_check, "If checked, replaces specific filenames in logs with generic placeholders for privacy."
    )

    scrub_logs_btn = ttk.Button(log_actions_frame, text="Scrub Logs", command=gui.on_scrub_logs)
    scrub_logs_btn.pack(side="left")
    ToolTip(scrub_logs_btn, "Permanently anonymize all existing file paths in log files. This cannot be undone.")

    # History actions frame
    history_actions_frame = ttk.Frame(log_hist_frame)
    history_actions_frame.grid(row=2, column=0, columnspan=3, sticky="w", padx=10, pady=(5, 10))

    # Use the method reference from the main GUI instance
    history_open_btn = ttk.Button(history_actions_frame, text="Open History File", command=gui.on_open_history_file)
    history_open_btn.pack(side="left", padx=(0, 10))
    ToolTip(history_open_btn, "Open the conversion_history_v2.json file (if it exists).")

    anonymize_history_check = ttk.Checkbutton(
        history_actions_frame, text="Anonymize Filenames in History", variable=gui.anonymize_history
    )
    anonymize_history_check.pack(side="left", padx=(0, 10))
    ToolTip(
        anonymize_history_check,
        "If checked, replaces specific filenames in the conversion_history_v2.json file "
        "with generic placeholders for privacy.",
    )

    scrub_history_btn = ttk.Button(history_actions_frame, text="Scrub History", command=gui.on_scrub_history)
    scrub_history_btn.pack(side="left")
    ToolTip(
        scrub_history_btn, "Permanently anonymize all existing file paths in the history file. This cannot be undone."
    )

    # --- Version Info ---
    version_frame = ttk.LabelFrame(settings_frame, text="Version Info")
    version_frame.grid(row=3, column=0, sticky="ew", padx=5, pady=(0, 10))

    # Get local ab-av1 version
    local_version = get_ab_av1_version() or "Not found"

    # ab-av1 version row
    ab_av1_frame = ttk.Frame(version_frame)
    ab_av1_frame.grid(row=0, column=0, sticky="w", padx=10, pady=(10, 10))

    ttk.Label(ab_av1_frame, text="ab-av1 Version:").pack(side="left", padx=(0, 5))
    ttk.Label(ab_av1_frame, text=local_version, font=("TkDefaultFont", 9, "bold")).pack(side="left", padx=(0, 15))

    check_update_btn = ttk.Button(ab_av1_frame, text="Check for Updates", command=gui.on_check_ab_av1_updates)
    check_update_btn.pack(side="left", padx=(0, 10))
    ToolTip(
        check_update_btn,
        "Check GitHub for the latest ab-av1 release.\n"
        "This will make a network request to api.github.com.\n"
        "If an update is available, click the link to open the release page.",
    )

    # Label to show update check result (initially empty)
    gui.ab_av1_update_label = ttk.Label(ab_av1_frame, text="")
    gui.ab_av1_update_label.pack(side="left")

    # FFmpeg version row
    ffmpeg_frame = ttk.Frame(version_frame)
    ffmpeg_frame.grid(row=1, column=0, sticky="w", padx=10, pady=(0, 10))

    # Get FFmpeg version info
    _, _, ffmpeg_version_string, _ = check_ffmpeg_availability()
    ffmpeg_version, ffmpeg_source, ffmpeg_build = parse_ffmpeg_version(ffmpeg_version_string)
    ffmpeg_display = ffmpeg_version or "Not found"
    if ffmpeg_source:
        source_info = ffmpeg_source
        if ffmpeg_build:
            source_info += f" {ffmpeg_build}"
        ffmpeg_display += f" ({source_info})"

    ttk.Label(ffmpeg_frame, text="FFmpeg Version:").pack(side="left", padx=(0, 5))
    ttk.Label(ffmpeg_frame, text=ffmpeg_display, font=("TkDefaultFont", 9, "bold")).pack(side="left", padx=(0, 15))

    # Show check button for supported sources (gyan.dev and BtbN)
    if ffmpeg_source in ("gyan.dev", "BtbN"):
        check_ffmpeg_btn = ttk.Button(ffmpeg_frame, text="Check for Updates", command=gui.on_check_ffmpeg_updates)
        check_ffmpeg_btn.pack(side="left", padx=(0, 10))
        tooltip_text = f"Check GitHub for the latest FFmpeg release from {ffmpeg_source}.\n"
        tooltip_text += "This will make a network request to api.github.com.\n"
        if ffmpeg_source == "gyan.dev":
            tooltip_text += "If an update is available, click the link to open the release page."
        else:
            tooltip_text += "Click the link to open the releases page."
        ToolTip(check_ffmpeg_btn, tooltip_text)

        gui.ffmpeg_update_label = ttk.Label(ffmpeg_frame, text="")
        gui.ffmpeg_update_label.pack(side="left")
        gui.ffmpeg_source = ffmpeg_source  # Store for use in handler
    else:
        gui.ffmpeg_update_label = None
        gui.ffmpeg_source = None
