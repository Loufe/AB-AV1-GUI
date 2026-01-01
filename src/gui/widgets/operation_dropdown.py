# src/gui/widgets/operation_dropdown.py
"""
In-cell dropdown overlay for the operation column in queue Treeview.

Provides contextual operation options based on whether a file has cached
CRF analysis data (Layer 2):
- Without Layer 2: "Analyze + Convert", "Analyze Only"
- With Layer 2: "Convert", "Re-analyze + Convert", "Analyze Only"
"""

from __future__ import annotations

import contextlib
import tkinter as tk
from tkinter import ttk
from typing import TYPE_CHECKING, Callable

from src.gui.constants import COLOR_MENU_ACTIVE_BG, COLOR_MENU_ACTIVE_FG, COLOR_MENU_BACKGROUND
from src.history_index import compute_path_hash, get_history_index
from src.models import OperationType, QueueItem

if TYPE_CHECKING:
    from src.gui.main_window import VideoConverterGUI


# Display labels for operation types
OPERATION_OPTIONS_WITH_LAYER2 = ("Convert", "Re-analyze + Convert", "Analyze Only")
OPERATION_OPTIONS_WITHOUT_LAYER2 = ("Analyze + Convert", "Analyze Only")

# Map display strings to OperationType enum values
OPERATION_DISPLAY_TO_ENUM: dict[str, OperationType] = {
    "Analyze + Convert": OperationType.CONVERT,
    "Convert": OperationType.CONVERT,
    "Re-analyze + Convert": OperationType.CONVERT,
    "Analyze Only": OperationType.ANALYZE,
}

# Operation column index in Treeview (after tree column #0)
# Columns: #0=Name, #1=size, #2=est_time, #3=operation, #4=output, #5=status
OPERATION_COLUMN_ID = "#3"


