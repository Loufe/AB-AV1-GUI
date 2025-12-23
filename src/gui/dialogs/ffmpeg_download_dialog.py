# src/gui/dialogs/ffmpeg_download_dialog.py
"""FFmpeg download options dialog."""

import tkinter as tk
import uuid
from dataclasses import dataclass
from pathlib import Path
from tkinter import ttk

from src.vendor_manager import FFMPEG_DIR


@dataclass
class DownloadDialogResult:
    """Result from the FFmpeg download dialog."""

    cancelled: bool
    option: str  # "vendor" | "existing"
    path: Path  # Where to install


def _is_dir_writable(path: Path) -> bool:
    """Check if a directory is writable by actually creating a test file.

    os.access() doesn't work reliably on Windows with UAC-protected directories,
    so we must actually attempt file creation to get an accurate result.
    """
    test_file = path / f".write_test_{uuid.uuid4().hex[:8]}.tmp"
    try:
        test_file.touch()
        test_file.unlink()
        return True
    except (OSError, PermissionError):
        return False


class FFmpegDownloadDialog(tk.Toplevel):
    """Modal dialog for FFmpeg download options."""

    def __init__(self, parent: tk.Tk, existing_ffmpeg_path: str | None):
        """Initialize the dialog.

        Args:
            parent: Parent window
            existing_ffmpeg_path: Path to existing ffmpeg.exe from shutil.which(), or None
        """
        super().__init__(parent)
        self.parent = parent
        self.existing_ffmpeg_path = existing_ffmpeg_path
        self.existing_ffmpeg_dir = Path(existing_ffmpeg_path).parent if existing_ffmpeg_path else None

        # Check if existing directory is writable (for protected paths like Chocolatey)
        self.existing_dir_writable = (
            _is_dir_writable(self.existing_ffmpeg_dir) if self.existing_ffmpeg_dir else False
        )

        self.result: DownloadDialogResult | None = None

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
        # Main frame with padding
        main_frame = ttk.Frame(self, padding=20)
        main_frame.pack(fill="both", expand=True)

        # Header
        ttk.Label(
            main_frame,
            text="Where would you like to install FFmpeg?",
            font=("TkDefaultFont", 10, "bold"),
        ).pack(anchor="w", pady=(0, 15))

        # Radio button variable
        self.selected_option = tk.StringVar(value="vendor")

        # Option A: Vendor folder (recommended)
        option_a_frame = ttk.Frame(main_frame)
        option_a_frame.pack(fill="x", pady=(0, 10))

        ttk.Radiobutton(
            option_a_frame,
            text="App folder (vendor/ffmpeg/)",
            variable=self.selected_option,
            value="vendor",
        ).pack(anchor="w")

        ttk.Label(
            option_a_frame,
            text="    Only this app will use it. Portable and isolated.  [Recommended]",
            foreground="gray",
        ).pack(anchor="w")

        # Option B: Update existing (only if detected AND writable)
        if self.existing_ffmpeg_path and self.existing_dir_writable:
            option_b_frame = ttk.Frame(main_frame)
            option_b_frame.pack(fill="x", pady=(0, 10))

            ttk.Radiobutton(
                option_b_frame,
                text="Update existing installation",
                variable=self.selected_option,
                value="existing",
            ).pack(anchor="w")

            ttk.Label(
                option_b_frame,
                text=f"    Detected at: {self.existing_ffmpeg_dir}",
                foreground="gray",
            ).pack(anchor="w")

            # Warning label
            warning_frame = ttk.Frame(option_b_frame)
            warning_frame.pack(anchor="w", padx=(20, 0))
            ttk.Label(warning_frame, text="⚠", foreground="orange").pack(side="left")
            ttk.Label(
                warning_frame,
                text=" May affect other applications using this FFmpeg",
                foreground="orange",
            ).pack(side="left")

        # Show info about non-writable existing installation
        elif self.existing_ffmpeg_path and not self.existing_dir_writable:
            info_frame = ttk.Frame(main_frame)
            info_frame.pack(fill="x", pady=(0, 10))

            ttk.Label(
                info_frame,
                text=f"Existing FFmpeg at: {self.existing_ffmpeg_dir}",
                foreground="gray",
            ).pack(anchor="w")

            ttk.Label(
                info_frame,
                text="    (Protected location - update via package manager)",
                foreground="gray",
            ).pack(anchor="w")

        # Separator
        ttk.Separator(main_frame, orient="horizontal").pack(fill="x", pady=15)

        # Download info
        ttk.Label(
            main_frame,
            text="Download: ~100 MB (gyan.dev full build with libsvtav1)",
            foreground="gray",
        ).pack(anchor="w")

        # Buttons frame
        btn_frame = ttk.Frame(main_frame)
        btn_frame.pack(fill="x", pady=(20, 0))

        ttk.Button(btn_frame, text="Cancel", command=self._on_cancel).pack(side="right", padx=(5, 0))
        ttk.Button(btn_frame, text="Download", command=self._on_download).pack(side="right")

        # Center on parent after widgets are created so we know the actual size
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
        self.result = DownloadDialogResult(cancelled=True, option="", path=Path())
        self.destroy()

    def _on_download(self):
        """Handle download button click."""
        option = self.selected_option.get()
        # Use existing_ffmpeg_dir if selected and set, otherwise fall back to vendor dir
        path = self.existing_ffmpeg_dir if option == "existing" and self.existing_ffmpeg_dir else FFMPEG_DIR

        self.result = DownloadDialogResult(cancelled=False, option=option, path=path)
        self.destroy()

    def show(self) -> DownloadDialogResult:
        """Show the dialog and wait for result.

        Returns:
            DownloadDialogResult with user's selection
        """
        self.wait_window()
        return self.result or DownloadDialogResult(cancelled=True, option="", path=Path())
