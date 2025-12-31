# src/gui/queue_manager.py
"""
Queue management functions extracted from main_window.py.

These functions handle queue item creation, categorization, and addition logic.
"""

import os
import uuid

from src.conversion_engine.scanner import find_video_files
from src.estimation import estimate_file_time
from src.gui.widgets.add_to_queue_dialog import AddToQueuePreviewDialog, QueuePreviewData
from src.history_index import compute_path_hash, get_history_index
from src.models import OperationType, OutputMode, QueueFileItem, QueueItem, QueueItemStatus


def load_queue_from_config(gui) -> list[QueueItem]:
    """Load queue items from config, filtering out completed/invalid entries."""
    raw_items = gui.config.get("queue_items", [])
    items = []
    for data in raw_items:
        try:
            item = QueueItem.from_dict(data)
            # Reset interrupted items (CONVERTING, STOPPED) to PENDING for retry
            if item.status in (QueueItemStatus.CONVERTING, QueueItemStatus.STOPPED):
                item.status = QueueItemStatus.PENDING
                # Reset outcome counters for retry
                item.files_succeeded = 0
                item.files_skipped = 0
                item.files_failed = 0
                item.last_error = None
            # Only restore PENDING items if file exists (skip completed/error items)
            if item.status == QueueItemStatus.PENDING and os.path.exists(item.source_path):
                items.append(item)
        except (KeyError, ValueError):
            continue  # Skip invalid entries
    return items


def find_existing_queue_item(gui, path: str) -> QueueItem | None:
    """Find an existing queue item by path."""
    for item in gui.get_queue_items():
        if item.source_path == path:
            return item
    return None


def get_selected_extensions(gui) -> list[str]:
    """Get list of selected file extensions."""
    extensions = []
    if gui.ext_mp4.get():
        extensions.append("mp4")
    if gui.ext_mkv.get():
        extensions.append("mkv")
    if gui.ext_avi.get():
        extensions.append("avi")
    if gui.ext_wmv.get():
        extensions.append("wmv")
    return extensions


def create_queue_item(gui, path: str, is_folder: bool, operation_type: OperationType) -> QueueItem:
    """Create a new QueueItem with default settings.

    For folder items, populates the files list by scanning for video files.
    """
    default_mode = gui.default_output_mode.get()

    # For folders, scan and populate the files list
    files: list[QueueFileItem] = []
    if is_folder:
        extensions = get_selected_extensions(gui)
        if extensions:
            file_paths = find_video_files(path, extensions)
            files = [
                QueueFileItem(path=fp, size_bytes=os.path.getsize(fp) if os.path.isfile(fp) else 0)
                for fp in file_paths
            ]

    return QueueItem(
        id=str(uuid.uuid4()),
        source_path=path,
        is_folder=is_folder,
        output_mode=OutputMode(default_mode),
        output_suffix=gui.default_suffix.get() if default_mode == "suffix" else None,
        output_folder=gui.default_output_folder.get() if default_mode == "separate_folder" else None,
        operation_type=operation_type,
        files=files,
        total_files=len(files) if is_folder else 1,
    )


def categorize_queue_items(
    gui, items: list[tuple[str, bool]], operation_type: OperationType
) -> tuple[list[tuple[str, bool]], list[str], list[tuple[str, bool, QueueItem]]]:
    """Categorize items for queue preview.

    Returns:
        Tuple of (to_add, duplicates, conflicts) where:
        - to_add: Items that can be added directly
        - duplicates: Paths already in queue with same operation
        - conflicts: (path, is_folder, existing_item) for different operation
    """
    to_add: list[tuple[str, bool]] = []
    duplicates: list[str] = []
    conflicts: list[tuple[str, bool, QueueItem]] = []

    for path, is_folder in items:
        existing = find_existing_queue_item(gui, path)
        if not existing:
            to_add.append((path, is_folder))
        elif existing.operation_type == operation_type:
            duplicates.append(path)
        else:
            conflicts.append((path, is_folder, existing))

    return to_add, duplicates, conflicts


