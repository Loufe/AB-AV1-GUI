# src/gui/dependency_manager.py
"""
Dependency management for ab-av1 and FFmpeg.

Handles version checking, update detection, and downloading of external tools.
"""

import logging
import shutil
import threading
import webbrowser
from pathlib import Path
from tkinter import ttk
from typing import TYPE_CHECKING

from src.ab_av1.checker import check_ab_av1_latest_github, get_ab_av1_version
from src.gui.constants import (
    COLOR_STATUS_ERROR,
    COLOR_STATUS_INFO,
    COLOR_STATUS_NEUTRAL,
    COLOR_STATUS_SUCCESS,
    FONT_SYSTEM_NORMAL,
    FONT_SYSTEM_UNDERLINE,
)
from src.gui.dialogs import FFmpegDownloadDialog
from src.utils import (
    check_ffmpeg_availability,
    check_ffmpeg_latest_btbn,
    check_ffmpeg_latest_gyan,
    parse_ffmpeg_version,
)
from src.vendor_manager import download_ab_av1, download_ffmpeg

if TYPE_CHECKING:
    from src.gui.main_window import VideoConverterGUI

logger = logging.getLogger(__name__)


def check_ab_av1_updates(gui: "VideoConverterGUI") -> None:
    """Check GitHub for the latest ab-av1 version and update the label.

    Disables the check button, compares local version with GitHub release,
    and shows an Update button if a newer version is available.

    Args:
        gui: The main application window instance.
    """
    # Disable check button immediately
    if gui.ab_av1_check_btn:
        gui.ab_av1_check_btn.config(state="disabled")

    # Reset label state
    _reset_ab_av1_update_label(gui)
    gui.ab_av1_update_label.config(text="Checking...", foreground=COLOR_STATUS_NEUTRAL)
    gui.root.update_idletasks()

    local_version = get_ab_av1_version()
    latest_version, _release_url, message = check_ab_av1_latest_github()

    # Check button stays disabled permanently after use

    if latest_version is None:
        gui.ab_av1_update_label.config(text=message, foreground=COLOR_STATUS_ERROR)
        return

    if local_version is None:
        gui.ab_av1_update_label.config(text=f"Latest: {latest_version}", foreground=COLOR_STATUS_NEUTRAL)
        return

    # Compare versions
    if local_version == latest_version:
        gui.ab_av1_update_label.config(text=f"Up to date ({latest_version})", foreground=COLOR_STATUS_SUCCESS)
    else:
        # Update available - create Update button dynamically
        gui.ab_av1_update_label.config(text=f"Update available: {latest_version}", foreground=COLOR_STATUS_INFO)

        # Create Update button if it doesn't exist
        if gui.ab_av1_update_btn is None:
            gui.ab_av1_update_btn = ttk.Button(
                gui.ab_av1_frame, text="Update", command=lambda: download_ab_av1_update(gui)
            )
            gui.ab_av1_update_btn.pack(side="left", padx=(5, 0))


def _reset_ab_av1_update_label(gui: "VideoConverterGUI") -> None:
    """Reset the ab-av1 update label to non-clickable state."""
    gui.ab_av1_update_label.config(cursor="", font=FONT_SYSTEM_NORMAL)
    gui.ab_av1_update_label.unbind("<Button-1>")


