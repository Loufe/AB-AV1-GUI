# src/gui/queue_tree.py
# ruff: noqa: SLF001  # This module accesses VideoConverterGUI internals by design
"""
Queue tree display and state management.

Provides incremental updates that modify rows in place, preserving folder
expand state, selection, and scroll position:
- refresh_queue_tree_values(): recompute values/tags for every row
- update_queue_item_row(): single item and its nested file rows
- add_queue_items_to_tree(): append-only insertion of new items
- remove_queue_items_from_tree(): targeted deletion with renumbering

refresh_queue_tree() remains the full delete-and-rebuild path for
structural changes (startup load, clear operations, conflict replacement)
and restores folder expand state across the rebuild.
"""

import contextlib
import logging
import os

from src.estimation import compute_grouped_percentiles, estimate_fresh_file_time
from src.gui.queue_controller import (
    update_clear_completed_button_state,
    update_remove_button_state,
    update_start_button_state,
)
from src.gui.tree_display import format_queue_file_status, format_queue_status_display, format_stream_display
from src.gui.tree_formatters import format_compact_time
from src.history_index import compute_path_hash, get_history_index
from src.models import OperationType, OutputMode, QueueItemStatus, TimeEstimate
from src.utils import format_file_size

logger = logging.getLogger(__name__)


def refresh_queue_tree(gui) -> None:
    """Rebuild the queue tree from _queue_items.

    Performs a full delete-and-rebuild, restoring folder expand state
    (keyed by queue item id) afterwards. Selection and scroll position are
    not preserved - use the incremental update functions for changes that
    don't restructure the tree.

    Args:
        gui: The VideoConverterGUI instance.
    """
    # Capture expand state before deleting rows (keyed by stable queue item id)
    expanded_ids = {
        queue_id
        for queue_id, tree_id in gui._queue_tree_map.items()
        if gui.queue_tree.exists(tree_id) and _is_expanded(gui.queue_tree, tree_id)
    }

    # Clear existing items
    for item in gui.queue_tree.get_children():
        gui.queue_tree.delete(item)
    gui._queue_tree_map.clear()
    gui._tree_queue_map.clear()
    gui._queue_file_tree_map.clear()
    gui._tree_file_map.clear()
    gui._queue_items_by_id = {item.id: item for item in gui._queue_items}

    stopping, index, percentiles_by_op, file_estimates = _display_context(gui)

    # Add each queue item with order number
    for order, queue_item in enumerate(gui._queue_items, start=1):
        values, item_tag = _build_item_values(gui, queue_item, stopping, index, percentiles_by_op, file_estimates)
        expanded = queue_item.id in expanded_ids and bool(queue_item.is_folder and queue_item.files)

        item_id = gui.queue_tree.insert(
            "",
            "end",
            text=_item_text(queue_item, order, expanded=expanded),
            values=values,
            tags=(item_tag,) if item_tag else (),
            open=expanded,
        )
        gui._queue_tree_map[queue_item.id] = item_id
        gui._tree_queue_map[item_id] = queue_item.id

        # Insert nested file rows for folder items
        if queue_item.is_folder and queue_item.files:
            _insert_folder_file_rows(gui, queue_item, item_id, stopping, file_estimates)

    _update_total_row(gui, index, percentiles_by_op, file_estimates)
    _update_button_states(gui)


def refresh_queue_tree_values(gui) -> None:
    """Recompute display values and tags for every row in place.

    Assumes the set and order of queue items is unchanged since the last
    structural update. Updates all columns (status, estimates, operation,
    output) without deleting rows, so expand state, selection, and scroll
    position survive. Falls back to a full rebuild if the tree has drifted
    from _queue_items.

    Args:
        gui: The VideoConverterGUI instance.
    """
    stopping, index, percentiles_by_op, file_estimates = _display_context(gui)

    for queue_item in gui._queue_items:
        tree_id = gui._queue_tree_map.get(queue_item.id)
        if not tree_id or not gui.queue_tree.exists(tree_id):
            logger.debug("Queue tree drifted from _queue_items; falling back to full rebuild")
            refresh_queue_tree(gui)
            return

        values, item_tag = _build_item_values(gui, queue_item, stopping, index, percentiles_by_op, file_estimates)
        gui.queue_tree.item(tree_id, values=values, tags=(item_tag,) if item_tag else ())

        if queue_item.is_folder and queue_item.files:
            _update_folder_file_rows(gui, queue_item, stopping, index, file_estimates)

    _update_total_row(gui, index, percentiles_by_op, file_estimates)
    _update_button_states(gui)


