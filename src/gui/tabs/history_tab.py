# src/gui/tabs/history_tab.py
"""
History tab module for viewing conversion history.

Displays a flat list of files processed by ab-av1 (CONVERTED, ANALYZED,
NOT_WORTHWHILE statuses) sorted by date with filtering controls.
"""

import logging
import os
import subprocess
import sys
import tkinter as tk
from tkinter import ttk
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from src.gui.main_window import VideoConverterGUI

from src.config import HISTORY_TREE_HEADINGS
from src.gui.base import ToolTip, TreeviewHeaderTooltip
from src.gui.constants import COLOR_STATUS_SUCCESS, COLOR_STATUS_WARNING
from src.history_index import get_history_index
from src.models import FileRecord, FileStatus
from src.utils import format_file_size, format_time

logger = logging.getLogger(__name__)

# Column widths (sized for 850px min / 950px default window)
COLUMN_WIDTHS = {
    "date": 78,
    "#0": 120,  # Name - minimum width, stretches
    "status": 75,
    "resolution": 82,
    "codec": 52,
    "bitrate": 72,
    "duration": 65,
    "audio": 48,
    "input_size": 65,
    "output_size": 65,
    "reduction": 72,
    "vmaf": 48,
    "crf": 38,
}


def create_history_tab(gui: "VideoConverterGUI") -> None:
    """Create the history tab."""
    main = ttk.Frame(gui.history_tab)
    main.pack(fill="both", expand=True, padx=10, pady=10)

    # Configure grid weights
    main.columnconfigure(0, weight=1)
    main.rowconfigure(1, weight=1)  # Tree row expands

    # --- Row 0: Controls ---
    controls_frame = ttk.Frame(main)
    controls_frame.grid(row=0, column=0, sticky="ew", pady=(0, 10))

    # Filter checkboxes
    ttk.Label(controls_frame, text="Show:").pack(side="left", padx=(0, 5))

    converted_cb = ttk.Checkbutton(
        controls_frame,
        text="Converted",
        variable=gui.history_show_converted,
        command=lambda: _on_filter_changed(gui),
    )
    converted_cb.pack(side="left", padx=(0, 10))
    ToolTip(converted_cb, "Files fully encoded to AV1")

    analyzed_cb = ttk.Checkbutton(
        controls_frame,
        text="Analyzed",
        variable=gui.history_show_analyzed,
        command=lambda: _on_filter_changed(gui),
    )
    analyzed_cb.pack(side="left", padx=(0, 10))
    ToolTip(analyzed_cb, "Files analyzed for optimal CRF but not yet encoded")

    skipped_cb = ttk.Checkbutton(
        controls_frame,
        text="Skipped",
        variable=gui.history_show_skipped,
        command=lambda: _on_filter_changed(gui),
    )
    skipped_cb.pack(side="left", padx=(0, 10))
    ToolTip(skipped_cb, "Files skipped (already AV1, or no worthwhile savings)")

    # Status label (right side)
    gui.history_status_label = ttk.Label(controls_frame, text="")
    gui.history_status_label.pack(side="right")

    # --- Row 1: Treeview ---
    tree_frame = ttk.Frame(main)
    tree_frame.grid(row=1, column=0, sticky="nsew")
    tree_frame.columnconfigure(0, weight=1)
    tree_frame.rowconfigure(0, weight=1)

    # Define columns (excluding #0 which is the tree column)
    columns = (
        "date",
        "status",
        "resolution",
        "codec",
        "bitrate",
        "duration",
        "audio",
        "input_size",
        "output_size",
        "reduction",
        "vmaf",
        "crf",
    )

    gui.history_tree = ttk.Treeview(
        tree_frame,
        columns=columns,
        show="tree headings",
        selectmode="extended",
    )

    # Configure columns
    gui.history_tree.column("#0", width=COLUMN_WIDTHS["#0"], minwidth=80, stretch=True)
    gui.history_tree.heading("#0", text=HISTORY_TREE_HEADINGS["#0"], command=lambda: sort_history_tree(gui, "#0"))

    for col in columns:
        width = COLUMN_WIDTHS.get(col, 60)
        anchor = "center" if col in ("status", "resolution", "codec", "audio") else "e"
        if col == "date":
            anchor = "w"
        gui.history_tree.column(col, width=width, minwidth=width, stretch=False, anchor=anchor)
        gui.history_tree.heading(
            col,
            text=HISTORY_TREE_HEADINGS.get(col, col.title()),
            command=lambda c=col: sort_history_tree(gui, c),
        )

    # Configure tags for status coloring
    gui.history_tree.tag_configure("done", foreground=COLOR_STATUS_SUCCESS)
    gui.history_tree.tag_configure("skip", foreground=COLOR_STATUS_WARNING)

    # Set up column header tooltips
    TreeviewHeaderTooltip(gui.history_tree, {
        "bitrate": "Source video bitrate.\nHigher usually means higher quality source.",
        "reduction": "Percentage of original file size saved.\nHigher = more space saved.",
        "vmaf": "Quality score achieved (0-100).\n95+ is visually lossless.",
        "crf": "Compression level used (0-63).\nLower = higher quality.",
    })

    # Scrollbar
    scrollbar = ttk.Scrollbar(tree_frame, orient="vertical", command=gui.history_tree.yview)
    gui.history_tree.configure(yscrollcommand=scrollbar.set)

    gui.history_tree.grid(row=0, column=0, sticky="nsew")
    scrollbar.grid(row=0, column=1, sticky="ns")

    # Bind right-click for context menu
    gui.history_tree.bind("<Button-3>", lambda e: _show_context_menu(gui, e))

    # Initial load
    gui.root.after(100, lambda: refresh_history_view(gui))