def check_ffmpeg_updates(gui: "VideoConverterGUI") -> None:
    """Check GitHub for the latest FFmpeg version and update the label.

    Supports both gyan.dev and BtbN release sources. For gyan.dev,
    compares semantic versions. For BtbN, shows the latest build date.

    Args:
        gui: The main application window instance.
    """
    if not gui.ffmpeg_update_label or not gui.ffmpeg_source:
        return

    # Disable check button immediately
    if gui.ffmpeg_check_btn:
        gui.ffmpeg_check_btn.config(state="disabled")

    # Reset label state
    _reset_ffmpeg_update_label(gui)
    gui.ffmpeg_update_label.config(text="Checking...", foreground=COLOR_STATUS_NEUTRAL)
    gui.root.update_idletasks()

    # Get local version
    _, _, version_string, _ = check_ffmpeg_availability()
    local_version, _, _ = parse_ffmpeg_version(version_string)

    # Check GitHub based on detected source
    if gui.ffmpeg_source == "gyan.dev":
        latest_version, release_url, message = check_ffmpeg_latest_gyan()
    elif gui.ffmpeg_source == "BtbN":
        latest_version, release_url, message = check_ffmpeg_latest_btbn()
    else:
        gui.ffmpeg_update_label.config(text="Unknown source", foreground=COLOR_STATUS_ERROR)
        # Check button stays disabled permanently after use
        return

    # Check button stays disabled permanently after use

    if latest_version is None:
        gui.ffmpeg_update_label.config(text=message, foreground=COLOR_STATUS_ERROR)
        return

    # For BtbN, we can't compare versions (date-based tags), so just show latest and link
    if gui.ffmpeg_source == "BtbN":
        # Extract display date from tag like "autobuild-2025-12-18-12-50" -> "2025-12-18"
        if "autobuild" in latest_version:
            display_tag = latest_version.replace("autobuild-", "").rsplit("-", 2)[0]
        else:
            display_tag = latest_version
        gui.ffmpeg_update_label.config(
            text=f"Latest: {display_tag}", foreground=COLOR_STATUS_INFO, cursor="hand2", font=FONT_SYSTEM_UNDERLINE
        )
        if release_url:
            gui.ffmpeg_update_label.bind("<Button-1>", lambda e: webbrowser.open(release_url))
        return

    # For gyan.dev, we can compare semantic versions
    if local_version is None:
        gui.ffmpeg_update_label.config(text=f"Latest: {latest_version}", foreground=COLOR_STATUS_NEUTRAL)
        return

    if local_version == latest_version:
        gui.ffmpeg_update_label.config(text=f"Up to date ({latest_version})", foreground=COLOR_STATUS_SUCCESS)
    else:
        # Update available - create Update button dynamically
        gui.ffmpeg_update_label.config(text=f"Update available: {latest_version}", foreground=COLOR_STATUS_INFO)

        # Create Update button if it doesn't exist
        if gui.ffmpeg_update_btn is None:
            gui.ffmpeg_update_btn = ttk.Button(
                gui.ffmpeg_frame, text="Update", command=lambda: download_ffmpeg_update(gui)
            )
            gui.ffmpeg_update_btn.pack(side="left", padx=(5, 0))


def _reset_ffmpeg_update_label(gui: "VideoConverterGUI") -> None:
    """Reset the FFmpeg update label to non-clickable state."""
    if gui.ffmpeg_update_label:
        gui.ffmpeg_update_label.config(cursor="", font=FONT_SYSTEM_NORMAL)
        gui.ffmpeg_update_label.unbind("<Button-1>")


def download_ab_av1_update(gui: "VideoConverterGUI") -> None:
    """Download ab-av1 from GitHub in a background thread.

    Shows download progress in the update label and refreshes the
    version display on completion.

    Args:
        gui: The main application window instance.
    """
    # Disable button and show progress
    if gui.ab_av1_download_btn:
        gui.ab_av1_download_btn.config(state="disabled")
    if gui.ab_av1_update_btn:
        gui.ab_av1_update_btn.config(state="disabled")
    gui.ab_av1_update_label.config(text="Downloading...", foreground=COLOR_STATUS_NEUTRAL)
    gui.root.update_idletasks()

    def download_thread():
        def progress_callback(downloaded, total):
            if total > 0:
                pct = int(downloaded * 100 / total)
                gui.root.after(0, lambda: gui.ab_av1_update_label.config(text=f"Downloading... {pct}%"))

        success, message = download_ab_av1(progress_callback)

        def update_ui():
            if success:
                # Update version label
                new_version = get_ab_av1_version() or "Installed"
                gui.ab_av1_version_label.config(text=new_version)
                gui.ab_av1_update_label.config(text="Download complete!", foreground=COLOR_STATUS_SUCCESS)

                # Destroy the Update button if it exists (user is now up to date)
                if gui.ab_av1_update_btn:
                    gui.ab_av1_update_btn.destroy()
                    gui.ab_av1_update_btn = None

                # Re-enable download button if it exists
                if gui.ab_av1_download_btn:
                    gui.ab_av1_download_btn.config(state="normal")
            else:
                gui.ab_av1_update_label.config(text=message, foreground=COLOR_STATUS_ERROR)
                # Re-enable buttons on failure
                if gui.ab_av1_download_btn:
                    gui.ab_av1_download_btn.config(state="normal")
                if gui.ab_av1_update_btn:
                    gui.ab_av1_update_btn.config(state="normal")

        gui.root.after(0, update_ui)

    threading.Thread(target=download_thread, daemon=True).start()