def update_queue_item_row(gui, queue_item) -> None:
    """Update a single queue item's row and nested file rows in place.

    Recomputes all columns for the item (e.g., after an operation type
    change) and refreshes the total row, since operation changes affect the
    queue-wide time estimate. Falls back to a full rebuild if the item has
    no tree row.

    Args:
        gui: The VideoConverterGUI instance.
        queue_item: The QueueItem whose row should be updated.
    """
    tree_id = gui._queue_tree_map.get(queue_item.id)
    if not tree_id or not gui.queue_tree.exists(tree_id):
        logger.debug("No tree row for queue item %s; falling back to full rebuild", queue_item.id)
        refresh_queue_tree(gui)
        return

    stopping, index, percentiles_by_op, file_estimates = _display_context(gui)

    values, item_tag = _build_item_values(gui, queue_item, stopping, index, percentiles_by_op, file_estimates)
    gui.queue_tree.item(tree_id, values=values, tags=(item_tag,) if item_tag else ())

    if queue_item.is_folder and queue_item.files:
        _update_folder_file_rows(gui, queue_item, stopping, index, file_estimates)

    _update_total_row(gui, index, percentiles_by_op, file_estimates)
    _update_button_states(gui)


def add_queue_items_to_tree(gui, new_items) -> None:
    """Append rows for newly added queue items without touching existing rows.

    The new items must already be appended to _queue_items (in order, at the
    tail). Falls back to a full rebuild if positions don't line up (e.g.,
    an item was replaced mid-list).

    Args:
        gui: The VideoConverterGUI instance.
        new_items: List of QueueItem objects that were appended to the queue.
    """
    if not new_items:
        return

    # Appending is only valid if the new items occupy the tail of _queue_items
    # and every existing item already has a row
    tail_ids = [item.id for item in gui._queue_items[-len(new_items):]]
    existing_count = len(gui._queue_items) - len(new_items)
    if tail_ids != [item.id for item in new_items] or existing_count != len(gui.queue_tree.get_children()):
        logger.debug("New queue items are not a clean tail append; falling back to full rebuild")
        refresh_queue_tree(gui)
        return

    stopping, index, percentiles_by_op, file_estimates = _display_context(gui)

    order = existing_count
    for queue_item in new_items:
        order += 1
        gui._queue_items_by_id[queue_item.id] = queue_item

        values, item_tag = _build_item_values(gui, queue_item, stopping, index, percentiles_by_op, file_estimates)
        item_id = gui.queue_tree.insert(
            "",
            "end",
            text=_item_text(queue_item, order, expanded=False),
            values=values,
            tags=(item_tag,) if item_tag else (),
        )
        gui._queue_tree_map[queue_item.id] = item_id
        gui._tree_queue_map[item_id] = queue_item.id

        if queue_item.is_folder and queue_item.files:
            _insert_folder_file_rows(gui, queue_item, item_id, stopping, file_estimates)

    _update_total_row(gui, index, percentiles_by_op, file_estimates)
    _update_button_states(gui)