def _on_filter_changed(gui: "VideoConverterGUI") -> None:
    """Handle filter checkbox change with debounce."""
    # Cancel any pending refresh
    if hasattr(gui, "_history_filter_timer") and gui._history_filter_timer:
        gui.root.after_cancel(gui._history_filter_timer)

    # Schedule refresh with 50ms debounce
    gui._history_filter_timer = gui.root.after(50, lambda: refresh_history_view(gui))


def refresh_history_view(gui: "VideoConverterGUI") -> None:
    """Load records and populate the tree."""
    try:
        records = load_history_records(gui)
        populate_history_tree(gui, records)

        # Reapply current sort if one is active, otherwise keep default date order
        if gui._history_sort_col:
            _reapply_current_sort(gui)
        else:
            # No sort active - default is date descending, update indicators to show this
            gui._history_sort_col = "date"
            gui._history_sort_reverse = True
            _update_sort_indicators(gui)

        gui.history_status_label.config(text=f"Showing {len(records)} records")
    except Exception:
        logger.exception("Error loading history")
        gui.history_status_label.config(text="Error loading history")


def load_history_records(gui: "VideoConverterGUI") -> list[FileRecord]:
    """Query HistoryIndex for records based on filter checkboxes."""
    index = get_history_index()
    records: list[FileRecord] = []

    if gui.history_show_converted.get():
        records.extend(index.get_by_status(FileStatus.CONVERTED))
    if gui.history_show_analyzed.get():
        records.extend(index.get_by_status(FileStatus.ANALYZED))
    if gui.history_show_skipped.get():
        records.extend(index.get_by_status(FileStatus.NOT_WORTHWHILE))

    # Sort by date descending (most recent first)
    records.sort(key=lambda r: r.last_updated or "", reverse=True)
    return records


def populate_history_tree(gui: "VideoConverterGUI", records: list[FileRecord]) -> None:
    """Clear and fill tree with records."""
    # Clear existing items
    gui.history_tree.delete(*gui.history_tree.get_children())
    gui._history_tree_map.clear()

    for record in records:
        name = _format_name(record)
        values = compute_history_display_values(record)
        tag = get_history_status_tag(record.status)

        item_id = gui.history_tree.insert(
            "",
            "end",
            text=name,
            values=values,
            tags=(tag,) if tag else (),
        )
        gui._history_tree_map[record.path_hash] = item_id


def _format_name(record: FileRecord) -> str:
    """Format filename for display."""
    if record.original_path:
        return os.path.basename(record.original_path)
    # Anonymized: show truncated hash
    return f"file_{record.path_hash[:8]}..."


