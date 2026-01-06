# src/gui/base.py
"""
Base GUI components for the AV1 Video Converter application.
"""

import logging
import os
import subprocess
import sys
import tkinter as tk
from tkinter import ttk
from typing import Callable

from src.gui.constants import COLOR_TOOLTIP_BACKGROUND
from src.gui.tree_utils import get_column_name

logger = logging.getLogger(__name__)


def open_in_explorer(path: str) -> None:
    """Open a file or folder in the native file explorer."""
    if not os.path.exists(path):
        logger.warning(f"Cannot open in explorer - path does not exist: {path}")
        return

    try:
        if sys.platform == "win32":
            os.startfile(path)
        elif sys.platform == "darwin":
            subprocess.run(["open", path], check=False)
        else:
            subprocess.run(["xdg-open", path], check=False)
    except OSError:
        logger.exception(f"Failed to open path in explorer: {path}")


def reveal_in_explorer(file_path: str) -> None:
    """Open the containing folder and select the file."""
    if not os.path.exists(file_path):
        logger.warning(f"Cannot reveal in explorer - path does not exist: {file_path}")
        return

    try:
        if sys.platform == "win32":
            subprocess.run(["explorer", "/select,", file_path], check=False)
        elif sys.platform == "darwin":
            subprocess.run(["open", "-R", file_path], check=False)
        else:
            # Linux: just open the containing folder
            subprocess.run(["xdg-open", os.path.dirname(file_path)], check=False)
    except OSError:
        logger.exception(f"Failed to reveal path in explorer: {file_path}")


class ToolTip:
    """Tooltip class for providing helpful information"""

    def __init__(self, widget, text):
        self.widget = widget
        self.text = text
        self.tooltip = None
        self.widget.bind("<Enter>", self.show_tooltip)
        self.widget.bind("<Leave>", self.hide_tooltip)

    def show_tooltip(self, event=None):
        # Prevent duplicate tooltips if Enter fires twice
        if self.tooltip:
            return

        x, y, _, _ = self.widget.bbox("insert")
        x += self.widget.winfo_rootx() + 25
        y += self.widget.winfo_rooty() + 25

        # Create a toplevel window
        self.tooltip = tk.Toplevel(self.widget)
        self.tooltip.wm_overrideredirect(True)
        self.tooltip.wm_geometry(f"+{x}+{y}")
        self.tooltip.configure(background=COLOR_TOOLTIP_BACKGROUND)

        label = tk.Label(
            self.tooltip,
            text=self.text,
            background=COLOR_TOOLTIP_BACKGROUND,
            relief="solid",
            borderwidth=1,
            padx=5,
            pady=3,
            wraplength=300,
            justify="left",
        )
        label.pack()

    def hide_tooltip(self, event=None):
        if self.tooltip:
            self.tooltip.destroy()
            self.tooltip = None


class _TreeviewTooltipBase:
    """Base class for Treeview tooltips with common display logic."""

    SHOW_DELAY = 500
    OFFSET_X = 15
    OFFSET_Y = 15

    def __init__(self, treeview: ttk.Treeview):
        self.treeview = treeview
        self.tooltip: tk.Toplevel | None = None
        self.current_target: str | None = None
        self.after_id: str | None = None

        self.treeview.bind("<Motion>", self._on_motion, add="+")
        self.treeview.bind("<Leave>", self._on_leave, add="+")

    def _identify_target(self, event) -> str | None:
        """Identify what the mouse is over. Override in subclasses."""
        raise NotImplementedError

    def _get_tooltip_text(self, target: str) -> str | None:
        """Get tooltip text for target. Override in subclasses."""
        raise NotImplementedError

    def _on_motion(self, event):
        """Handle mouse motion over the treeview."""
        target = self._identify_target(event)

        if target != self.current_target:
            self._cancel_pending()
            self._hide_tooltip()
            self.current_target = target

            if target and self._get_tooltip_text(target):
                self.after_id = self.treeview.after(
                    self.SHOW_DELAY, lambda: self._show_tooltip(event.x_root, event.y_root)
                )

    def _on_leave(self, event):
        """Handle mouse leaving the treeview."""
        self._cancel_pending()
        self._hide_tooltip()
        self.current_target = None

    def _cancel_pending(self):
        """Cancel any pending tooltip show."""
        if self.after_id:
            self.treeview.after_cancel(self.after_id)
            self.after_id = None

    def _show_tooltip(self, x: int, y: int):
        """Show the tooltip at the given position."""
        self.after_id = None

        if not self.current_target:
            return

        text = self._get_tooltip_text(self.current_target)
        if not text:
            return

        self.tooltip = tk.Toplevel(self.treeview)
        self.tooltip.wm_overrideredirect(True)
        self.tooltip.wm_geometry(f"+{x + self.OFFSET_X}+{y + self.OFFSET_Y}")
        self.tooltip.configure(background=COLOR_TOOLTIP_BACKGROUND)
        self.tooltip.wm_attributes("-topmost", True)

        label = tk.Label(
            self.tooltip,
            text=text,
            background=COLOR_TOOLTIP_BACKGROUND,
            relief="solid",
            borderwidth=1,
            padx=6,
            pady=4,
            wraplength=350,
            justify="left",
        )
        label.pack()

    def _hide_tooltip(self):
        """Hide the current tooltip."""
        if self.tooltip:
            self.tooltip.destroy()
            self.tooltip = None


class TreeviewRowTooltip(_TreeviewTooltipBase):
    """Dynamic tooltip for Treeview rows.

    Usage:
        def get_tooltip(item_id: str) -> str | None:
            values = tree.item(item_id, "values")
            return "Done" if values[0] == "Done" else None

        TreeviewRowTooltip(tree, get_tooltip)
    """

    def __init__(self, treeview: ttk.Treeview, get_tooltip_text: Callable[[str], str | None]):
        super().__init__(treeview)
        self._get_text_callback = get_tooltip_text

    def _identify_target(self, event) -> str | None:
        return self.treeview.identify_row(event.y) or None

    def _get_tooltip_text(self, target: str) -> str | None:
        return self._get_text_callback(target)


class TreeviewHeaderTooltip(_TreeviewTooltipBase):
    """Tooltip for Treeview column headers.

    Usage:
        TreeviewHeaderTooltip(tree, {
            "efficiency": "GB saved per hour of conversion time",
        })
    """

    SHOW_DELAY = 400

    def __init__(self, treeview: ttk.Treeview, column_tooltips: dict[str, str]):
        super().__init__(treeview)
        self.column_tooltips = column_tooltips

    def _identify_target(self, event) -> str | None:
        if self.treeview.identify_region(event.x, event.y) != "heading":
            return None
        column_id = self.treeview.identify_column(event.x)
        return get_column_name(self.treeview, column_id)

    def _get_tooltip_text(self, target: str) -> str | None:
        return self.column_tooltips.get(target)
