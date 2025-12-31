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
from src.models import FileStatus
from src.utils import format_file_size


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
        if item.status not in ("pending", "converting"):
            continue

        if item.is_folder:
            # Find all files in tree_item_map that are under this folder
            folder_prefix = item.source_path + os.sep
            for file_path in gui.get_tree_item_map():
                if file_path.startswith(folder_prefix) or file_path == item.source_path:
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