def remove_queue_items_from_tree(gui, removed_items) -> None:
    """Remove specific queue item rows, renumber the rest, and refresh totals.

    The items must already be removed from _queue_items. Other rows are left
    untouched apart from their order-number prefix, so expand state,
    selection, and scroll position survive.

    Args:
        gui: The VideoConverterGUI instance.
        removed_items: List of QueueItem objects removed from the queue.
    """
    for queue_item in removed_items:
        gui._queue_items_by_id.pop(queue_item.id, None)
        tree_id = gui._queue_tree_map.pop(queue_item.id, None)
        if tree_id:
            gui._tree_queue_map.pop(tree_id, None)
            if gui.queue_tree.exists(tree_id):
                gui.queue_tree.delete(tree_id)  # Nested file rows are deleted with the parent
        for file_item in queue_item.files:
            file_tree_id = gui._queue_file_tree_map.pop(file_item.path, None)
            if file_tree_id:
                gui._tree_file_map.pop(file_tree_id, None)

    if len(gui.queue_tree.get_children()) != len(gui._queue_items):
        logger.debug("Queue tree drifted from _queue_items after removal; falling back to full rebuild")
        refresh_queue_tree(gui)
        return

    _renumber_queue_rows(gui)

    _stopping, index, percentiles_by_op, file_estimates = _display_context(gui)
    _update_total_row(gui, index, percentiles_by_op, file_estimates)
    _update_button_states(gui)


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

    if len(new_order) != len(gui._queue_items):
        return  # Length mismatch - something wrong, bail out

    # Early exit if order unchanged (identity comparison)
    if all(a is b for a, b in zip(new_order, gui._queue_items, strict=True)):
        return

    gui._queue_items = new_order
    gui.save_queue_to_config()
    # Rows already sit in the new order after the drag; only the number prefixes are stale
    _renumber_queue_rows(gui)


# =============================================================================
# Private Helper Functions
# =============================================================================


def _is_expanded(tree, tree_id: str) -> bool:
    """Check a row's open state (Tk may return int, bool, or str)."""
    return str(tree.item(tree_id, "open")) in ("1", "true", "True")


def _item_text(queue_item, order: int, *, expanded: bool) -> str:
    """Build the tree-column text for a top-level queue item row."""
    icon = "📁" if queue_item.is_folder else "🎬"
    # Only show expand arrow if folder has files to display
    arrow = ""
    if queue_item.is_folder and queue_item.files:
        arrow = "▼ " if expanded else "▶ "
    name = os.path.basename(queue_item.source_path)
    return f"{order}. {arrow}{icon} {name}"


def _renumber_queue_rows(gui) -> None:
    """Rewrite the order-number prefix of every top-level row in place."""
    for order, tree_id in enumerate(gui.queue_tree.get_children(), start=1):
        queue_id = gui._tree_queue_map.get(tree_id)
        queue_item = gui._queue_items_by_id.get(queue_id) if queue_id else None
        if queue_item is None:
            continue
        expanded = _is_expanded(gui.queue_tree, tree_id)
        gui.queue_tree.item(tree_id, text=_item_text(queue_item, order, expanded=expanded))


def _display_context(gui) -> tuple[bool, object, dict, dict[str, TimeEstimate]]:
    """Compute shared display inputs for row building.

    Returns:
        Tuple of (stopping, history index, percentiles by operation type,
        pre-computed per-file time estimates).
    """
    # Check if stop has been requested (pending items won't run)
    stopping = gui.session.running and gui.stop_event is not None and gui.stop_event.is_set()

    index = get_history_index()

    # Pre-compute percentiles once for all estimates (see TIME_ESTIMATION.md ## Performance)
    percentiles_by_op = {
        OperationType.CONVERT: compute_grouped_percentiles(OperationType.CONVERT),
        OperationType.ANALYZE: compute_grouped_percentiles(OperationType.ANALYZE),
    }

    # Pre-compute time estimates for all nested files (avoids 3x calls per file)
    file_estimates: dict[str, TimeEstimate] = {}
    for queue_item in gui._queue_items:
        if queue_item.is_folder and queue_item.files:
            op_type = queue_item.operation_type
            percentiles = percentiles_by_op.get(op_type)
            for file_item in queue_item.files:
                if file_item.path not in file_estimates:
                    file_estimates[file_item.path] = estimate_fresh_file_time(
                        file_item.path, operation_type=op_type, grouped_percentiles=percentiles
                    )

    return stopping, index, percentiles_by_op, file_estimates