class OperationDropdownManager:
    """Manages in-cell dropdown overlay for the queue tree operation column.

    Handles positioning, focus management, and cleanup of the Combobox overlay.
    Only one dropdown can be active at a time.
    """

    def __init__(self, gui: VideoConverterGUI) -> None:
        self._gui = gui
        self._active_combo: ttk.Combobox | None = None
        self._active_item_id: str | None = None
        self._pending_cleanup_id: str | None = None

    def is_operation_column_click(self, event: tk.Event) -> bool:
        """Check if click event is on the operation column of a valid item."""
        tree = self._gui.queue_tree
        item_id = tree.identify_row(event.y)
        col_id = tree.identify_column(event.x)
        return bool(item_id and col_id == OPERATION_COLUMN_ID)

    def show_dropdown(self, event: tk.Event) -> bool:
        """Show operation dropdown at clicked cell.

        Args:
            event: The click event from Treeview

        Returns:
            True if dropdown was shown, False otherwise
        """
        # Don't show while queue is processing
        if self._gui.session.running:
            return False

        tree = self._gui.queue_tree
        item_id = tree.identify_row(event.y)

        if not item_id:
            return False

        # Get the queue item
        queue_item = self._gui.get_queue_item_for_tree_item(item_id)
        if not queue_item:
            return False

        # Don't allow editing folder items directly (only files)
        if queue_item.is_folder:
            return False

        # Cleanup any existing dropdown
        self._cleanup()

        # Get cell bounding box
        try:
            bbox = tree.bbox(item_id, OPERATION_COLUMN_ID)
        except tk.TclError:
            return False

        if not bbox:  # Item not visible (scrolled out of view)
            return False

        x, y, width, height = bbox

        # Create and position the combobox
        combo = ttk.Combobox(tree, state="readonly")
        self._populate_options(combo, queue_item)

        # Place inside the tree widget at cell position
        combo.place(x=x, y=y, width=max(width, 130), height=height)

        # Store references
        self._active_combo = combo
        self._active_item_id = item_id

        # Bind events for selection and cleanup
        combo.bind("<<ComboboxSelected>>", lambda e: self._on_select(queue_item))
        combo.bind("<Escape>", lambda e: self._cleanup())
        combo.bind("<FocusOut>", self._on_focus_out)

        # Focus and open dropdown
        combo.focus_set()
        combo.event_generate("<Down>")  # Open the dropdown list

        return True

    def _populate_options(self, combo: ttk.Combobox, queue_item: QueueItem) -> None:
        """Populate dropdown with contextual options based on Layer 2 data."""
        has_layer2 = self._has_layer2_data(queue_item.source_path)
        is_analyze = queue_item.operation_type == OperationType.ANALYZE

        # Options and default selection depend on Layer 2 availability
        # With Layer 2: ["Convert", "Re-analyze + Convert", "Analyze Only"] (idx 0, 1, 2)
        # Without Layer 2: ["Analyze + Convert", "Analyze Only"] (idx 0, 1)
        options = OPERATION_OPTIONS_WITH_LAYER2 if has_layer2 else OPERATION_OPTIONS_WITHOUT_LAYER2
        current_idx = (2 if has_layer2 else 1) if is_analyze else 0

        combo["values"] = options
        combo.current(current_idx)

    def _has_layer2_data(self, source_path: str) -> bool:
        """Check if file has cached CRF analysis results."""
        path_hash = compute_path_hash(source_path)
        record = get_history_index().get(path_hash)
        return bool(record and record.best_crf is not None and record.best_vmaf_achieved is not None)

    def _on_select(self, queue_item: QueueItem) -> None:
        """Handle dropdown selection."""
        if not self._active_combo:
            return

        selected = self._active_combo.get()
        if not selected:
            self._cleanup()
            return

        # Map display string to enum
        new_operation = OPERATION_DISPLAY_TO_ENUM.get(selected)
        if new_operation is None:
            self._cleanup()
            return

        # Handle "Re-analyze + Convert" - clear cached Layer 2 data
        if selected == "Re-analyze + Convert":
            self._clear_layer2_data(queue_item.source_path)

        # Update queue item if operation changed
        if queue_item.operation_type != new_operation:
            queue_item.operation_type = new_operation
            self._gui.save_queue_to_config()
            self._gui.refresh_queue_tree()

        self._cleanup()

    def _clear_layer2_data(self, source_path: str) -> None:
        """Clear cached CRF/VMAF data so file will be re-analyzed."""
        path_hash = compute_path_hash(source_path)
        index = get_history_index()
        record = index.get(path_hash)

        if record:
            record.best_crf = None
            record.best_vmaf_achieved = None
            record.predicted_output_size = None
            record.predicted_size_reduction = None
            index.save()

    def _on_focus_out(self, event: tk.Event) -> None:
        """Handle focus loss - cleanup after brief delay to allow selection."""
        # Cancel any pending cleanup
        if self._pending_cleanup_id:
            self._gui.root.after_cancel(self._pending_cleanup_id)

        # Delay cleanup to allow ComboboxSelected event to fire first
        self._pending_cleanup_id = self._gui.root.after(150, self._cleanup)

    def _cleanup(self) -> None:
        """Destroy active dropdown and reset state."""
        # Cancel pending cleanup timer
        if self._pending_cleanup_id:
            with contextlib.suppress(tk.TclError):
                self._gui.root.after_cancel(self._pending_cleanup_id)
            self._pending_cleanup_id = None

        # Destroy combo widget
        if self._active_combo:
            with contextlib.suppress(tk.TclError):
                if self._active_combo.winfo_exists():
                    self._active_combo.destroy()
            self._active_combo = None

        self._active_item_id = None


def build_operation_submenu(
    parent_menu: tk.Menu, queue_item: QueueItem, on_operation_change: Callable[[QueueItem, str], None]
) -> tk.Menu:
    """Build a submenu for changing operation type.

    Args:
        parent_menu: The parent context menu
        queue_item: The queue item to modify
        on_operation_change: Callback(queue_item, selected_display_string)

    Returns:
        The submenu widget
    """
    submenu = tk.Menu(
        parent_menu,
        tearoff=0,
        background=COLOR_MENU_BACKGROUND,
        activebackground=COLOR_MENU_ACTIVE_BG,
        activeforeground=COLOR_MENU_ACTIVE_FG,
    )

    # Determine available options based on Layer 2 data
    path_hash = compute_path_hash(queue_item.source_path)
    record = get_history_index().get(path_hash)
    has_layer2 = bool(record and record.best_crf is not None and record.best_vmaf_achieved is not None)

    options = OPERATION_OPTIONS_WITH_LAYER2 if has_layer2 else OPERATION_OPTIONS_WITHOUT_LAYER2

    # Determine current selection for checkmark indicator
    if queue_item.operation_type == OperationType.ANALYZE:
        current_display = "Analyze Only"
    else:
        current_display = "Convert" if has_layer2 else "Analyze + Convert"

    # Add options with checkmark on current
    for option in options:
        submenu.add_command(
            label=f"{'> ' if option == current_display else '   '}{option}",
            command=lambda opt=option: on_operation_change(queue_item, opt),
        )

    return submenu