def compute_history_display_values(record: FileRecord) -> tuple[str, ...]:
    """Compute all column values for a history record."""
    # Date
    date_str = record.last_updated[:10] if record.last_updated else "—"

    # Status
    status_map = {
        FileStatus.CONVERTED: "Converted",
        FileStatus.ANALYZED: "Analyzed",
        FileStatus.NOT_WORTHWHILE: "Skipped",
    }
    status = status_map.get(record.status, "—")

    # Resolution
    resolution = f"{record.width}x{record.height}" if record.width and record.height else "—"

    # Codec
    codec = (record.video_codec or "—").upper()

    # Bitrate (convert kbps to Mbps)
    if record.bitrate_kbps:
        if record.bitrate_kbps >= 1000:
            bitrate = f"{record.bitrate_kbps / 1000:.1f} Mbps"
        else:
            bitrate = f"{record.bitrate_kbps:.0f} kbps"
    else:
        bitrate = "—"

    # Duration
    duration = format_time(record.duration_sec) if record.duration_sec else "—"

    # Audio codec(s) from audio_streams
    if record.audio_streams:
        if len(record.audio_streams) == 1:
            audio = record.audio_streams[0].codec.upper()
        elif len(record.audio_streams) <= 3:
            # Show all codecs: "AAC, AC3"
            codecs = [s.codec.upper() for s in record.audio_streams]
            audio = ", ".join(codecs)
        else:
            # Too many - show count
            audio = f"{len(record.audio_streams)} audio"
    else:
        audio = "—"

    # Input size
    input_size = format_file_size(record.file_size_bytes) if record.file_size_bytes else "—"

    # Output columns - only show for CONVERTED status
    if record.status == FileStatus.CONVERTED:
        output_size = format_file_size(record.output_size_bytes) if record.output_size_bytes else "—"
        reduction = f"{record.reduction_percent:.1f}%" if record.reduction_percent is not None else "—"
        vmaf = f"{record.final_vmaf:.1f}" if record.final_vmaf is not None else "—"
        crf = str(record.final_crf) if record.final_crf is not None else "—"
    else:
        output_size = "—"
        reduction = "—"
        vmaf = "—"
        crf = "—"

    return (
        date_str, status, resolution, codec, bitrate, duration,
        audio, input_size, output_size, reduction, vmaf, crf,
    )


def get_history_status_tag(status: FileStatus | None) -> str:
    """Get the display tag for status coloring."""
    if status == FileStatus.CONVERTED:
        return "done"
    if status == FileStatus.NOT_WORTHWHILE:
        return "skip"
    return ""


def sort_history_tree(gui: "VideoConverterGUI", col: str) -> None:
    """Sort the history tree by the specified column (called on header click)."""
    # Toggle direction if same column clicked
    if gui._history_sort_col == col:
        gui._history_sort_reverse = not gui._history_sort_reverse
    else:
        gui._history_sort_col = col
        # Default to descending for date, ascending for others
        gui._history_sort_reverse = col == "date"

    _apply_sort(gui)


def _reapply_current_sort(gui: "VideoConverterGUI") -> None:
    """Reapply the current sort after data refresh (maintains direction)."""
    if gui._history_sort_col:
        _apply_sort(gui)


def _apply_sort(gui: "VideoConverterGUI") -> None:
    """Apply the current sort column and direction to the tree."""
    col = gui._history_sort_col
    if not col:
        return

    # Get all items
    items = []
    for item_id in gui.history_tree.get_children():
        text = gui.history_tree.item(item_id, "text")
        values = gui.history_tree.item(item_id, "values")
        tags = gui.history_tree.item(item_id, "tags")
        items.append((item_id, text, values, tags))

    # Define column index mapping
    col_indices = {
        "date": 0,
        "status": 1,
        "resolution": 2,
        "codec": 3,
        "bitrate": 4,
        "duration": 5,
        "audio": 6,
        "input_size": 7,
        "output_size": 8,
        "reduction": 9,
        "vmaf": 10,
        "crf": 11,
    }

    def get_sort_key(item: tuple) -> tuple:
        _, text, values, _ = item
        if col == "#0":
            return (text.lower(),)

        idx = col_indices.get(col, 0)
        val = values[idx] if idx < len(values) else ""

        # Parse numeric values for proper sorting
        if col in ("input_size", "output_size"):
            return (_parse_size(val),)
        if col == "reduction":
            return (_parse_percent(val),)
        if col in ("vmaf", "crf"):
            return (_parse_number(val),)
        if col == "bitrate":
            return (_parse_bitrate(val),)
        if col == "duration":
            return (_parse_duration(val),)
        return (val,)

    items.sort(key=get_sort_key, reverse=gui._history_sort_reverse)

    # Reorder tree
    for i, (item_id, _, _, _) in enumerate(items):
        gui.history_tree.move(item_id, "", i)

    # Update header indicators
    _update_sort_indicators(gui)