def _build_item_values(
    gui, queue_item, stopping: bool, index, percentiles_by_op: dict, file_estimates: dict[str, TimeEstimate]
) -> tuple[tuple, str]:
    """Compute the values tuple and status tag for a top-level queue item row."""
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
    est_time_display = _format_time_estimate(queue_item, percentiles_by_op, file_estimates)

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

    values = (format_display, size_display, est_time_display, operation_display, output_display, status_display)
    return values, item_tag


def _build_file_values(
    file_item, stopping: bool, index, file_estimates: dict[str, TimeEstimate]
) -> tuple[tuple, str]:
    """Compute the values tuple and status tag for a nested file row."""
    file_size = format_file_size(file_item.size_bytes) if file_item.size_bytes > 0 else "—"

    # Look up file record for format display
    file_record = index.lookup_file(file_item.path)
    if file_record and file_record.video_codec:
        file_format = format_stream_display(file_record.video_codec, file_record.audio_streams)
    else:
        file_format = "—"

    # Use pre-computed estimate from cache
    file_time_estimate = file_estimates.get(file_item.path)
    if file_time_estimate and file_time_estimate.confidence != "none" and file_time_estimate.best_seconds > 0:
        file_est_time = format_compact_time(file_time_estimate.best_seconds, confidence=file_time_estimate.confidence)
    else:
        file_est_time = "—"

    # Get status display and tag using shared function
    file_status, file_tag = format_queue_file_status(
        file_item.status,
        stopping=stopping,
        error_message=file_item.error_message,
        skip_reason=file_item.skip_reason,
    )

    return (file_format, file_size, file_est_time, "", "", file_status), file_tag


def _update_folder_file_rows(
    gui, queue_item, stopping: bool, index, file_estimates: dict[str, TimeEstimate]
) -> None:
    """Update nested file rows of a folder item in place (missing rows are skipped)."""
    for file_item in queue_item.files:
        file_tree_id = gui._queue_file_tree_map.get(file_item.path)
        if not file_tree_id or not gui.queue_tree.exists(file_tree_id):
            continue
        values, file_tag = _build_file_values(file_item, stopping, index, file_estimates)
        gui.queue_tree.item(file_tree_id, values=values, tags=(file_tag,))


def _update_button_states(gui) -> None:
    """Update queue-related button states after a tree change."""
    update_clear_completed_button_state(gui)
    update_remove_button_state(gui)
    update_start_button_state(gui)


def _format_format_display(queue_item, record) -> str:
    """Format the format column display text (codec/stream info)."""
    if queue_item.is_folder:
        return ""  # Folders don't show format
    if not record or not record.video_codec:
        return "—"
    return format_stream_display(record.video_codec, record.audio_streams)


def _format_operation_display(queue_item, record) -> str:
    """Format the operation column display text."""
    if queue_item.operation_type == OperationType.ANALYZE:
        return "Analyze"
    if queue_item.operation_type == OperationType.CONVERT:
        # For folders, check if ALL files have Layer 2 data
        if queue_item.is_folder and queue_item.files:
            index = get_history_index()
            for file_item in queue_item.files:
                file_record = index.lookup_file(file_item.path)
                if not (file_record and file_record.best_crf is not None
                        and file_record.best_vmaf_achieved is not None):
                    return "Analyze+Convert"
            return "Convert"
        # Single file: check if it has Layer 2 data (CRF search results)
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


def _format_time_estimate(queue_item, percentiles_by_op: dict, file_estimates: dict[str, TimeEstimate]) -> str:
    """Format the estimated time column display text."""
    op_type = queue_item.operation_type
    percentiles = percentiles_by_op.get(op_type)
    if queue_item.is_folder and queue_item.files:
        # Sum estimates for all files in folder, track lowest confidence
        total_seconds = 0.0
        lowest_confidence = "high"
        confidence_order = {"high": 0, "medium": 1, "low": 2, "none": 3}
        for file_item in queue_item.files:
            # Use pre-computed estimate from cache
            file_estimate = file_estimates.get(file_item.path)
            if file_estimate is None:
                file_estimate = estimate_fresh_file_time(
                    file_item.path, operation_type=op_type, grouped_percentiles=percentiles
                )
            if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                total_seconds += file_estimate.best_seconds
                if confidence_order.get(file_estimate.confidence, 3) > confidence_order.get(lowest_confidence, 0):
                    lowest_confidence = file_estimate.confidence
        if total_seconds > 0:
            return format_compact_time(total_seconds, confidence=lowest_confidence)
        return "—"
    if not queue_item.is_folder:
        file_estimate = estimate_fresh_file_time(
            queue_item.source_path, operation_type=op_type, grouped_percentiles=percentiles
        )
        if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
            return format_compact_time(file_estimate.best_seconds, confidence=file_estimate.confidence)
        return "—"
    # Folder without files populated - can't estimate
    return "—"


