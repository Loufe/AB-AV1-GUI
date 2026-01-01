# src/gui/tree_state_manager.py
"""
Tree state management functions for the analysis tree.

Extracted from main_window.py to manage tree state synchronization
and aggregate calculations.
"""

import os
import tkinter as tk

from src.estimation import estimate_file_time
from src.gui.tree_formatters import format_compact_time, format_efficiency
from src.history_index import get_history_index
from src.models import FileStatus, QueueItemStatus
from src.utils import format_file_size


def update_tree_row(gui, file_path: str):
    """Update a single file row with data from history index.

    Args:
        gui: The VideoConverterGUI instance.
        file_path: Path to the file that was analyzed.
    """
    # Find the tree item by file_path
    item_id = gui.get_tree_item_map().get(file_path)
    if not item_id or not gui.analysis_tree.exists(item_id):
        return

    # Get file data from history index
    index = get_history_index()
    record = index.lookup_file(file_path)

    if not record:
        return

    has_layer2 = record.predicted_size_reduction is not None
    # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
    reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent

    # Calculate display values based on record status
    size_str = format_file_size(record.file_size_bytes) if record.file_size_bytes else "—"
    tag = ""
    if record.status == FileStatus.SCANNED and reduction_percent and record.file_size_bytes:
        # File needs conversion - show estimates
        file_savings = int(record.file_size_bytes * reduction_percent / 100)
        savings_str = format_file_size(file_savings)
        if not has_layer2:
            savings_str = f"~{savings_str}"

        file_time = estimate_file_time(
            codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
        ).best_seconds
        time_str = format_compact_time(file_time) if file_time > 0 else "—"
        eff_str = format_efficiency(file_savings, file_time)
    elif record.status == FileStatus.CONVERTED:
        savings_str = "Done"
        time_str = "—"
        eff_str = "—"
        tag = "done"
    elif record.status == FileStatus.NOT_WORTHWHILE:
        savings_str = "Skip"
        time_str = "—"
        eff_str = "—"
        tag = "skip"
    elif record.video_codec and record.video_codec.lower() == "av1":
        # Already AV1 - show subtle indicator
        savings_str = "AV1"
        time_str = "—"
        eff_str = "—"
        tag = "av1"
    else:
        # No data yet
        savings_str = "—"
        time_str = "—"
        eff_str = "—"

    # Update tree item - preserve queue tags (in_queue, partial_queue) while updating status tags
    current_tags = list(gui.analysis_tree.item(item_id, "tags") or ())
    queue_tags = [t for t in current_tags if t in ("in_queue", "partial_queue")]
    new_tags = queue_tags + ([tag] if tag else [])
    gui.analysis_tree.item(item_id, values=(size_str, savings_str, time_str, eff_str), tags=tuple(new_tags))

    # Update all ancestor folder aggregates
    parent_id = gui.analysis_tree.parent(item_id)
    while parent_id:
        update_folder_aggregates(gui, parent_id)
        parent_id = gui.analysis_tree.parent(parent_id)


def batch_update_tree_rows(gui, file_paths: list[str]) -> None:
    """Update multiple tree rows efficiently without per-file folder updates.

    Updates file rows and collects affected folders, then updates folder
    aggregates once at the end. Much faster than calling update_tree_row
    for each file individually.

    Args:
        gui: The VideoConverterGUI instance.
        file_paths: List of file paths to update.
    """
    if not file_paths:
        return

    index = get_history_index()
    affected_folders: set[str] = set()

    for file_path in file_paths:
        item_id = gui.get_tree_item_map().get(file_path)
        if not item_id or not gui.analysis_tree.exists(item_id):
            continue

        record = index.lookup_file(file_path)
        if not record:
            continue

        # Calculate display values
        size_str = format_file_size(record.file_size_bytes) if record.file_size_bytes else "—"
        tag = ""
        # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
        has_layer2 = record.predicted_size_reduction is not None
        reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent
        if record.status == FileStatus.SCANNED and reduction_percent and record.file_size_bytes:
            file_savings = int(record.file_size_bytes * reduction_percent / 100)
            savings_str = format_file_size(file_savings)
            if not has_layer2:
                savings_str = f"~{savings_str}"
            file_time = estimate_file_time(
                codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
            ).best_seconds
            time_str = format_compact_time(file_time) if file_time > 0 else "—"
            eff_str = format_efficiency(file_savings, file_time)
        elif record.status == FileStatus.CONVERTED:
            savings_str = "Done"
            time_str = "—"
            eff_str = "—"
            tag = "done"
        elif record.status == FileStatus.NOT_WORTHWHILE:
            savings_str = "Skip"
            time_str = "—"
            eff_str = "—"
            tag = "skip"
        elif record.video_codec and record.video_codec.lower() == "av1":
            # Already AV1 - show subtle indicator
            savings_str = "AV1"
            time_str = "—"
            eff_str = "—"
            tag = "av1"
        else:
            savings_str = "—"
            time_str = "—"
            eff_str = "—"

        # Update tree item - preserve queue tags (in_queue, partial_queue) while updating status tags
        current_tags = list(gui.analysis_tree.item(item_id, "tags") or ())
        # Remove old status tags (done, skip, av1) but keep queue tags
        queue_tags = [t for t in current_tags if t in ("in_queue", "partial_queue")]
        new_tags = queue_tags + ([tag] if tag else [])
        gui.analysis_tree.item(
            item_id, values=(size_str, savings_str, time_str, eff_str), tags=tuple(new_tags)
        )

        # Track all ancestor folders for batch aggregate update
        parent_id = gui.analysis_tree.parent(item_id)
        while parent_id:
            affected_folders.add(parent_id)
            parent_id = gui.analysis_tree.parent(parent_id)

    # Update folder aggregates once for all affected folders
    # Pre-build reverse map once for all folder updates
    if affected_folders:
        item_to_path = {item_id: path for path, item_id in gui.get_tree_item_map().items()}
        for folder_id in affected_folders:
            update_folder_aggregates(gui, folder_id, item_to_path)


