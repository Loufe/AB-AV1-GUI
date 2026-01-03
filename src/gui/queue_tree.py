# src/gui/queue_tree.py
# ruff: noqa: SLF001  # This module accesses VideoConverterGUI internals by design
"""
Queue tree display and state management.

Handles full rebuilds of the queue tree view, including:
- Queue item display with status, size, time estimates
- Nested file display for folder items
- Total row updates
- Drag-drop reordering synchronization
"""

import os

from src.estimation import estimate_file_time
from src.gui.tree_display import format_queue_file_status, format_queue_status_display, format_stream_display
from src.gui.tree_formatters import format_compact_time
from src.history_index import compute_path_hash, get_history_index
from src.models import OperationType, OutputMode, QueueItemStatus
from src.utils import format_file_size


def refresh_queue_tree(gui) -> None:
    """Refresh the queue tree view from _queue_items.

    Performs a full rebuild of the queue tree, updating all items
    with current status, size, time estimates, and nested files.

    Args:
        gui: The VideoConverterGUI instance.
    """
    # Clear existing items
    for item in gui.queue_tree.get_children():
        gui.queue_tree.delete(item)
    gui._queue_tree_map.clear()
    gui._tree_queue_map.clear()
    gui._queue_file_tree_map.clear()
    gui._tree_file_map.clear()
    gui._queue_items_by_id = {item.id: item for item in gui._queue_items}

    # Check if stop has been requested (pending items won't run)
    stopping = gui.session.running and gui.stop_event is not None and gui.stop_event.is_set()

    index = get_history_index()

    # Add each queue item with order number
    for order, queue_item in enumerate(gui._queue_items, start=1):
        # Get history record for metadata
        path_hash = compute_path_hash(queue_item.source_path)
        record = index.get(path_hash)

        # Determine operation display based on operation type and Layer 2 data
        operation_display = _format_operation_display(queue_item, record)

        # Format output mode display
        output_display = _format_output_display(queue_item, gui)

        # Format codec/stream info
        format_display = _format_format_display(queue_item, record)

        # Format size - for folders, sum file sizes
        size_display = _format_size_display(queue_item, record)

        # Format estimated time
        est_time_display = _format_time_estimate(queue_item, record, index)

        # Format status with tag
        status_display, item_tag = format_queue_status_display(
            queue_item.status,
            stopping=stopping,
            total_files=queue_item.total_files,
            processed_files=queue_item.processed_files,
        )
        # Use the item's own format_status_display for non-stopping cases
        if not stopping or queue_item.status != QueueItemStatus.PENDING:
            if queue_item.status == QueueItemStatus.CONVERTING and queue_item.total_files > 0:
                status_display = f"Converting ({queue_item.processed_files}/{queue_item.total_files})"
            else:
                status_display = queue_item.format_status_display()

        # Insert item with order number prefix
        icon = "📁" if queue_item.is_folder else "🎬"
        # Only show expand arrow if folder has files to display
        prefix = "▶ " if queue_item.is_folder and queue_item.files else ""
        name = os.path.basename(queue_item.source_path)

        item_id = gui.queue_tree.insert(
            "",
            "end",
            text=f"{order}. {prefix}{icon} {name}",
            values=(format_display, size_display, est_time_display, operation_display, output_display, status_display),
            tags=(item_tag,) if item_tag else (),
        )
        gui._queue_tree_map[queue_item.id] = item_id
        gui._tree_queue_map[item_id] = queue_item.id

        # Insert nested file rows for folder items
        if queue_item.is_folder and queue_item.files:
            _insert_folder_file_rows(gui, queue_item, item_id, stopping)

    # Update total row
    _update_total_row(gui, index)


def sync_queue_order_from_tree(gui) -> None:
    """Sync _queue_items order from the tree view after drag-drop reordering.

    Args:
        gui: The VideoConverterGUI instance.
    """
    # Rebuild _queue_items in tree order using O(1) lookups
    new_order = []
    for tree_id in gui.queue_tree.get_children():
        queue_id = gui._tree_queue_map.get(tree_id)
        if queue_id:
            item = gui._queue_items_by_id.get(queue_id)
            if item:
                new_order.append(item)

    if len(new_order) == len(gui._queue_items):
        gui._queue_items = new_order
        gui.save_queue_to_config()
        # Refresh to update order numbers
        refresh_queue_tree(gui)


# =============================================================================
# Private Helper Functions
# =============================================================================


def _format_format_display(queue_item, record) -> str:
    """Format the format column display text (codec/stream info)."""
    if queue_item.is_folder:
        return ""  # Folders don't show format
    if not record or not record.video_codec:
        return "—"
    return format_stream_display(record.video_codec, record.audio_codec)


def _format_operation_display(queue_item, record) -> str:
    """Format the operation column display text."""
    if queue_item.operation_type == OperationType.ANALYZE:
        return "Analyze"
    if queue_item.operation_type == OperationType.CONVERT:
        # Check if file has Layer 2 data (CRF search results)
        has_layer2 = record and record.best_crf is not None and record.best_vmaf_achieved is not None
        return "Convert" if has_layer2 else "Analyze+Convert"
    return "Unknown"


def _format_output_display(queue_item, gui) -> str:
    """Format the output column display text."""
    if queue_item.operation_type == OperationType.ANALYZE:
        return "—"
    if queue_item.output_mode == OutputMode.REPLACE:
        return "Replace"
    if queue_item.output_mode == OutputMode.SUFFIX:
        suffix = queue_item.output_suffix or gui.default_suffix.get()
        return f"{suffix}"
    folder_name = os.path.basename(queue_item.output_folder or gui.default_output_folder.get() or "")
    return f"→ {folder_name}/" if folder_name else "Separate"