def calculate_queue_estimates(gui, items: list[tuple[str, bool]]) -> tuple[float | None, float | None]:
    """Calculate time estimate and potential savings for items.

    Returns:
        Tuple of (estimated_time_seconds, estimated_savings_percent)
    """
    total_time = 0.0
    total_original_size = 0
    total_saved_bytes = 0.0
    has_time_estimates = False

    index = get_history_index()

    for path, is_folder in items:
        if is_folder:
            continue  # Skip folders for now, would need to scan

        # Try to get time estimate
        estimate = estimate_file_time(path)
        if estimate.confidence != "none":
            total_time += estimate.best_seconds
            has_time_estimates = True

        # Try to get savings estimate from history
        path_hash = compute_path_hash(path)
        record = index.get(path_hash)
        if record and record.file_size_bytes:
            # Use Layer 2 data if available, fall back to Layer 1
            reduction_pct = record.predicted_size_reduction or record.estimated_reduction_percent
            if reduction_pct:
                total_original_size += record.file_size_bytes
                total_saved_bytes += record.file_size_bytes * (reduction_pct / 100)

    # Calculate overall reduction percentage (weighted average)
    total_savings = None
    if total_original_size > 0 and total_saved_bytes > 0:
        total_savings = (total_saved_bytes / total_original_size) * 100

    return (total_time if has_time_estimates else None, total_savings)


def add_items_to_queue(
    gui, items: list[tuple[str, bool]], operation_type: OperationType, force_preview: bool = False
) -> dict[str, int]:
    """Add items to queue with appropriate UI feedback.

    This is the main entry point for all queue additions.

    Args:
        items: List of (path, is_folder) tuples
        operation_type: OperationType.CONVERT or OperationType.ANALYZE
        force_preview: If True, always show preview dialog (for "Add All")
                      If False, only show dialog if there are conflicts

    Returns:
        Dict with counts: {"added", "duplicate", "conflict_added", "conflict_replaced", "cancelled"}
    """
    counts = {"added": 0, "duplicate": 0, "conflict_added": 0, "conflict_replaced": 0, "cancelled": 0}

    if not items:
        return counts

    # Categorize items
    to_add, duplicates, conflicts = categorize_queue_items(gui, items, operation_type)
    counts["duplicate"] = len(duplicates)

    # Determine if we need to show the preview dialog
    show_dialog = force_preview or len(conflicts) > 0

    if show_dialog:
        # Calculate estimates for preview
        estimated_time, estimated_savings = calculate_queue_estimates(gui, to_add)

        # Build preview data
        preview_data = QueuePreviewData(
            to_add=to_add,
            duplicates=duplicates,
            conflicts=conflicts,
            operation_type=operation_type,
            estimated_time_sec=estimated_time,
            estimated_savings_percent=estimated_savings,
        )

        # Show dialog
        dialog = AddToQueuePreviewDialog(gui.root, preview_data)
        result = dialog.result

        if result["action"] == "cancel":
            counts["cancelled"] = len(to_add) + len(conflicts)
            return counts

        conflict_resolution = result["conflict_resolution"]
    else:
        # No dialog needed, just add
        conflict_resolution = "skip"

    # Add the non-conflicting items
    queue_items = gui.get_queue_items()
    for path, is_folder in to_add:
        item = create_queue_item(gui, path, is_folder, operation_type)
        queue_items.append(item)
        counts["added"] += 1

    # Handle conflicts based on resolution choice
    if conflicts:
        if conflict_resolution == "keep_both":
            for path, is_folder, _ in conflicts:
                item = create_queue_item(gui, path, is_folder, operation_type)
                queue_items.append(item)
                counts["conflict_added"] += 1
        elif conflict_resolution == "replace":
            for path, is_folder, existing in conflicts:
                new_item = create_queue_item(gui, path, is_folder, operation_type)
                idx = queue_items.index(existing)
                queue_items[idx] = new_item
                counts["conflict_replaced"] += 1
        # else: skip - conflicts are not added

    # Save and refresh if anything was added or modified
    if counts["added"] > 0 or counts["conflict_added"] > 0 or counts["conflict_replaced"] > 0:
        gui.save_queue_to_config()
        gui.refresh_queue_tree()
        gui.sync_queue_tags_to_analysis_tree()

    return counts
