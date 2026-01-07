"""Preview dialog for adding items to the conversion queue."""

from __future__ import annotations

import tkinter as tk
from dataclasses import dataclass, field
from tkinter import ttk
from typing import TYPE_CHECKING

from src.models import OperationType

if TYPE_CHECKING:
    from src.models import QueueItem


@dataclass
class QueuePreviewData:
    """Data for the add-to-queue preview dialog."""

    to_add: list[tuple[str, bool]] = field(default_factory=list)  # (path, is_folder)
    duplicates: list[str] = field(default_factory=list)  # Already in queue (same op)
    conflicts: list[tuple[str, bool, QueueItem]] = field(default_factory=list)  # (path, is_folder, existing_item)
    skipped: list[tuple[str, str]] = field(default_factory=list)  # (path, reason) - filtered out
    operation_type: OperationType = OperationType.CONVERT
    estimated_time_sec: float | None = None
    estimated_savings_percent: float | None = None
    total_files_to_add: int | None = None  # Actual file count (for folders, counts nested files)

    @property
    def total_items(self) -> int:
        """Total number of items being considered."""
        return len(self.to_add) + len(self.duplicates) + len(self.conflicts) + len(self.skipped)

    @property
    def has_conflicts(self) -> bool:
        """Whether there are operation type conflicts."""
        return len(self.conflicts) > 0


