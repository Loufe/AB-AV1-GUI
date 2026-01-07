# src/gui/queue_manager.py
"""
Queue management functions extracted from main_window.py.

These functions handle queue item creation, categorization, and addition logic.
"""

import os
import uuid

from src.conversion_engine.scanner import find_video_files
from src.estimation import compute_grouped_percentiles, get_resolution_bucket
from src.gui.analysis_tree import extract_paths_from_queue_items
from src.gui.widgets.add_to_queue_dialog import AddToQueuePreviewDialog, QueuePreviewData
from src.history_index import get_history_index
from src.models import FileStatus, OperationType, OutputMode, QueueFileItem, QueueItem, QueueItemStatus
from src.privacy import normalize_path


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
                # Reset nested file statuses to PENDING as well
                for file_item in item.files:
                    file_item.status = QueueItemStatus.PENDING
                    file_item.error_message = None
            # Only restore PENDING items if file exists (skip completed/error items)
            if item.status == QueueItemStatus.PENDING and os.path.exists(item.source_path):
                items.append(item)
        except (KeyError, ValueError):
            continue  # Skip invalid entries
    return items


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


def filter_file_for_queue(
    file_path: str, operation_type: OperationType, index=None
) -> tuple[bool, str | None]:
    """Check if a file should be added to the queue.

    Args:
        file_path: Path to the video file.
        operation_type: The operation type (CONVERT or ANALYZE).
        index: Optional HistoryIndex instance (will get singleton if not provided).

    Returns:
        Tuple of (should_add, skip_reason) where:
        - should_add: True if file passes all filters
        - skip_reason: Reason string if skipped, None if should_add is True
    """
    if index is None:
        index = get_history_index()

    record = index.lookup_file(file_path)
    if record:
        # Already converted - skip
        if record.status == FileStatus.CONVERTED:
            return False, "already converted"
        # Not worth converting - skip
        if record.status == FileStatus.NOT_WORTHWHILE:
            return False, "not worth converting"
        # Already analyzed - skip for ANALYZE operations
        if operation_type == OperationType.ANALYZE and record.status == FileStatus.ANALYZED:
            return False, "already analyzed"
        # Already AV1 codec - skip for CONVERT operations
        if operation_type == OperationType.CONVERT and record.video_codec == "av1":
            return False, "already AV1"

    return True, None


def create_queue_item(
    gui,
    path: str,
    is_folder: bool,
    operation_type: OperationType,
    cached_files: list[str] | None = None,
) -> QueueItem:
    """Create a new QueueItem with default settings.

    For folder items, populates the files list by scanning for video files.
    Files are filtered based on history status (already converted, not worthwhile, etc.).

    Args:
        gui: The GUI instance.
        path: Path to the file or folder.
        is_folder: Whether the path is a folder.
        operation_type: The operation type (CONVERT or ANALYZE).
        cached_files: Optional pre-scanned and filtered file list for folders (Fix 3).
                     If provided, skips rescanning the folder.
    """
    default_mode = gui.default_output_mode.get()

    # For folders, populate the files list
    files: list[QueueFileItem] = []
    if is_folder:
        # Use cached files if available (Fix 3: avoid duplicate scanning)
        if cached_files is not None:
            file_paths = cached_files
        else:
            # Fallback: scan and filter (for cases where cache isn't available)
            extensions = get_selected_extensions(gui)
            file_paths = []
            if extensions:
                all_files = find_video_files(path, extensions)
                index = get_history_index()
                for fp in all_files:
                    should_add, _ = filter_file_for_queue(fp, operation_type, index)
                    if should_add:
                        file_paths.append(fp)

        for fp in file_paths:
            try:
                size = os.path.getsize(fp) if os.path.isfile(fp) else 0
            except OSError:
                size = 0
            files.append(QueueFileItem(path=fp, size_bytes=size))

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
) -> tuple[
    list[tuple[str, bool]],
    list[str],
    list[tuple[str, bool, QueueItem]],
    list[tuple[str, str]],
    dict[str, list[str]],
]:
    """Categorize items for queue preview.

    Filters out items that shouldn't be added based on their history status:
    - Already converted (CONVERTED status)
    - Not worth converting (NOT_WORTHWHILE status)
    - Already AV1 codec (for CONVERT operations)
    - Folders with no convertible files

    Returns:
        Tuple of (to_add, duplicates, conflicts, skipped, folder_files_cache) where:
        - to_add: Items that can be added directly (files/folders with convertible content)
        - duplicates: Paths already in queue with same operation
        - conflicts: (path, is_folder, existing_item) for different operation
        - skipped: (path, reason) for files/folders filtered out
        - folder_files_cache: {folder_path: [file_paths]} for folders that passed filtering
    """
    to_add: list[tuple[str, bool]] = []
    duplicates: list[str] = []
    conflicts: list[tuple[str, bool, QueueItem]] = []
    skipped: list[tuple[str, str]] = []
    folder_files_cache: dict[str, list[str]] = {}

    index = get_history_index()
    extensions = get_selected_extensions(gui)

    # Build O(1) lookup for existing queue items (Fix 1: avoid O(N*M) linear search)
    queue_path_map = {normalize_path(item.source_path): item for item in gui.get_queue_items()}

    for path, is_folder in items:
        # O(1) lookup instead of O(M) linear search
        normalized_path = normalize_path(path)
        existing = queue_path_map.get(normalized_path)
        if existing:
            if existing.operation_type == operation_type:
                duplicates.append(path)
            else:
                conflicts.append((path, is_folder, existing))
            continue

        # For folders, scan contents and check if any files pass the filter
        if is_folder:
            if not extensions:
                skipped.append((path, "no file extensions selected"))
                continue

            file_paths = find_video_files(path, extensions)
            if not file_paths:
                skipped.append((path, "no video files found"))
                continue

            # Check each file in the folder and cache convertible files
            convertible_files: list[str] = []
            for fp in file_paths:
                should_add, reason = filter_file_for_queue(fp, operation_type, index)
                if should_add:
                    convertible_files.append(fp)
                else:
                    skipped.append((fp, reason or "filtered"))

            if not convertible_files:
                skipped.append((path, "no convertible files (all filtered)"))
            else:
                to_add.append((path, is_folder))
                # Cache the file list to avoid re-scanning in create_queue_item (Fix 3)
                folder_files_cache[path] = convertible_files
            continue

        # Individual file - use shared filter function
        should_add, reason = filter_file_for_queue(path, operation_type, index)
        if should_add:
            to_add.append((path, is_folder))
        else:
            skipped.append((path, reason or "filtered"))

    return to_add, duplicates, conflicts, skipped, folder_files_cache


