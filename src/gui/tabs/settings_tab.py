# src/gui/tabs/settings_tab.py
"""
Settings tab module for the AV1 Video Converter application.
"""

import logging
from tkinter import ttk

from src.ab_av1.checker import get_ab_av1_version
from src.gui.base import ToolTip
from src.gui.constants import COLOR_STATUS_NEUTRAL, COLOR_STATUS_SUCCESS_LIGHT, FONT_SYSTEM_BOLD
from src.hardware_accel import get_available_hw_decoders
from src.utils import check_ffmpeg_availability, parse_ffmpeg_version
from src.vendor_manager import get_ab_av1_path, is_using_vendor_ffmpeg

logger = logging.getLogger(__name__)


def create_settings_tab(gui):
    """Create the settings tab"""
    settings_frame = ttk.Frame(gui.settings_tab)
    settings_frame.pack(fill="both", expand=True, padx=10, pady=5)
    settings_frame.columnconfigure(0, weight=1)  # Allow LabelFrames to expand horizontally

    # --- Output Settings (includes overwrite option) ---
    output_frame = ttk.LabelFrame(settings_frame, text="Output Settings")
    output_frame.grid(row=0, column=0, sticky="ew", padx=5, pady=(5, 5))
    output_frame.columnconfigure(1, weight=1)

    # Overwrite option (moved from General)
    overwrite_check = ttk.Checkbutton(
        output_frame, text="Overwrite output file if it already exists", variable=gui.overwrite
    )
    overwrite_check.grid(row=0, column=0, columnspan=3, sticky="w", pady=(5, 3), padx=10)
    ToolTip(
        overwrite_check,
        "If checked, existing files in the output folder with the same name will be replaced.\n"
        "If unchecked, files that already exist in the output folder will be skipped.",
    )

    # Default Output Mode dropdown
    ttk.Label(output_frame, text="Default Output Mode:").grid(row=1, column=0, sticky="w", padx=10, pady=3)
    mode_combo = ttk.Combobox(output_frame, textvariable=gui.default_output_mode, width=18, state="readonly")
    mode_combo["values"] = ("replace", "suffix", "separate_folder")
    mode_combo.grid(row=1, column=1, sticky="w", padx=5, pady=3)
    ToolTip(
        mode_combo,
        "Replace: Delete original after conversion\n"
        "Suffix: Keep original, add suffix to output\n"
        "Separate Folder: Output to different folder",
    )

    # Default Suffix entry
    ttk.Label(output_frame, text="Default Suffix:").grid(row=2, column=0, sticky="w", padx=10, pady=3)
    suffix_entry = ttk.Entry(output_frame, textvariable=gui.default_suffix, width=15)
    suffix_entry.grid(row=2, column=1, sticky="w", padx=5, pady=3)
    ToolTip(suffix_entry, "Suffix added before .mkv extension\nExample: '_av1' creates 'video_av1.mkv'")

    # Default Output Folder (for separate_folder mode)
    ttk.Label(output_frame, text="Default Output Folder:").grid(row=3, column=0, sticky="w", padx=10, pady=(3, 5))
    folder_entry = ttk.Entry(output_frame, textvariable=gui.default_output_folder)
    folder_entry.grid(row=3, column=1, sticky="ew", padx=5, pady=(3, 5))
    ttk.Button(output_frame, text="Browse...", command=gui.on_browse_default_output_folder).grid(
        row=3, column=2, padx=(0, 10), pady=(3, 5)
    )

    # --- Processing Options (File Processing + Hardware Acceleration) ---
    processing_frame = ttk.LabelFrame(settings_frame, text="Processing Options")
    processing_frame.grid(row=1, column=0, sticky="ew", padx=5, pady=(0, 5))

    # File extensions label
    ttk.Label(processing_frame, text="File Extensions to Process:", style="Header.TLabel").grid(
        row=0, column=0, sticky="w", padx=10, pady=(5, 3)
    )

    # File extensions checkboxes
    ext_frame = ttk.Frame(processing_frame)
    ext_frame.grid(row=1, column=0, sticky="w", padx=10, pady=(0, 5))

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
        row=2, column=0, sticky="w", padx=10, pady=(5, 3)
    )

    # Audio conversion frame
    audio_frame = ttk.Frame(processing_frame)
    audio_frame.grid(row=3, column=0, sticky="w", padx=10, pady=(0, 5))

    audio_check = ttk.Checkbutton(audio_frame, text="Convert non-AAC/Opus audio to", variable=gui.convert_audio)
    audio_check.pack(side="left", padx=(0, 5))
    ToolTip(
        audio_check,
        "When enabled, audio tracks not already AAC or Opus will be re-encoded to the selected codec.\n"
        "If disabled, original audio tracks are copied without re-encoding.",
    )

    audio_combo = ttk.Combobox(audio_frame, textvariable=gui.audio_codec, width=10, state="readonly")
    audio_combo["values"] = ("opus", "aac")
    audio_combo.pack(side="left")
    ToolTip(
        audio_combo, "Select audio codec for re-encoding. Opus offers better compression. AAC is widely compatible."
    )

    # Hardware decoding (moved from separate Hardware Acceleration frame)
    hw_decode_row = ttk.Frame(processing_frame)
    hw_decode_row.grid(row=4, column=0, sticky="w", padx=10, pady=(5, 5))

    hw_decode_check = ttk.Checkbutton(
        hw_decode_row, text="Use hardware-accelerated decoding", variable=gui.hw_decode_enabled
    )
    hw_decode_check.pack(side="left")
    ToolTip(
        hw_decode_check,
        "Enable NVIDIA CUVID or Intel QSV hardware decoding for faster source video decoding.\n"
        "Requires ab-av1 v0.10.0+ and compatible hardware/drivers.\n"
        "If unavailable, falls back to software decoding automatically.",
    )

    # Inline status indicator
    detected_decoders = get_available_hw_decoders()
    if detected_decoders:
        status_text = f"({len(detected_decoders)} available)"
        status_color = COLOR_STATUS_SUCCESS_LIGHT
    else:
        status_text = "(none detected)"
        status_color = COLOR_STATUS_NEUTRAL

    status_label = ttk.Label(hw_decode_row, text=status_text, foreground=status_color)
    status_label.pack(side="left", padx=(10, 0))

    # --- Logging & History Settings ---
    log_hist_frame = ttk.LabelFrame(settings_frame, text="Logging & History")
    log_hist_frame.grid(row=2, column=0, sticky="ew", padx=5, pady=(0, 5))
    log_hist_frame.columnconfigure(1, weight=1)  # Make entry expand

    # Log folder setting
    ttk.Label(log_hist_frame, text="Log Folder:").grid(row=0, column=0, sticky="w", padx=(10, 5), pady=(5, 3))
    log_entry = ttk.Entry(log_hist_frame, textvariable=gui.log_folder)
    log_entry.grid(row=0, column=1, sticky="ew", padx=5, pady=(5, 3))
    log_browse_btn = ttk.Button(log_hist_frame, text="Browse...", command=gui.on_browse_log_folder)
    log_browse_btn.grid(row=0, column=2, sticky="e", padx=(0, 10), pady=(5, 3))

    # Log actions frame
    log_actions_frame = ttk.Frame(log_hist_frame)
    log_actions_frame.grid(row=1, column=0, columnspan=3, sticky="w", padx=10, pady=(0, 3))

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
    history_actions_frame.grid(row=2, column=0, columnspan=3, sticky="w", padx=10, pady=(3, 5))

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
    version_frame.grid(row=3, column=0, sticky="ew", padx=5, pady=(0, 5))

    # Get local ab-av1 version
    local_version = get_ab_av1_version() or "Not found"
    has_ab_av1 = get_ab_av1_path() is not None

    # ab-av1 version row
    ab_av1_frame = ttk.Frame(version_frame)
    ab_av1_frame.grid(row=0, column=0, sticky="w", padx=10, pady=(5, 3))
    gui.ab_av1_frame = ab_av1_frame  # Store reference for dynamic button creation

    ttk.Label(ab_av1_frame, text="ab-av1 Version:").pack(side="left", padx=(0, 5))
    gui.ab_av1_version_label = ttk.Label(ab_av1_frame, text=local_version, font=FONT_SYSTEM_BOLD)
    gui.ab_av1_version_label.pack(side="left", padx=(0, 15))

    if not has_ab_av1:
        # Not installed - show Download button only
        gui.ab_av1_download_btn = ttk.Button(ab_av1_frame, text="Download", command=gui.on_download_ab_av1)
        gui.ab_av1_download_btn.pack(side="left", padx=(0, 5))
        ToolTip(
            gui.ab_av1_download_btn,
            "Download the latest ab-av1 from GitHub.\nThis will download ~3 MB and install to vendor/ab-av1/.",
        )
        gui.ab_av1_check_btn = None
    else:
        # Installed - show Check for Updates button only (Update button created if needed)
        gui.ab_av1_download_btn = None
        gui.ab_av1_check_btn = ttk.Button(ab_av1_frame, text="Check for Updates", command=gui.on_check_ab_av1_updates)
        gui.ab_av1_check_btn.pack(side="left", padx=(0, 10))
        ToolTip(
            gui.ab_av1_check_btn,
            "Check GitHub for the latest ab-av1 release.\nThis will make a network request to api.github.com.",
        )

    # Label to show update check result (initially empty)
    gui.ab_av1_update_label = ttk.Label(ab_av1_frame, text="")
    gui.ab_av1_update_label.pack(side="left")

    # FFmpeg version row
    ffmpeg_frame = ttk.Frame(version_frame)
    ffmpeg_frame.grid(row=1, column=0, sticky="w", padx=10, pady=(3, 5))
    gui.ffmpeg_frame = ffmpeg_frame  # Store reference for dynamic button creation

    # Get FFmpeg version info
    ffmpeg_available, _, ffmpeg_version_string, _ = check_ffmpeg_availability()
    ffmpeg_version, ffmpeg_source, ffmpeg_build = parse_ffmpeg_version(ffmpeg_version_string)
    using_vendor = is_using_vendor_ffmpeg()

    # Build display string
    if not ffmpeg_available:
        ffmpeg_display = "Not found"
    else:
        ffmpeg_display = ffmpeg_version or "Unknown"
        if using_vendor:
            ffmpeg_display += " (vendor)"
        elif ffmpeg_source:
            source_info = ffmpeg_source
            if ffmpeg_build:
                source_info += f" {ffmpeg_build}"
            ffmpeg_display += f" ({source_info})"

    ttk.Label(ffmpeg_frame, text="FFmpeg Version:").pack(side="left", padx=(0, 5))
    gui.ffmpeg_version_label = ttk.Label(ffmpeg_frame, text=ffmpeg_display, font=FONT_SYSTEM_BOLD)
    gui.ffmpeg_version_label.pack(side="left", padx=(0, 15))

    if not ffmpeg_available:
        # FFmpeg not found anywhere - show Download button only
        gui.ffmpeg_download_btn = ttk.Button(ffmpeg_frame, text="Download", command=gui.on_download_ffmpeg)
        gui.ffmpeg_download_btn.pack(side="left", padx=(0, 5))
        tooltip = "Download FFmpeg (gyan.dev full build) to vendor/ffmpeg/.\n"
        tooltip += "This will download ~100 MB and includes libsvtav1."
        ToolTip(gui.ffmpeg_download_btn, tooltip)
        gui.ffmpeg_check_btn = None
        gui.ffmpeg_update_label = None
        gui.ffmpeg_source = None
    else:
        # FFmpeg exists (vendor or system) - show Check for Updates button only
        gui.ffmpeg_download_btn = None

        # Determine source for update checking
        if using_vendor:
            gui.ffmpeg_source = "gyan.dev"
        elif ffmpeg_source in ("gyan.dev", "BtbN"):
            gui.ffmpeg_source = ffmpeg_source
        else:
            gui.ffmpeg_source = None  # Unknown source, can't check updates

        if gui.ffmpeg_source:
            gui.ffmpeg_check_btn = ttk.Button(
                ffmpeg_frame, text="Check for Updates", command=gui.on_check_ffmpeg_updates
            )
            gui.ffmpeg_check_btn.pack(side="left", padx=(0, 10))
            tooltip_text = f"Check GitHub for the latest FFmpeg release from {gui.ffmpeg_source}.\n"
            tooltip_text += "This will make a network request to api.github.com."
            ToolTip(gui.ffmpeg_check_btn, tooltip_text)

            gui.ffmpeg_update_label = ttk.Label(ffmpeg_frame, text="")
            gui.ffmpeg_update_label.pack(side="left")
        else:
            # Unknown source - can't check for updates
            gui.ffmpeg_check_btn = None
            gui.ffmpeg_update_label = None
