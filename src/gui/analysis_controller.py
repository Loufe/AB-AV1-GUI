# src/gui/analysis_controller.py
# ruff: noqa: SLF001  # This module accesses VideoConverterGUI internals by design
"""
Analysis tab coordination and event handling.

Manages the analysis workflow including:
- Folder/extension change detection with debouncing
- Background scan lifecycle (start, finish, cleanup)
- ffprobe analysis coordination
- Total row updates
- Add-all-to-queue operations
"""

import logging
import os
import threading
import tkinter as tk
from tkinter import messagebox

from src.estimation import compute_grouped_percentiles, estimate_file_time
from src.gui import analysis_scanner
from src.gui.tree_formatters import clear_sort_state, format_compact_time, format_efficiency, sort_analysis_tree
from src.history_index import get_history_index
from src.models import FileStatus, OperationType
from src.utils import format_file_size

logger = logging.getLogger(__name__)


# =============================================================================
# Scanning Coordination
# =============================================================================


def on_folder_or_extension_changed(gui, *args) -> None:
    """Auto-refresh analysis tree when folder or extensions change.

    Uses a debounce timer to avoid excessive refreshes during rapid changes.

    Args:
        gui: The VideoConverterGUI instance.
        *args: Variable arguments passed by tkinter trace callback.
    """
    # Cancel pending refresh if any
    if gui._refresh_timer_id:
        gui.root.after_cancel(gui._refresh_timer_id)

    # Schedule refresh after 500ms delay (debounce)
    gui._refresh_timer_id = gui.root.after(500, lambda: refresh_analysis_tree(gui))


def refresh_analysis_tree(gui) -> None:
    """Start background scan to populate tree incrementally.

    Args:
        gui: The VideoConverterGUI instance.
    """
    gui._refresh_timer_id = None

    folder = gui.input_folder.get()
    if not folder or not os.path.isdir(folder):
        clear_analysis_tree(gui)
        update_add_all_buttons_state(gui)
        return

    # Get selected extensions
    extensions = []
    if gui.ext_mp4.get():
        extensions.append("mp4")
    if gui.ext_mkv.get():
        extensions.append("mkv")
    if gui.ext_avi.get():
        extensions.append("avi")
    if gui.ext_wmv.get():
        extensions.append("wmv")

    if not extensions:
        clear_analysis_tree(gui)
        update_add_all_buttons_state(gui)
        return

    # Clear tree and start background scan
    clear_analysis_tree(gui)
    gui._scanning = True
    gui.analysis_total_tree.item("total", text="Scanning...", values=("", "—", "—", "—", "—"))
    update_add_all_buttons_state(gui)  # Disable while scanning

    # Show scanning badge (floating indicator, tree visible behind)
    gui.analysis_scan_badge.config(text="Scanning folder...")
    gui.analysis_scan_badge.place(relx=0.5, rely=0.5, anchor="center")
    gui.analysis_scan_badge.lift()

    # Cancel any existing scan
    if gui._scan_stop_event:
        gui._scan_stop_event.set()

    # Start incremental background scan
    gui._scan_stop_event = threading.Event()
    threading.Thread(
        target=analysis_scanner.incremental_scan_thread,
        args=(gui, folder, extensions, gui._scan_stop_event),
        daemon=True,
    ).start()


def prune_empty_folders(gui) -> int:
    """Remove folders with no children from the tree (runs on UI thread).

    Runs repeatedly until no more empty folders are found, handling
    the case where removing a child folder makes its parent empty.

    Args:
        gui: The VideoConverterGUI instance.

    Returns:
        Total number of folders removed.
    """
    total_removed = 0

    while True:
        removed_this_pass = 0
        items_to_check = list(gui.analysis_tree.get_children(""))

        while items_to_check:
            item_id = items_to_check.pop()
            children = gui.analysis_tree.get_children(item_id)

            if children:
                # Has children - check them too
                items_to_check.extend(children)
            else:
                # No children - if it's a folder, remove it
                text = gui.analysis_tree.item(item_id, "text")
                if "📁" in text:
                    gui.analysis_tree.delete(item_id)
                    # Clean up cached folder aggregates
                    if hasattr(gui, "folder_aggregates"):
                        gui.folder_aggregates.pop(item_id, None)
                    removed_this_pass += 1

        total_removed += removed_this_pass
        if removed_this_pass == 0:
            break  # No more empty folders

    return total_removed