def calculate_queue_estimates(
    gui, items: list[tuple[str, bool]], operation_type: OperationType
) -> tuple[float | None, float | None]:
    """Calculate time estimate and potential savings for items.

    Uses pre-computed encoding rate percentiles grouped by (codec, resolution) for
    O(C+N) complexity instead of O(N*C) from calling estimate_file_time() per file.

    Args:
        items: List of (path, is_folder) tuples
        operation_type: ANALYZE uses crf_search_time only, CONVERT uses full encoding time

    Returns:
        Tuple of (estimated_time_seconds, estimated_savings_percent)
    """
    total_time = 0.0
    total_original_size = 0
    total_saved_bytes = 0.0
    has_time_estimates = False

    index = get_history_index()

    # Pre-compute percentiles grouped by (codec, resolution) in one pass
    # Pass operation_type so ANALYZE uses crf_search_time, CONVERT uses encoding_time
    percentiles_by_group = compute_grouped_percentiles(operation_type)
    global_percentiles = percentiles_by_group.get((None, None))

    for path, is_folder in items:
        if is_folder:
            continue  # Skip folders for now, would need to scan

        # O(1) lookup from history index
        record = index.lookup_file(path)
        if not record:
            continue

        # Time estimate using pre-computed percentiles (avoids ffprobe and O(C) searches)
        duration = record.duration_sec
        if duration and duration > 0:
            codec = record.video_codec
            res_bucket = get_resolution_bucket(record.width, record.height)

            # Try (codec, resolution) first, then codec-only, then global
            stats = (
                percentiles_by_group.get((codec, res_bucket))
                or percentiles_by_group.get((codec, None))
                or global_percentiles
            )
            if stats:
                total_time += duration * stats["p50"]
                has_time_estimates = True

        # Savings estimate from cached record (already O(1))
        if record.file_size_bytes:
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
        Dict with counts: {"added", "duplicate", "conflict_added", "conflict_replaced", "cancelled", "skipped"}
    """
    counts = {"added": 0, "duplicate": 0, "conflict_added": 0, "conflict_replaced": 0, "cancelled": 0, "skipped": 0}

    if not items:
        return counts

    # Categorize items (includes filtering and caches folder file lists)
    to_add, duplicates, conflicts, skipped, folder_files_cache = categorize_queue_items(
        gui, items, operation_type
    )
    counts["duplicate"] = len(duplicates)
    counts["skipped"] = len(skipped)

    # Determine if we need to show the preview dialog
    show_dialog = force_preview or len(conflicts) > 0 or len(skipped) > 0

    if show_dialog:
        # Calculate estimates for preview (uses operation_type for correct time estimates)
        estimated_time, estimated_savings = calculate_queue_estimates(gui, to_add, operation_type)

        # Build preview data
        preview_data = QueuePreviewData(
            to_add=to_add,
            duplicates=duplicates,
            conflicts=conflicts,
            skipped=skipped,
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
    items_added: list[QueueItem] = []
    for path, is_folder in to_add:
        # Use cached file list for folders to avoid re-scanning (Fix 3)
        cached_files = folder_files_cache.get(path) if is_folder else None
        item = create_queue_item(gui, path, is_folder, operation_type, cached_files)
        # Safety check: don't add folders with no convertible files
        if is_folder and not item.files:
            continue
        queue_items.append(item)
        items_added.append(item)
        counts["added"] += 1

    # Handle conflicts based on resolution choice
    # Note: Conflicts won't have cached files (they're detected before folder scanning),
    # so create_queue_item will do the scanning for folder conflicts. This is acceptable
    # since conflicts are rare (only when same path exists with different operation type).
    if conflicts:
        if conflict_resolution == "keep_both":
            for path, is_folder, _ in conflicts:
                item = create_queue_item(gui, path, is_folder, operation_type)
                # Safety check: don't add folders with no convertible files
                if is_folder and not item.files:
                    continue
                queue_items.append(item)
                items_added.append(item)
                counts["conflict_added"] += 1
        elif conflict_resolution == "replace":
            for path, is_folder, existing in conflicts:
                new_item = create_queue_item(gui, path, is_folder, operation_type)
                # Safety check: don't replace with empty folder
                if is_folder and not new_item.files:
                    continue
                idx = queue_items.index(existing)
                queue_items[idx] = new_item
                items_added.append(new_item)
                counts["conflict_replaced"] += 1
        # else: skip - conflicts are not added

    # Save and refresh if anything was added or modified
    if counts["added"] > 0 or counts["conflict_added"] > 0 or counts["conflict_replaced"] > 0:
        gui.save_queue_to_config()
        gui.refresh_queue_tree()
        added_paths = extract_paths_from_queue_items(items_added)
        gui.sync_queue_tags_to_analysis_tree(added_paths=added_paths)

    return counts