def update_folder_aggregates(gui, folder_id: str, item_to_path: dict[str, str] | None = None):
    """Recalculate and update folder aggregate values from history index.

    Args:
        gui: The VideoConverterGUI instance.
        folder_id: The tree item ID of the folder to update.
        item_to_path: Optional pre-built reverse map of item_id -> file_path.
                     If not provided, builds one (slower for batch updates).
    """
    if item_to_path is None:
        item_to_path = {item_id: path for path, item_id in gui.get_tree_item_map().items()}

    # Get all children (files)
    children = gui.analysis_tree.get_children(folder_id)

    # Sum up size, savings and time from all files using history index
    total_size = 0
    total_savings = 0
    total_time = 0
    any_estimate = False  # Track if any file lacks CRF search (layer 2) data

    index = get_history_index()

    for child_id in children:
        file_path = item_to_path.get(child_id)
        if not file_path:
            continue

        # Look up file data from history index
        record = index.lookup_file(file_path)
        if not record:
            continue

        # Sum size for all files
        if record.file_size_bytes:
            total_size += record.file_size_bytes

        # Check if file needs conversion and has estimates
        # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
        reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent
        if record.status == FileStatus.SCANNED and reduction_percent:
            # Track if this file only has ffprobe-level analysis (no CRF search)
            if record.predicted_size_reduction is None:
                any_estimate = True

            # Calculate savings from reduction percentage
            if record.file_size_bytes:
                file_savings = int(record.file_size_bytes * reduction_percent / 100)
                total_savings += file_savings

            # Get time estimate
            file_time = estimate_file_time(
                codec=record.video_codec, duration=record.duration_sec, size=record.file_size_bytes
            ).best_seconds
            total_time += file_time

    # Update folder display (efficiency = aggregate savings / aggregate time)
    size_str = format_file_size(total_size) if total_size > 0 else "—"
    savings_str = format_file_size(total_savings) if total_savings > 0 else "—"
    if any_estimate and savings_str != "—":
        savings_str = f"~{savings_str}"
    time_str = format_compact_time(total_time) if total_time > 0 else "—"
    eff_str = format_efficiency(total_savings, total_time)
    gui.analysis_tree.item(folder_id, values=(size_str, savings_str, time_str, eff_str))


def get_queued_file_paths(gui) -> set[str]:
    """Get set of all file paths currently in the conversion queue.

    For folder queue items, finds all video files under that folder
    that are in the analysis tree. For file queue items, returns the path directly.

    Args:
        gui: The VideoConverterGUI instance.

    Returns:
        Set of file paths that are in pending/converting queue items.
    """
    queued_paths: set[str] = set()

    for item in gui.get_queue_items():
        if item.status not in (QueueItemStatus.PENDING, QueueItemStatus.CONVERTING):
            continue

        if item.is_folder:
            # Find all files in tree_item_map that are under this folder
            # Normalize paths to handle mixed separators on Windows
            normalized_folder = os.path.normpath(item.source_path)
            folder_prefix = normalized_folder + os.sep
            for file_path in gui.get_tree_item_map():
                normalized_file = os.path.normpath(file_path)
                if normalized_file.startswith(folder_prefix) or normalized_file == normalized_folder:
                    queued_paths.add(file_path)
        else:
            # Single file
            queued_paths.add(item.source_path)

    return queued_paths


def sync_queue_tags_to_analysis_tree(gui):
    """Synchronize queue status to analysis tree item tags.

    Applies 'in_queue' tag to files in the queue, 'partial_queue' to folders
    with some (but not all) files queued, and removes queue tags from items
    no longer in queue.

    Args:
        gui: The VideoConverterGUI instance.
    """
    if not hasattr(gui, "analysis_tree") or not gui.get_tree_item_map():
        return

    queued_paths = get_queued_file_paths(gui)

    # Track which folders need updating and their child stats
    folder_stats: dict[str, tuple[int, int]] = {}  # folder_id -> (queued_count, total_count)

    # Update file tags
    for file_path, item_id in gui.get_tree_item_map().items():
        try:
            if not gui.analysis_tree.exists(item_id):
                continue

            current_tags = list(gui.analysis_tree.item(item_id, "tags") or ())

            # Remove existing queue tags
            current_tags = [t for t in current_tags if t not in ("in_queue", "partial_queue")]

            # Add queue tag if file is in queue
            is_queued = file_path in queued_paths
            if is_queued:
                current_tags.append("in_queue")

            gui.analysis_tree.item(item_id, tags=tuple(current_tags))

            # Track parent folder stats
            parent_id = gui.analysis_tree.parent(item_id)
            if parent_id:
                queued, total = folder_stats.get(parent_id, (0, 0))
                folder_stats[parent_id] = (queued + (1 if is_queued else 0), total + 1)

        except tk.TclError:
            continue

    # Update folder tags based on child stats
    for folder_id, (queued_count, total_count) in folder_stats.items():
        try:
            if not gui.analysis_tree.exists(folder_id):
                continue

            current_tags = list(gui.analysis_tree.item(folder_id, "tags") or ())
            current_tags = [t for t in current_tags if t not in ("in_queue", "partial_queue")]

            if queued_count == total_count and total_count > 0:
                # All children queued
                current_tags.append("in_queue")
            elif queued_count > 0:
                # Some children queued
                current_tags.append("partial_queue")

            gui.analysis_tree.item(folder_id, tags=tuple(current_tags))
        except tk.TclError:
            continue