def _format_size_display(queue_item, record) -> str:
    """Format the size column display text."""
    if queue_item.is_folder and queue_item.files:
        total_size = sum(f.size_bytes for f in queue_item.files)
        return format_file_size(total_size) if total_size > 0 else "—"
    if record and record.file_size_bytes:
        return format_file_size(record.file_size_bytes)
    if os.path.isfile(queue_item.source_path):
        try:
            return format_file_size(os.path.getsize(queue_item.source_path))
        except OSError:
            return "—"
    else:
        return "—"


def _format_time_estimate(queue_item, record, index) -> str:
    """Format the estimated time column display text."""
    op_type = queue_item.operation_type
    if queue_item.is_folder and queue_item.files:
        # Sum estimates for all files in folder, track lowest confidence
        total_seconds = 0.0
        lowest_confidence = "high"
        confidence_order = {"high": 0, "medium": 1, "low": 2, "none": 3}
        for file_item in queue_item.files:
            file_estimate = estimate_file_time(file_item.path, operation_type=op_type)
            if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                total_seconds += file_estimate.best_seconds
                if confidence_order.get(file_estimate.confidence, 3) > confidence_order.get(lowest_confidence, 0):
                    lowest_confidence = file_estimate.confidence
        if total_seconds > 0:
            return format_compact_time(total_seconds, confidence=lowest_confidence)
        return "—"
    if record:
        # Use record data for single file estimate
        file_estimate = estimate_file_time(
            codec=record.video_codec,
            duration=record.duration_sec,
            width=record.width,
            height=record.height,
            operation_type=op_type,
        )
        if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
            return format_compact_time(file_estimate.best_seconds, confidence=file_estimate.confidence)
        return "—"
    if not queue_item.is_folder:
        # Try path-based estimate as fallback (only for files, not folders)
        file_estimate = estimate_file_time(queue_item.source_path, operation_type=op_type)
        if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
            return format_compact_time(file_estimate.best_seconds, confidence=file_estimate.confidence)
        return "—"
    # Folder without files populated - can't estimate
    return "—"


def _insert_folder_file_rows(gui, queue_item, parent_item_id: str, stopping: bool) -> None:
    """Insert nested file rows for a folder queue item."""
    op_type = queue_item.operation_type
    for file_item in queue_item.files:
        file_name = os.path.basename(file_item.path)
        file_size = format_file_size(file_item.size_bytes) if file_item.size_bytes > 0 else "—"

        # Calculate estimated time for this file
        file_time_estimate = estimate_file_time(file_item.path, operation_type=op_type)
        if file_time_estimate.confidence != "none" and file_time_estimate.best_seconds > 0:
            file_est_time = format_compact_time(
                file_time_estimate.best_seconds, confidence=file_time_estimate.confidence
            )
        else:
            file_est_time = "—"

        # Get status display and tag using shared function
        file_status, file_tag = format_queue_file_status(
            file_item.status,
            stopping=stopping,
            error_message=file_item.error_message,
        )

        file_tree_id = gui.queue_tree.insert(
            parent_item_id,
            "end",
            text=f"    🎬 {file_name}",
            values=("", file_size, file_est_time, "", "", file_status),
            tags=(file_tag,),
        )
        gui._queue_file_tree_map[file_item.path] = file_tree_id
        gui._tree_file_map[file_tree_id] = (queue_item.id, file_item.path)


def _update_total_row(gui, index) -> None:
    """Update the total row at the bottom of the queue tree."""
    total_items = len(gui._queue_items)
    total_files = sum(len(item.files) if item.is_folder else 1 for item in gui._queue_items)

    # Calculate total estimated time, tracking lowest confidence
    total_est_seconds = 0.0
    lowest_confidence = "high"
    confidence_order = {"high": 0, "medium": 1, "low": 2, "none": 3}

    for queue_item in gui._queue_items:
        op_type = queue_item.operation_type
        if queue_item.is_folder and queue_item.files:
            for file_item in queue_item.files:
                file_estimate = estimate_file_time(file_item.path, operation_type=op_type)
                if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                    total_est_seconds += file_estimate.best_seconds
                    if confidence_order.get(file_estimate.confidence, 3) > confidence_order.get(lowest_confidence, 0):
                        lowest_confidence = file_estimate.confidence
        elif not queue_item.is_folder:
            # Single file item
            path_hash = compute_path_hash(queue_item.source_path)
            record = index.get(path_hash)
            if record:
                file_estimate = estimate_file_time(
                    codec=record.video_codec,
                    duration=record.duration_sec,
                    width=record.width,
                    height=record.height,
                    operation_type=op_type,
                )
            else:
                file_estimate = estimate_file_time(queue_item.source_path, operation_type=op_type)
            if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                total_est_seconds += file_estimate.best_seconds
                if confidence_order.get(file_estimate.confidence, 3) > confidence_order.get(lowest_confidence, 0):
                    lowest_confidence = file_estimate.confidence
        # else: folder without files populated - skip estimation

    if total_est_seconds > 0:
        total_est_time_display = format_compact_time(total_est_seconds, confidence=lowest_confidence)
    else:
        total_est_time_display = "—"

    if total_files != total_items:
        status_text = f"{total_items} items ({total_files} files)"
        gui.queue_total_tree.item("total", values=("", "", total_est_time_display, "", "", status_text))
    else:
        gui.queue_total_tree.item("total", values=("", "", total_est_time_display, "", "", f"{total_items} items"))