def _parse_size(val: str) -> float:
    """Parse file size string to bytes for sorting."""
    if val == "—":
        return -1
    try:
        parts = val.split()
        if len(parts) != 2:
            return -1
        num = float(parts[0])
        unit = parts[1].upper()
        multipliers = {"B": 1, "KB": 1024, "MB": 1024**2, "GB": 1024**3, "TB": 1024**4}
        return num * multipliers.get(unit, 1)
    except (ValueError, IndexError):
        return -1


def _parse_percent(val: str) -> float:
    """Parse percentage string for sorting."""
    if val == "—":
        return -1
    try:
        return float(val.rstrip("%"))
    except ValueError:
        return -1


def _parse_number(val: str) -> float:
    """Parse numeric string for sorting."""
    if val == "—":
        return -1
    try:
        return float(val)
    except ValueError:
        return -1


def _parse_bitrate(val: str) -> float:
    """Parse bitrate string to kbps for sorting."""
    if val == "—":
        return -1
    try:
        parts = val.split()
        if len(parts) != 2:
            return -1
        num = float(parts[0])
        unit = parts[1].lower()
        if "mbps" in unit:
            return num * 1000
        return num
    except (ValueError, IndexError):
        return -1


def _parse_duration(val: str) -> float:
    """Parse duration string to seconds for sorting."""
    if val == "—":
        return -1
    try:
        parts = val.split(":")
        if len(parts) == 3:
            return int(parts[0]) * 3600 + int(parts[1]) * 60 + int(parts[2])
        if len(parts) == 2:
            return int(parts[0]) * 60 + int(parts[1])
        return float(parts[0])
    except (ValueError, IndexError):
        return -1


def _update_sort_indicators(gui: "VideoConverterGUI") -> None:
    """Update column headers to show sort direction."""
    all_columns = [
        "#0", "date", "status", "resolution", "codec", "bitrate",
        "duration", "audio", "input_size", "output_size", "reduction", "vmaf", "crf",
    ]
    for col in all_columns:
        base_text = HISTORY_TREE_HEADINGS.get(col, col.title())
        if col == gui._history_sort_col:
            indicator = " ▼" if gui._history_sort_reverse else " ▲"
            gui.history_tree.heading(col, text=base_text + indicator)
        else:
            gui.history_tree.heading(col, text=base_text)


def _show_context_menu(gui: "VideoConverterGUI", event: tk.Event) -> None:
    """Show right-click context menu."""
    item_id = gui.history_tree.identify_row(event.y)
    if not item_id:
        return

    # Select the item under cursor
    gui.history_tree.selection_set(item_id)

    # Find the record
    record = _get_record_for_tree_item(gui, item_id)
    if not record or not record.original_path:
        # No menu for anonymized files
        return

    menu = tk.Menu(gui.root, tearoff=0)
    menu.add_command(label="Open File", command=lambda: _on_open_file(record))
    menu.add_command(label="Show in Folder", command=lambda: _on_show_in_folder(record))

    try:
        menu.tk_popup(event.x_root, event.y_root)
    finally:
        menu.grab_release()


def _get_record_for_tree_item(gui: "VideoConverterGUI", item_id: str) -> FileRecord | None:
    """Find the FileRecord for a tree item."""
    # Reverse lookup: find path_hash from item_id
    for path_hash, tid in gui._history_tree_map.items():
        if tid == item_id:
            index = get_history_index()
            return index.get(path_hash)
    return None


def _on_open_file(record: FileRecord) -> None:
    """Open the file with the default application."""
    if not record.original_path:
        return

    try:
        if sys.platform == "win32":
            os.startfile(record.original_path)
        elif sys.platform == "darwin":
            subprocess.run(["open", record.original_path], check=False)
        else:
            subprocess.run(["xdg-open", record.original_path], check=False)
    except Exception:
        logger.exception(f"Failed to open file: {record.original_path}")


def _on_show_in_folder(record: FileRecord) -> None:
    """Open the containing folder with the file selected."""
    if not record.original_path:
        return

    try:
        if sys.platform == "win32":
            subprocess.run(["explorer", "/select,", record.original_path], check=False)
        elif sys.platform == "darwin":
            subprocess.run(["open", "-R", record.original_path], check=False)
        else:
            # Linux: open the containing folder
            folder = os.path.dirname(record.original_path)
            subprocess.run(["xdg-open", folder], check=False)
    except Exception:
        logger.exception(f"Failed to show in folder: {record.original_path}")