class AddToQueuePreviewDialog(tk.Toplevel):
    """Unified preview dialog for adding items to queue.

    Works for single files or bulk operations. Shows:
    - Summary counts (to add, duplicates, conflicts)
    - Estimated time (if available)
    - Conflict resolution options (only if conflicts exist)
    """

    def __init__(self, parent: tk.Tk | tk.Toplevel, preview_data: QueuePreviewData):
        super().__init__(parent)
        self.preview_data = preview_data
        self.result: dict[str, str] = {"action": "cancel", "conflict_resolution": "skip"}

        self._conflict_var = tk.StringVar(value="replace")

        self._setup_window()
        self._create_widgets()
        self._center_on_parent(parent)

        # Make modal and wait
        self.grab_set()
        self.wait_window()

    def _setup_window(self) -> None:
        """Configure window properties."""
        op_name = self.preview_data.operation_type.value.capitalize()
        self.title(f"Add to Queue: {op_name}")
        if isinstance(self.master, (tk.Tk, tk.Toplevel)):
            self.transient(self.master)
        self.resizable(False, False)
        self.protocol("WM_DELETE_WINDOW", self._on_cancel)
        self.bind("<Escape>", lambda e: self._on_cancel())
        self.bind("<Return>", lambda e: self._on_confirm())

    def _create_widgets(self) -> None:
        """Create dialog widgets."""
        main_frame = ttk.Frame(self, padding=20)
        main_frame.pack(fill="both", expand=True)

        # Summary section
        self._create_summary_section(main_frame)

        # Estimates section (if available)
        if self.preview_data.estimated_time_sec:
            self._create_estimates_section(main_frame)

        # Conflict resolution section (only if conflicts)
        if self.preview_data.has_conflicts:
            self._create_conflict_section(main_frame)

        # Buttons
        self._create_buttons(main_frame)

    def _create_summary_section(self, parent: ttk.Frame) -> None:
        """Create the summary counts section."""
        summary_frame = ttk.LabelFrame(parent, text="Summary", padding=10)
        summary_frame.pack(fill="x", pady=(0, 10))

        data = self.preview_data
        op_name = data.operation_type.value.capitalize()

        # Files to add - use total_files_to_add if available (counts nested files in folders)
        add_count = data.total_files_to_add if data.total_files_to_add is not None else len(data.to_add)
        if data.has_conflicts:
            # When there are conflicts, we might add more depending on resolution
            add_text = f"{add_count} file(s) ready to add for {op_name}"
        else:
            add_text = f"{add_count} file(s) will be added for {op_name}"

        if add_count > 0:
            ttk.Label(summary_frame, text=add_text).pack(anchor="w")

        # Duplicates
        if data.duplicates:
            dup_text = f"{len(data.duplicates)} already in queue (will skip)"
            ttk.Label(summary_frame, text=dup_text, foreground="gray").pack(anchor="w")

        # Skipped (filtered out)
        if data.skipped:
            # Group by reason for cleaner display, sorted for consistent ordering
            reason_counts: dict[str, int] = {}
            for _, reason in data.skipped:
                reason_counts[reason] = reason_counts.get(reason, 0) + 1
            for reason, count in sorted(reason_counts.items()):
                skip_text = f"{count} {reason} (will skip)"
                ttk.Label(summary_frame, text=skip_text, foreground="gray").pack(anchor="w")

        # Conflicts
        if data.conflicts:
            existing_op = data.conflicts[0][2].operation_type.value.capitalize()
            conflict_text = f"{len(data.conflicts)} queued for {existing_op} (see options below)"
            ttk.Label(summary_frame, text=conflict_text, foreground="orange").pack(anchor="w")

    def _create_estimates_section(self, parent: ttk.Frame) -> None:
        """Create the time/savings estimates section."""
        est_frame = ttk.LabelFrame(parent, text="Estimates", padding=10)
        est_frame.pack(fill="x", pady=(0, 10))

        if self.preview_data.estimated_time_sec:
            time_str = self._format_duration(self.preview_data.estimated_time_sec)
            ttk.Label(est_frame, text=f"Estimated time: {time_str}").pack(anchor="w")

        if self.preview_data.estimated_savings_percent:
            savings_str = f"~{self.preview_data.estimated_savings_percent:.1f}%"
            ttk.Label(est_frame, text=f"Estimated savings: {savings_str}").pack(anchor="w")

    def _create_conflict_section(self, parent: ttk.Frame) -> None:
        """Create the conflict resolution options section."""
        conflict_frame = ttk.LabelFrame(parent, text="Conflict Resolution", padding=10)
        conflict_frame.pack(fill="x", pady=(0, 10))

        existing_op = self.preview_data.conflicts[0][2].operation_type.value.capitalize()
        new_op = self.preview_data.operation_type.value.capitalize()

        ttk.Label(conflict_frame, text=f"How should we handle files queued for {existing_op}?", wraplength=350).pack(
            anchor="w", pady=(0, 10)
        )

        # Radio buttons for conflict resolution
        ttk.Radiobutton(
            conflict_frame, text=f"Skip (keep existing {existing_op} tasks)", variable=self._conflict_var, value="skip"
        ).pack(anchor="w", pady=2)

        ttk.Radiobutton(
            conflict_frame,
            text=f"Keep both ({existing_op} + separate {new_op})",
            variable=self._conflict_var,
            value="keep_both",
        ).pack(anchor="w", pady=2)

        ttk.Radiobutton(
            conflict_frame, text=f"Replace with {new_op}", variable=self._conflict_var, value="replace"
        ).pack(anchor="w", pady=2)

    def _create_buttons(self, parent: ttk.Frame) -> None:
        """Create the action buttons."""
        btn_frame = ttk.Frame(parent)
        btn_frame.pack(fill="x", pady=(10, 0))

        ttk.Button(btn_frame, text="Cancel", command=self._on_cancel).pack(side="right", padx=(10, 0))

        add_text = "Add to Queue"
        ttk.Button(btn_frame, text=add_text, command=self._on_confirm).pack(side="right")

    def _center_on_parent(self, parent: tk.Tk | tk.Toplevel) -> None:
        """Center the dialog on the parent window."""
        self.update_idletasks()
        x = parent.winfo_x() + (parent.winfo_width() - self.winfo_width()) // 2
        y = parent.winfo_y() + (parent.winfo_height() - self.winfo_height()) // 2
        self.geometry(f"+{x}+{y}")

    def _on_confirm(self) -> None:
        """Handle confirm button click."""
        self.result = {"action": "confirm", "conflict_resolution": self._conflict_var.get()}
        self.destroy()

    def _on_cancel(self) -> None:
        """Handle cancel button click or window close."""
        self.result = {"action": "cancel", "conflict_resolution": "skip"}
        self.destroy()

    @staticmethod
    def _format_duration(seconds: float) -> str:
        """Format seconds as approximate human-readable duration for estimates."""
        if seconds < 60:  # noqa: PLR2004
            return f"~{int(seconds)}s"
        if seconds < 3600:  # noqa: PLR2004
            mins = int(seconds / 60)
            return f"~{mins}m"
        hours = int(seconds / 3600)
        mins = int((seconds % 3600) / 60)
        if mins > 0:
            return f"~{hours}h {mins}m"
        return f"~{hours}h"
