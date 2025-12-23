# src/gui/base.py
"""
Base GUI components for the AV1 Video Converter application.
"""

import tkinter as tk
from tkinter import ttk
from typing import Callable


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
        self.tooltip.configure(background="lightyellow")

        label = tk.Label(
            self.tooltip,
            text=self.text,
            background="lightyellow",
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


class TreeviewRowTooltip:
    """Dynamic tooltip for Treeview rows.

    Shows tooltips based on the row being hovered. The tooltip content is
    determined by a callback function that receives the item ID and returns
    the tooltip text (or None to show no tooltip).

    Usage:
        def get_tooltip(item_id: str) -> str | None:
            # Return tooltip text based on row data
            values = tree.item(item_id, "values")
            if values[0] == "Done":
                return "This file has been converted"
            return None

        TreeviewRowTooltip(tree, get_tooltip)
    """

    # Delay before showing tooltip (ms)
    SHOW_DELAY = 500
    # Offset from cursor
    OFFSET_X = 15
    OFFSET_Y = 15

    def __init__(self, treeview: ttk.Treeview, get_tooltip_text: Callable[[str], str | None]):
        """Initialize the treeview tooltip.

        Args:
            treeview: The Treeview widget to add tooltips to.
            get_tooltip_text: Callback that takes an item ID and returns tooltip
                text, or None if no tooltip should be shown for that item.
        """
        self.treeview = treeview
        self.get_tooltip_text = get_tooltip_text
        self.tooltip: tk.Toplevel | None = None
        self.current_item: str | None = None
        self.after_id: str | None = None

        # Bind events
        self.treeview.bind("<Motion>", self._on_motion, add="+")
        self.treeview.bind("<Leave>", self._on_leave, add="+")

    def _on_motion(self, event):
        """Handle mouse motion over the treeview."""
        # Identify which row the mouse is over
        item_id = self.treeview.identify_row(event.y)

        # If we moved to a different item (or no item), hide current tooltip
        if item_id != self.current_item:
            self._cancel_pending()
            self._hide_tooltip()
            self.current_item = item_id

            # Schedule showing tooltip for new item
            if item_id:
                self.after_id = self.treeview.after(
                    self.SHOW_DELAY, lambda: self._show_tooltip(event.x_root, event.y_root)
                )

    def _on_leave(self, event):
        """Handle mouse leaving the treeview."""
        self._cancel_pending()
        self._hide_tooltip()
        self.current_item = None

    def _cancel_pending(self):
        """Cancel any pending tooltip show."""
        if self.after_id:
            self.treeview.after_cancel(self.after_id)
            self.after_id = None

    def _show_tooltip(self, x: int, y: int):
        """Show the tooltip at the given position."""
        self.after_id = None

        if not self.current_item:
            return

        # Get tooltip text from callback
        text = self.get_tooltip_text(self.current_item)
        if not text:
            return

        # Create tooltip window
        self.tooltip = tk.Toplevel(self.treeview)
        self.tooltip.wm_overrideredirect(True)
        self.tooltip.wm_geometry(f"+{x + self.OFFSET_X}+{y + self.OFFSET_Y}")
        self.tooltip.configure(background="lightyellow")

        # Prevent tooltip from stealing focus
        self.tooltip.wm_attributes("-topmost", True)

        label = tk.Label(
            self.tooltip,
            text=text,
            background="lightyellow",
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