def finish_incremental_scan(gui, stopped: bool) -> None:
    """Clean up after incremental scan completes (runs on UI thread).

    Args:
        gui: The VideoConverterGUI instance.
        stopped: True if scan was stopped early, False if completed normally.
    """
    if stopped:
        # Scan was cancelled by a new scan starting - let the new scan handle UI state.
        # Don't hide badge or modify state, as the new scan has already taken over.
        return

    gui._scanning = False

    # Hide scanning badge
    gui.analysis_scan_badge.place_forget()

    # Prune empty folders after scan completes
    prune_empty_folders(gui)

    # Update total row with any cached data
    update_total_from_tree(gui)

    # Sync queue tags to show which files are in the conversion queue
    gui.sync_queue_tags_to_analysis_tree()

    # Enable Add All buttons if there are files
    update_add_all_buttons_state(gui)


# =============================================================================
# Analysis Actions
# =============================================================================


def on_analyze_folders(gui) -> None:
    """Run ffprobe analysis on files already in the tree.

    The tree is populated by the incremental scan when the tab opens or
    folder changes. This button runs ffprobe to get file metadata and
    estimate potential savings.

    Args:
        gui: The VideoConverterGUI instance.
    """
    output_folder = gui.output_folder.get()
    if not output_folder:
        messagebox.showwarning("Invalid Folder", "Please select an output folder for analysis.")
        return

    # Get file paths from existing tree
    file_paths = list(gui._tree_item_map.keys())

    if not file_paths:
        messagebox.showinfo(
            "No Files", "No files to analyze. Select a folder with video files and wait for the scan to complete."
        )
        return

    # Disable buttons immediately to prevent double-clicks
    gui.analyze_button.config(state="disabled")
    gui.add_all_analyze_button.config(state="disabled")
    gui.add_all_convert_button.config(state="disabled")

    input_folder = gui.input_folder.get()
    anonymize = gui.anonymize_history.get()
    logger.info(f"Starting ffprobe analysis of {len(file_paths)} files in: {input_folder}")

    # Cancel any pending auto-refresh timer
    if gui._refresh_timer_id:
        gui.root.after_cancel(gui._refresh_timer_id)
        gui._refresh_timer_id = None

    # Cancel any running analysis or incremental scan
    if gui.analysis_stop_event:
        gui.analysis_stop_event.set()
    if gui._scan_stop_event:
        gui._scan_stop_event.set()

    # Run ffprobe analysis in background thread
    # Pass input_folder and anonymize as parameters (captured on main thread)
    gui.analysis_stop_event = threading.Event()
    gui.analysis_thread = threading.Thread(
        target=analysis_scanner.run_ffprobe_analysis,
        args=(gui, file_paths, output_folder, input_folder, anonymize),
        daemon=True,
    )
    gui.analysis_thread.start()


def on_ffprobe_complete(gui) -> None:
    """Handle ffprobe analysis completion (called on main thread).

    Args:
        gui: The VideoConverterGUI instance.
    """
    gui.analyze_button.config(state="normal")

    # Update total row
    update_total_from_tree(gui)

    # Enable Add All buttons
    update_add_all_buttons_state(gui)

    # Apply default sort by efficiency (highest first) if user hasn't sorted yet
    if gui._sort_col is None:
        sort_analysis_tree(gui, "efficiency", descending=True)


# =============================================================================
# Button State
# =============================================================================


def update_add_all_buttons_state(gui) -> None:
    """Enable/disable the Add All buttons based on whether there are files in the tree.

    Args:
        gui: The VideoConverterGUI instance.
    """
    has_files = bool(gui._tree_item_map)
    state = "normal" if has_files else "disabled"
    gui.add_all_analyze_button.config(state=state)
    gui.add_all_convert_button.config(state=state)


# =============================================================================
# Queue Integration
# =============================================================================


def add_all_to_queue(gui, operation_type: OperationType) -> None:
    """Add all discovered files to the queue with the specified operation type.

    Args:
        gui: The VideoConverterGUI instance.
        operation_type: The operation type (ANALYZE or CONVERT).
    """
    file_paths = list(gui._tree_item_map.keys())
    if not file_paths:
        messagebox.showinfo("No Files", "No files to add. Run a scan first.")
        return

    # Build items list (all files, not folders)
    items = [(path, False) for path in file_paths]

    # Use preview dialog for bulk operations
    result = gui.add_items_to_queue(items, operation_type, force_preview=True)

    total_added = result["added"] + result["conflict_added"] + result["conflict_replaced"]
    if total_added > 0:
        gui.tab_control.select(gui.convert_tab)


def on_add_all_analyze(gui) -> None:
    """Add all discovered files to the queue for CRF analysis.

    Args:
        gui: The VideoConverterGUI instance.
    """
    add_all_to_queue(gui, OperationType.ANALYZE)


def on_add_all_convert(gui) -> None:
    """Add all discovered files to the queue for conversion.

    Args:
        gui: The VideoConverterGUI instance.
    """
    add_all_to_queue(gui, OperationType.CONVERT)


