# src/gui/queue_controller.py
# ruff: noqa: SLF001  # This module accesses VideoConverterGUI internals by design
"""
Queue tab event handling and coordination.

Manages queue UI interactions including:
- Adding files/folders to queue
- Removing items from queue
- Queue selection and properties panel updates
- Item property changes (output mode, suffix, folder)
"""

from tkinter import filedialog, messagebox

from src.models import OperationType, OutputMode

# =============================================================================
# Add to Queue
# =============================================================================


def on_add_folder_to_queue(gui) -> None:
    """Add a folder to the conversion queue.

    Args:
        gui: The VideoConverterGUI instance.
    """
    folder = filedialog.askdirectory(title="Select Folder to Convert")
    if not folder:
        return
    add_to_queue(gui, folder, is_folder=True)


def on_add_files_to_queue(gui) -> None:
    """Add individual files to the conversion queue.

    Args:
        gui: The VideoConverterGUI instance.
    """
    files = filedialog.askopenfilenames(
        title="Select Video Files",
        filetypes=[("Video files", "*.mp4 *.mkv *.avi *.wmv *.mov *.webm"), ("All files", "*.*")],
    )
    for f in files:
        add_to_queue(gui, f, is_folder=False)


def add_to_queue(
    gui, path: str, is_folder: bool, operation_type: OperationType = OperationType.CONVERT
) -> str:
    """Add a single item to the queue (convenience wrapper).

    For selective adds without conflicts, this is silent.
    For conflicts, shows the preview dialog.

    Args:
        gui: The VideoConverterGUI instance.
        path: Path to file or folder to add.
        is_folder: True if path is a folder, False if file.
        operation_type: The operation type (CONVERT or ANALYZE).

    Returns:
        "added", "duplicate", "conflict_added", "conflict_replaced", or "cancelled"
    """
    result = gui.add_items_to_queue([(path, is_folder)], operation_type, force_preview=False)

    if result["added"] > 0:
        return "added"
    if result["duplicate"] > 0:
        return "duplicate"
    if result["conflict_added"] > 0:
        return "conflict_added"
    if result["conflict_replaced"] > 0:
        return "conflict_replaced"
    return "cancelled"


# =============================================================================
# Remove from Queue
# =============================================================================


def on_remove_from_queue(gui) -> None:
    """Remove selected items from queue.

    Args:
        gui: The VideoConverterGUI instance.
    """
    # Get selected items
    selected = gui.queue_tree.selection()
    if not selected:
        return

    # Collect queue items to remove using O(1) lookups
    items_to_remove = []
    for tree_id in selected:
        item = gui.get_queue_item_for_tree_item(tree_id)
        if item:
            items_to_remove.append(item)

    # Remove from queue
    for item in items_to_remove:
        gui._queue_items.remove(item)
        gui._queue_items_by_id.pop(item.id, None)

    gui.save_queue_to_config()
    gui.refresh_queue_tree()
    gui.sync_queue_tags_to_analysis_tree()


def on_clear_queue(gui) -> None:
    """Clear all items from queue.

    Args:
        gui: The VideoConverterGUI instance.
    """
    if gui.session.running:
        messagebox.showwarning("Queue Running", "Cannot clear queue while conversion is running.")
        return
    if not gui._queue_items:
        return
    if messagebox.askyesno("Clear Queue", "Remove all items from the queue?"):
        gui._queue_items.clear()
        gui._queue_items_by_id.clear()
        gui.save_queue_to_config()
        gui.refresh_queue_tree()
        gui.sync_queue_tags_to_analysis_tree()


# =============================================================================
# Selection and Properties Panel
# =============================================================================


def on_queue_selection_changed(gui) -> None:
    """Handle selection change in queue tree.

    Updates the properties panel based on the selected queue item.

    Args:
        gui: The VideoConverterGUI instance.
    """
    selected = gui.queue_tree.selection()
    if not selected:
        # Disable controls and show placeholder when nothing selected
        gui.item_mode_combo.config(state="disabled")
        gui.item_suffix_entry.config(state="disabled")
        gui.item_folder_entry.config(state="disabled")
        gui.item_folder_browse_button.config(state="disabled")
        gui.item_output_mode.set("")
        gui.item_suffix.set("")
        gui.item_output_folder.set("")
        gui.item_source_label.config(text="Select an item to configure")
        return

    # Get the queue item for the selected tree item
    queue_item = gui.get_queue_item_for_tree_item(selected[0])
    if not queue_item:
        return

    # Check if this is an ANALYZE-only operation
    is_analyze = queue_item.operation_type == OperationType.ANALYZE

    # Update properties panel with selected item's values
    if is_analyze:
        # Disable output-related fields for ANALYZE operations (no output file produced)
        gui.item_output_mode.set("—")
        gui.item_suffix.set("")
        gui.item_output_folder.set("")
        gui.item_source_label.config(text=f"{queue_item.source_path} (Analysis only - no output file)")
        # Disable the widgets
        gui.item_mode_combo.config(state="disabled")
        gui.item_suffix_entry.config(state="disabled")
        gui.item_folder_entry.config(state="disabled")
        gui.item_folder_browse_button.config(state="disabled")
    else:
        # Enable and populate for CONVERT operations
        gui.item_mode_combo.config(state="readonly")
        gui.item_suffix_entry.config(state="normal")
        gui.item_folder_entry.config(state="normal")
        gui.item_folder_browse_button.config(state="normal")
        gui.item_output_mode.set(queue_item.output_mode.value)
        gui.item_suffix.set(queue_item.output_suffix or gui.default_suffix.get())
        gui.item_output_folder.set(queue_item.output_folder or gui.default_output_folder.get())
        gui.item_source_label.config(text=queue_item.source_path)


# =============================================================================
# Property Changes
# =============================================================================


def on_item_output_mode_changed(gui) -> None:
    """Handle output mode change for selected item.

    Args:
        gui: The VideoConverterGUI instance.
    """
    selected = gui.queue_tree.selection()
    if not selected:
        return

    queue_item = gui.get_queue_item_for_tree_item(selected[0])
    if queue_item:
        queue_item.output_mode = OutputMode(gui.item_output_mode.get())
        gui.save_queue_to_config()
        gui.refresh_queue_tree()


def on_item_suffix_changed(gui) -> None:
    """Handle suffix change for selected item.

    Args:
        gui: The VideoConverterGUI instance.
    """
    selected = gui.queue_tree.selection()
    if not selected:
        return

    queue_item = gui.get_queue_item_for_tree_item(selected[0])
    if queue_item:
        queue_item.output_suffix = gui.item_suffix.get()
        gui.save_queue_to_config()
        gui.refresh_queue_tree()


def on_browse_item_output_folder(gui) -> None:
    """Browse for item-specific output folder.

    Args:
        gui: The VideoConverterGUI instance.
    """
    folder = filedialog.askdirectory(title="Select Output Folder")
    if not folder:
        return

    gui.item_output_folder.set(folder)

    # Update selected item
    selected = gui.queue_tree.selection()
    if not selected:
        return

    queue_item = gui.get_queue_item_for_tree_item(selected[0])
    if queue_item:
        queue_item.output_folder = folder
        gui.save_queue_to_config()
        gui.refresh_queue_tree()
