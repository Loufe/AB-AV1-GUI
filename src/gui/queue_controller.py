# src/gui/queue_controller.py
# ruff: noqa: SLF001  # This module accesses VideoConverterGUI internals by design
"""
Queue tab event handling and coordination.

Manages queue UI interactions including:
- Adding files/folders to queue
- Removing items from queue
- Queue selection and properties panel updates
- Item property changes (suffix, folder)
"""

from tkinter import filedialog, messagebox

from src.gui.analysis_tree import extract_paths_from_queue_items
from src.models import OperationType, QueueItemStatus

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

    Skips items that are currently converting (status == CONVERTING).

    Args:
        gui: The VideoConverterGUI instance.
    """
    # Get selected items
    selected = gui.queue_tree.selection()
    if not selected:
        return

    # Collect queue items to remove using O(1) lookups, skip CONVERTING items
    items_to_remove = []
    for tree_id in selected:
        item = gui.get_queue_item_for_tree_item(tree_id)
        if item and item.status != QueueItemStatus.CONVERTING:
            items_to_remove.append(item)

    if not items_to_remove:
        return

    # Extract paths before removal for incremental sync
    removed_paths = extract_paths_from_queue_items(items_to_remove)

    # Remove from queue
    for item in items_to_remove:
        gui._queue_items.remove(item)
        gui._queue_items_by_id.pop(item.id, None)

    gui.save_queue_to_config()
    gui.refresh_queue_tree()
    gui.sync_queue_tags_to_analysis_tree(removed_paths=removed_paths)


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
        # Extract paths before clearing for incremental sync
        removed_paths = extract_paths_from_queue_items(gui._queue_items)
        gui._queue_items.clear()
        gui._queue_items_by_id.clear()
        gui.save_queue_to_config()
        gui.refresh_queue_tree()
        gui.sync_queue_tags_to_analysis_tree(removed_paths=removed_paths)


def on_clear_completed(gui) -> None:
    """Clear completed, errored, and stopped items from queue (no confirmation).

    Args:
        gui: The VideoConverterGUI instance.
    """
    # Filter to keep only pending and converting items
    keep_statuses = {QueueItemStatus.PENDING, QueueItemStatus.CONVERTING}
    items_to_remove = [item for item in gui._queue_items if item.status not in keep_statuses]

    if items_to_remove:
        # Extract paths before removal for incremental sync
        removed_paths = extract_paths_from_queue_items(items_to_remove)
        gui._queue_items = [item for item in gui._queue_items if item.status in keep_statuses]
        gui._queue_items_by_id = {item.id: item for item in gui._queue_items}
        gui.save_queue_to_config()
        gui.refresh_queue_tree()
        gui.sync_queue_tags_to_analysis_tree(removed_paths=removed_paths)


def update_clear_completed_button_state(gui) -> None:
    """Update the clear completed button state based on queue contents.

    Enables the button if there are any completed/errored/stopped items.

    Args:
        gui: The VideoConverterGUI instance.
    """
    keep_statuses = {QueueItemStatus.PENDING, QueueItemStatus.CONVERTING}
    has_clearable = any(item.status not in keep_statuses for item in gui._queue_items)
    state = "normal" if has_clearable else "disabled"
    gui.clear_completed_button.config(state=state)


def update_remove_button_state(gui) -> None:
    """Update remove button state based on selection.

    Enables if any selected items are removable (not CONVERTING).

    Args:
        gui: The VideoConverterGUI instance.
    """
    selection = gui.queue_tree.selection()
    if not selection:
        gui.remove_queue_button.config(state="disabled")
        return

    # Check if any selected item is removable (not currently converting)
    for tree_id in selection:
        item = gui.get_queue_item_for_tree_item(tree_id)
        if item and item.status != QueueItemStatus.CONVERTING:
            gui.remove_queue_button.config(state="normal")
            return

    gui.remove_queue_button.config(state="disabled")


def update_start_button_state(gui) -> None:
    """Update start button state based on queue contents and running state.

    Disabled when conversion is running or no pending items exist.

    Args:
        gui: The VideoConverterGUI instance.
    """
    if gui.session.running:
        gui.start_button.config(state="disabled")
        return

    has_pending = any(item.status == QueueItemStatus.PENDING for item in gui._queue_items)
    state = "normal" if has_pending else "disabled"
    gui.start_button.config(state=state)


# =============================================================================
# Property Changes
# =============================================================================


def on_item_suffix_changed(gui) -> None:
    """Handle suffix change - applies to all queue items (bulk setting).

    Args:
        gui: The VideoConverterGUI instance.
    """
    new_suffix = gui.item_suffix.get()
    if not new_suffix:
        return

    # Update all queue items with the new suffix
    for queue_item in gui.get_queue_items():
        if queue_item.operation_type != OperationType.ANALYZE:
            queue_item.output_suffix = new_suffix

    gui.save_queue_to_config()
    gui.refresh_queue_tree()


def on_browse_item_output_folder(gui) -> None:
    """Browse for output folder - applies to all queue items (bulk setting).

    Args:
        gui: The VideoConverterGUI instance.
    """
    folder = filedialog.askdirectory(title="Select Output Folder")
    if not folder:
        return

    gui.item_output_folder.set(folder)

    # Update all queue items with the new folder
    for queue_item in gui.get_queue_items():
        if queue_item.operation_type != OperationType.ANALYZE:
            queue_item.output_folder = folder

    gui.save_queue_to_config()
    gui.refresh_queue_tree()