# =============================================================================
# Tree Utilities
# =============================================================================


def get_file_path_for_tree_item(gui, item_id: str) -> str | None:
    """Look up the file path for a given tree item ID.

    Args:
        gui: The VideoConverterGUI instance.
        item_id: The tree item ID to look up.

    Returns:
        The file path, or None if not found.
    """
    for path, tid in gui._tree_item_map.items():
        if tid == item_id:
            return path
    return None


def get_analysis_tree_tooltip(gui, item_id: str) -> str | None:
    """Generate tooltip text for an analysis tree item.

    Args:
        gui: The VideoConverterGUI instance.
        item_id: The tree item ID to generate tooltip for.

    Returns:
        Tooltip text, or None if no tooltip should be shown.
    """
    # Check if it's a folder (has children)
    if gui.analysis_tree.get_children(item_id):
        return None  # No tooltip for folders

    # Get file path for this item
    file_path = get_file_path_for_tree_item(gui, item_id)
    if not file_path:
        return None

    # Look up record from history index
    index = get_history_index()
    record = index.lookup_file(file_path)

    if not record:
        return "Not yet analyzed. Click Analyze to scan."

    # Generate tooltip based on status
    if record.status == FileStatus.CONVERTED:
        # Show conversion details
        lines = ["Already converted"]
        if record.reduction_percent is not None:
            lines[0] += f": {record.reduction_percent:.0f}% smaller"
        if record.final_crf is not None and record.final_vmaf is not None:
            lines.append(f"CRF {record.final_crf}, VMAF {record.final_vmaf:.1f}")
        return "\n".join(lines)

    if record.status == FileStatus.NOT_WORTHWHILE:
        # Show skip reason
        if record.skip_reason:
            return f"Skipped: {record.skip_reason}"
        if record.min_vmaf_attempted:
            return f"Skipped: VMAF {record.min_vmaf_attempted} unattainable"
        return "Skipped: Quality target unattainable"

    if record.status == FileStatus.ANALYZED:
        # Layer 2 complete (CRF search done)
        lines = ["Ready to convert (CRF search complete)"]
        if record.best_crf is not None and record.best_vmaf_achieved is not None:
            lines.append(f"CRF {record.best_crf} → VMAF {record.best_vmaf_achieved:.1f}")
        return "\n".join(lines)

    # FileStatus.SCANNED - check for Layer 2 data (fallback for old records)
    if record.predicted_size_reduction is not None:
        lines = ["Ready to convert (CRF search complete)"]
        if record.best_crf is not None and record.best_vmaf_achieved is not None:
            lines.append(f"CRF {record.best_crf} → VMAF {record.best_vmaf_achieved:.1f}")
        return "\n".join(lines)

    if record.estimated_reduction_percent is not None:
        # Layer 1 only (ffprobe estimate)
        if record.estimated_from_similar and record.estimated_from_similar > 0:
            return f"Estimate based on {record.estimated_from_similar} similar file(s)"
        return None  # No tooltip for generic estimates

    return "Not yet analyzed. Click Analyze to scan."


# =============================================================================
# Completion Handling
# =============================================================================


def update_analysis_tree_for_completed_file(gui, file_path: str, status: str) -> None:
    """Update analysis tree entry when a file completes conversion.

    Args:
        gui: The VideoConverterGUI instance.
        file_path: Full path to the completed file.
        status: Completion status - "done" for successful, "skip" for not worthwhile.
    """
    if not hasattr(gui, "analysis_tree"):
        return

    # Normalize for case-insensitive lookup on Windows
    item_id = gui._tree_item_map.get(os.path.normcase(file_path))
    if not item_id:
        return

    try:
        if not gui.analysis_tree.exists(item_id):
            return

        # Get current tags and remove queue-related ones
        current_tags = list(gui.analysis_tree.item(item_id, "tags") or ())
        current_tags = [t for t in current_tags if t not in ("in_queue", "partial_queue", "done", "skip", "av1")]

        # Add the completion status tag
        current_tags.append(status)

        # Update the tree item
        if status == "done":
            gui.analysis_tree.item(item_id, tags=tuple(current_tags))
            gui.analysis_tree.set(item_id, "savings", "Done")
        elif status == "skip":
            gui.analysis_tree.item(item_id, tags=tuple(current_tags))
            gui.analysis_tree.set(item_id, "savings", "Skip")

        # Clear time and efficiency for completed files
        gui.analysis_tree.set(item_id, "time", "—")
        gui.analysis_tree.set(item_id, "efficiency", "—")

        # Update all ancestor folder aggregates
        parent_id = gui.analysis_tree.parent(item_id)
        while parent_id:
            gui.update_folder_aggregates(parent_id)
            parent_id = gui.analysis_tree.parent(parent_id)

        # Sync queue tags since this file is no longer "in queue" effectively
        gui.sync_queue_tags_to_analysis_tree()

        # Update the total row to reflect done/skip count changes
        update_total_from_tree(gui)

    except tk.TclError:
        pass