def download_ffmpeg_update(gui: "VideoConverterGUI") -> None:
    """Download FFmpeg to vendor folder in a background thread.

    Shows a confirmation dialog if system FFmpeg exists, then downloads
    with progress display.

    Args:
        gui: The main application window instance.
    """
    # Check for existing system FFmpeg
    existing_ffmpeg = shutil.which("ffmpeg")

    # If system FFmpeg exists, show informational dialog
    if existing_ffmpeg:
        existing_dir = Path(existing_ffmpeg).parent
        dialog = FFmpegDownloadDialog(gui.root, existing_dir)
        if not dialog.show():
            return  # User cancelled

    # Disable button and show progress
    if gui.ffmpeg_download_btn:
        gui.ffmpeg_download_btn.config(state="disabled")
    if gui.ffmpeg_update_btn:
        gui.ffmpeg_update_btn.config(state="disabled")
    if gui.ffmpeg_update_label:
        gui.ffmpeg_update_label.config(text="Downloading...", foreground=COLOR_STATUS_NEUTRAL)
    gui.root.update_idletasks()

    def download_thread():
        def progress_callback(downloaded, total):
            if total > 0:
                mb_downloaded = downloaded / (1024 * 1024)
                mb_total = total / (1024 * 1024)
                text = f"Downloading... {mb_downloaded:.0f}/{mb_total:.0f} MB"
                label = gui.ffmpeg_update_label
                if label:
                    gui.root.after(0, lambda t=text, lbl=label: lbl.config(text=t))

        success, message = download_ffmpeg(progress_callback)

        def update_ui():
            if success:
                # Update version label
                _, _, version_string, _ = check_ffmpeg_availability()
                version, _, _ = parse_ffmpeg_version(version_string)
                display = f"{version} (vendor)" if version else "Installed"
                gui.ffmpeg_version_label.config(text=display)
                if gui.ffmpeg_update_label:
                    gui.ffmpeg_update_label.config(text="Download complete!", foreground=COLOR_STATUS_SUCCESS)
                # Update ffmpeg_source to gyan.dev
                gui.ffmpeg_source = "gyan.dev"

                # Destroy the Update button if it exists (user is now up to date)
                if gui.ffmpeg_update_btn:
                    gui.ffmpeg_update_btn.destroy()
                    gui.ffmpeg_update_btn = None

                # Re-enable download button if it exists
                if gui.ffmpeg_download_btn:
                    gui.ffmpeg_download_btn.config(state="normal")
            else:
                if gui.ffmpeg_update_label:
                    gui.ffmpeg_update_label.config(text=message, foreground=COLOR_STATUS_ERROR)
                # Re-enable buttons on failure
                if gui.ffmpeg_download_btn:
                    gui.ffmpeg_download_btn.config(state="normal")
                if gui.ffmpeg_update_btn:
                    gui.ffmpeg_update_btn.config(state="normal")

        gui.root.after(0, update_ui)

    threading.Thread(target=download_thread, daemon=True).start()