def _insert_folder_file_rows(
    gui, queue_item, parent_item_id: str, stopping: bool, file_estimates: dict[str, TimeEstimate]
) -> None:
    """Insert nested file rows for a folder queue item."""
    index = get_history_index()

    for file_item in queue_item.files:
        values, file_tag = _build_file_values(file_item, stopping, index, file_estimates)
        file_tree_id = gui.queue_tree.insert(
            parent_item_id,
            "end",
            text=f"    🎬 {os.path.basename(file_item.path)}",
            values=values,
            tags=(file_tag,),
        )
        gui._queue_file_tree_map[file_item.path] = file_tree_id
        gui._tree_file_map[file_tree_id] = (queue_item.id, file_item.path)


def _update_total_row(
    gui, index, percentiles_by_op: dict, file_estimates: dict[str, TimeEstimate]
) -> None:
    """Update the total row at the bottom of the queue tree."""
    total_items = len(gui._queue_items)
    total_files = sum(len(item.files) if item.is_folder else 1 for item in gui._queue_items)

    # Calculate total size and estimated time
    total_size_bytes = 0
    total_est_seconds = 0.0
    lowest_confidence = "high"
    confidence_order = {"high": 0, "medium": 1, "low": 2, "none": 3}

    for queue_item in gui._queue_items:
        op_type = queue_item.operation_type
        percentiles = percentiles_by_op.get(op_type)
        if queue_item.is_folder and queue_item.files:
            # Folder: sum file sizes and use pre-computed estimates
            for file_item in queue_item.files:
                if file_item.size_bytes > 0:
                    total_size_bytes += file_item.size_bytes
                # Use pre-computed estimate from cache
                file_estimate = file_estimates.get(file_item.path)
                if file_estimate is None:
                    file_estimate = estimate_fresh_file_time(
                        file_item.path, operation_type=op_type, grouped_percentiles=percentiles
                    )
                if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                    total_est_seconds += file_estimate.best_seconds
                    if confidence_order.get(file_estimate.confidence, 3) > confidence_order.get(lowest_confidence, 0):
                        lowest_confidence = file_estimate.confidence
        elif not queue_item.is_folder:
            # Single file: get size from record or filesystem
            path_hash = compute_path_hash(queue_item.source_path)
            record = index.get(path_hash)
            if record and record.file_size_bytes:
                total_size_bytes += record.file_size_bytes
            elif os.path.isfile(queue_item.source_path):
                with contextlib.suppress(OSError):
                    total_size_bytes += os.path.getsize(queue_item.source_path)

            # Time estimate
            file_estimate = estimate_fresh_file_time(
                queue_item.source_path, operation_type=op_type, grouped_percentiles=percentiles
            )
            if file_estimate.confidence != "none" and file_estimate.best_seconds > 0:
                total_est_seconds += file_estimate.best_seconds
                if confidence_order.get(file_estimate.confidence, 3) > confidence_order.get(lowest_confidence, 0):
                    lowest_confidence = file_estimate.confidence
        # else: folder without files populated - skip
    # Format displays
    total_size_display = format_file_size(total_size_bytes) if total_size_bytes > 0 else "—"
    if total_est_seconds > 0:
        total_est_time_display = format_compact_time(total_est_seconds, confidence=lowest_confidence)
    else:
        total_est_time_display = "—"
    status_text = f"{total_items} items ({total_files} files)" if total_files != total_items else f"{total_items} items"

    gui.queue_total_tree.item("total", values=("", total_size_display, total_est_time_display, "", "", status_text))