# =============================================================================
# Total Row
# =============================================================================


def update_total_row(
    gui,
    total_files: int,
    convertible: int,
    done_count: int,
    skip_count: int,
    total_size: int,
    total_savings: int,
    total_time: float,
    any_estimate: bool = False,
) -> None:
    """Update the fixed total row at the bottom of the analysis tree.

    Args:
        gui: The VideoConverterGUI instance.
        total_files: Total number of files in the tree.
        convertible: Number of files that can be converted.
        done_count: Number of already converted files.
        skip_count: Number of files skipped (not worthwhile).
        total_size: Total size of all files in bytes.
        total_savings: Estimated total savings in bytes.
        total_time: Estimated total time in seconds.
        any_estimate: If True, at least one file lacks CRF search data.
    """
    # Build breakdown string with only non-zero counts
    parts = []
    if convertible > 0:
        parts.append(f"{convertible} convertible")
    if done_count > 0:
        parts.append(f"{done_count} done")
    if skip_count > 0:
        parts.append(f"{skip_count} skipped")

    name_text = f"Total: {', '.join(parts)} / {total_files} files" if parts else f"Total ({total_files} files)"

    size_str = format_file_size(total_size) if total_size > 0 else "—"
    savings_str = format_file_size(total_savings) if total_savings > 0 else "—"
    if any_estimate and savings_str != "—":
        savings_str = f"~{savings_str}"
    # Total row uses "low" confidence if any file is an estimate (aggregated estimates)
    total_confidence = "low" if any_estimate else "high"
    time_str = format_compact_time(total_time, confidence=total_confidence)
    eff_str = format_efficiency(total_savings, total_time)

    # Update the total row (format column is empty for totals)
    gui.analysis_total_tree.item("total", text=name_text, values=("", size_str, savings_str, time_str, eff_str))


def update_total_from_tree(gui) -> int:
    """Compute and update totals from files in the tree using history index.

    Iterates through all files in _tree_item_map, looks up their records
    in the history index, and sums up savings/time for convertible files.

    Args:
        gui: The VideoConverterGUI instance.

    Returns:
        Number of convertible files found.
    """
    index = get_history_index()
    total_files = len(gui._tree_item_map)
    convertible = 0
    done_count = 0
    skip_count = 0
    total_size = 0
    total_savings = 0
    total_time = 0.0
    any_estimate = False  # Track if any file lacks CRF search (layer 2) data

    # Pre-compute percentiles once for all time estimates
    grouped_percentiles = compute_grouped_percentiles()

    for file_path in gui._tree_item_map:
        record = index.lookup_file(file_path)
        if not record:
            continue
        # Sum size for all files
        if record.file_size_bytes:
            total_size += record.file_size_bytes
        if record.status == FileStatus.CONVERTED:
            done_count += 1
        elif record.status == FileStatus.NOT_WORTHWHILE:
            skip_count += 1
        else:
            # Use Layer 2 data if available, otherwise fall back to Layer 1 estimate
            reduction_percent = record.predicted_size_reduction or record.estimated_reduction_percent
            if record.status in (FileStatus.SCANNED, FileStatus.ANALYZED) and reduction_percent:
                convertible += 1
                # Track if this file only has ffprobe-level analysis (no CRF search)
                if record.predicted_size_reduction is None:
                    any_estimate = True
                if record.file_size_bytes:
                    total_savings += int(record.file_size_bytes * reduction_percent / 100)
                file_time = estimate_file_time(
                    codec=record.video_codec,
                    duration=record.duration_sec,
                    width=record.width,
                    height=record.height,
                    grouped_percentiles=grouped_percentiles,
                ).best_seconds
                total_time += file_time

    update_total_row(
        gui, total_files, convertible, done_count, skip_count, total_size, total_savings, total_time, any_estimate
    )
    return convertible


# =============================================================================
# Cleanup
# =============================================================================


def clear_analysis_tree(gui) -> None:
    """Clear analysis tree, item map, and sort state.

    Args:
        gui: The VideoConverterGUI instance.
    """
    for item in gui.analysis_tree.get_children():
        gui.analysis_tree.delete(item)
    gui._tree_item_map.clear()
    if hasattr(gui, "folder_aggregates"):
        gui.folder_aggregates.clear()
    clear_sort_state(gui)
