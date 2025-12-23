# src/gui/dialogs/ffmpeg_download_dialog.py
"""FFmpeg download dialog - shown when system FFmpeg exists but user wants vendor copy."""

import tkinter as tk
from pathlib import Path
from tkinter import ttk


class FFmpegDownloadDialog(tk.Toplevel):
    """Informational dialog shown when downloading FFmpeg while a system copy exists.

    This dialog informs the user that:
    - A system FFmpeg was detected at [path]
    - We'll install a portable copy to vendor/ffmpeg/
    - To update their system FFmpeg, they should use their package manager
    """

    def __init__(self, parent: tk.Tk, existing_ffmpeg_dir: Path):
        """Initialize the dialog.

        Args:
            parent: Parent window
            existing_ffmpeg_dir: Directory containing existing ffmpeg.exe
        """
        super().__init__(parent)
        self.parent = parent
        self.existing_ffmpeg_dir = existing_ffmpeg_dir
        self.cancelled = True

        self._setup_window()
        self._create_widgets()
        self._bind_events()

    def _setup_window(self):
        """Configure the dialog window."""
        self.title("Download FFmpeg")
        self.transient(self.parent)
        self.grab_set()
        self.resizable(False, False)

    def _create_widgets(self):
        """Create dialog widgets."""
        main_frame = ttk.Frame(self, padding=20)
        main_frame.pack(fill="both", expand=True)

        # Header
        ttk.Label(
            main_frame,
            text="Install Portable FFmpeg",
            font=("TkDefaultFont", 10, "bold"),
        ).pack(anchor="w", pady=(0, 15))

        # Existing installation info
        ttk.Label(
            main_frame,
            text="System FFmpeg detected at:",
        ).pack(anchor="w")

        ttk.Label(
            main_frame,
            text=f"    {self.existing_ffmpeg_dir}",
            foreground="gray",
        ).pack(anchor="w", pady=(0, 10))

        # What will happen
        ttk.Label(
            main_frame,
            text="This will install a portable copy to vendor/ffmpeg/",
        ).pack(anchor="w")

        ttk.Label(
            main_frame,
            text="This app will use the portable copy instead.",
        ).pack(anchor="w")

        ttk.Label(
            main_frame,
            text="Your system installation will not be modified.",
            foreground="#c00000",
        ).pack(anchor="w", pady=(0, 10))

        # Hint about updating system install
        ttk.Label(
            main_frame,
            text="To update your system FFmpeg, use your package manager:",
            foreground="gray",
        ).pack(anchor="w")

        ttk.Label(
            main_frame,
            text="    choco upgrade ffmpeg  /  winget upgrade ffmpeg",
            foreground="gray",
            font=("Consolas", 9),
        ).pack(anchor="w")

        # Separator
        ttk.Separator(main_frame, orient="horizontal").pack(fill="x", pady=15)

        # Download info
        ttk.Label(
            main_frame,
            text="Download: ~100 MB (gyan.dev full build with libsvtav1)",
            foreground="gray",
        ).pack(anchor="w")

        # Buttons
        btn_frame = ttk.Frame(main_frame)
        btn_frame.pack(fill="x", pady=(20, 0))

        ttk.Button(btn_frame, text="Cancel", command=self._on_cancel).pack(side="right", padx=(5, 0))
        ttk.Button(btn_frame, text="Download", command=self._on_download).pack(side="right")

        self._center_on_parent()

    def _center_on_parent(self):
        """Center the dialog on its parent window."""
        self.update_idletasks()
        width = self.winfo_reqwidth()
        height = self.winfo_reqheight()
        x = self.parent.winfo_x() + (self.parent.winfo_width() - width) // 2
        y = self.parent.winfo_y() + (self.parent.winfo_height() - height) // 2
        self.geometry(f"{width}x{height}+{x}+{y}")

    def _bind_events(self):
        """Bind keyboard events."""
        self.bind("<Escape>", lambda e: self._on_cancel())
        self.bind("<Return>", lambda e: self._on_download())
        self.protocol("WM_DELETE_WINDOW", self._on_cancel)

    def _on_cancel(self):
        """Handle cancel button click."""
        self.cancelled = True
        self.destroy()

    def _on_download(self):
        """Handle download button click."""
        self.cancelled = False
        self.destroy()

    def show(self) -> bool:
        """Show the dialog and wait for result.

        Returns:
            True if user clicked Download, False if cancelled.
        """
        self.wait_window()
        return not self.cancelled
