"""
Tree formatting and parsing utilities for tree view display.

Pure functions for formatting data (time, size, efficiency) into compact
display strings and parsing those strings back for sorting/comparison.
"""

import logging

from src.config import ANALYSIS_TREE_HEADINGS, EFFICIENCY_DECIMAL_THRESHOLD

logger = logging.getLogger(__name__)


def format_compact_time(seconds: float) -> str:
    """Format time in a compact way for the analysis tree.

    Args:
        seconds: Time in seconds

    Returns:
        Formatted string like "2h 15m", "45m", "12m"
    """
    if seconds <= 0:
        return "—"

    hours = int(seconds // 3600)
    minutes = int((seconds % 3600) // 60)

    if hours > 0:
        return f"{hours}h {minutes}m"
    if minutes > 0:
        return f"{minutes}m"
    return "< 1m"


def format_efficiency(savings_bytes: int, time_seconds: float) -> str:
    """Format efficiency (savings per time) for display.

    Args:
        savings_bytes: Estimated savings in bytes
        time_seconds: Estimated conversion time in seconds

    Returns:
        Formatted string like "2.5 GB/h", "12 GB/h", or "—"
    """
    if savings_bytes <= 0 or time_seconds <= 0:
        return "—"

    # Calculate GB saved per hour
    gb_per_hr = (savings_bytes / 1_073_741_824) / (time_seconds / 3600)

    # No decimals for >= 10 GB/h
    if gb_per_hr >= EFFICIENCY_DECIMAL_THRESHOLD:
        return f"{gb_per_hr:.0f} GB/h"

    # Show one decimal for smaller values
    return f"{gb_per_hr:.1f} GB/h"


def parse_size_to_bytes(size_str: str) -> float:
    """Parse formatted size string to bytes for sorting.

    Args:
        size_str: Formatted size like "~1.2 GB", "500 MB", or "—"

    Returns:
        Size in bytes, or float('inf') for "—"
    """
    if size_str == "—":
        return float("inf")

    # Remove ~ prefix if present
    size_str = size_str.lstrip("~").strip()

    # Parse value and unit
    parts = size_str.split()
    expected_parts = 2
    if len(parts) != expected_parts:
        return float("inf")

    try:
        value = float(parts[0])
        unit = parts[1].upper()

        # Convert to bytes
        multipliers = {"B": 1, "KB": 1024, "MB": 1024**2, "GB": 1024**3, "TB": 1024**4}
        if unit not in multipliers:
            return float("inf")
        return value * multipliers[unit]
    except (ValueError, KeyError):
        return float("inf")


def parse_time_to_seconds(time_str: str) -> float:
    """Parse formatted time string to seconds for sorting.

    Args:
        time_str: Formatted time like "2h 15m", "45m", or "—"

    Returns:
        Time in seconds, or float('inf') for "—"
    """
    if time_str in {"—", "< 1m"}:
        return float("inf") if time_str == "—" else 30  # Treat "< 1m" as 30 seconds

    total_seconds = 0.0
    # Parse patterns like "2h 15m" or "45m"
    parts = time_str.split()
    for part in parts:
        try:
            if part.endswith("h"):
                total_seconds += float(part[:-1]) * 3600
            elif part.endswith("m"):
                total_seconds += float(part[:-1]) * 60
        except ValueError:
            continue

    return total_seconds if total_seconds > 0 else float("inf")


def parse_efficiency_to_value(eff_str: str) -> float:
    """Parse formatted efficiency string to numeric value for sorting.

    Args:
        eff_str: Formatted efficiency like "2.5 GB/h", "12 GB/h", or "—"

    Returns:
        Efficiency in GB/hr, or float('-inf') for "—" (sorts last when descending)
    """
    if eff_str == "—":
        return float("-inf")  # Sort "—" last when sorting by efficiency (descending)

    try:
        parts = eff_str.split()
        expected_parts = 2
        if len(parts) != expected_parts:
            return float("-inf")

        value = float(parts[0])
        unit = parts[1]

        if unit == "GB/h":
            return value
        return float("-inf")
    except (ValueError, IndexError):
        return float("-inf")


def sort_analysis_tree(gui, col: str, descending: bool | None = None):
    """Sort the analysis tree by the specified column.

    Sorting is done within each parent (preserves hierarchy).
    Folders sort before files when sorting by Name.
    Toggle direction on repeated clicks (when descending is None).

    Args:
        gui: The GUI instance (VideoConverterGUI)
        col: Column to sort by ("#0", "size", "savings", "time", or "efficiency")
        descending: If specified, force this direction. If None, toggle on repeat click.
    """
    if descending is not None:
        # Programmatic sort with explicit direction
        gui._sort_col = col  # noqa: SLF001 - accessing GUI internal state
        gui._sort_reverse = descending  # noqa: SLF001 - accessing GUI internal state
    elif gui._sort_col == col:  # noqa: SLF001 - accessing GUI internal state
        # Toggle direction if same column clicked
        gui._sort_reverse = not gui._sort_reverse  # noqa: SLF001 - accessing GUI internal state
    else:
        gui._sort_col = col  # noqa: SLF001 - accessing GUI internal state
        gui._sort_reverse = False  # noqa: SLF001 - accessing GUI internal state

    def get_sort_key(item_id: str) -> tuple:
        """Get sort key for an item.

        Returns tuple: (is_file, sort_value)
        - Folders always sort before files (is_file=False for folders)
        - Sort value depends on column
        """
        # Check if item is a file (has parent) or folder (no parent or root)
        parent = gui.analysis_tree.parent(item_id)
        is_file = bool(parent)

        if col == "#0":
            # Sort by name
            text = gui.analysis_tree.item(item_id, "text")
            # Remove arrows, icons, and leading spaces
            name = text.replace("▶", "").replace("▼", "").replace("📁", "").replace("🎬", "").strip()
            return (is_file, name.lower())
        if col == "format":
            # Sort by format string (values[0])
            values = gui.analysis_tree.item(item_id, "values")
            if values and len(values) >= 1:
                return (is_file, values[0].lower() if values[0] else "")
            return (is_file, "")
        if col == "size":
            # Sort by file size (values[1])
            values = gui.analysis_tree.item(item_id, "values")
            if values and len(values) >= 2:  # noqa: PLR2004 - column index bounds check
                size_bytes = parse_size_to_bytes(values[1])
                return (is_file, size_bytes)
            return (is_file, float("inf"))
        if col == "savings":
            # Sort by estimated savings (values[2])
            values = gui.analysis_tree.item(item_id, "values")
            if values and len(values) >= 3:  # noqa: PLR2004 - column index bounds check
                size_bytes = parse_size_to_bytes(values[2])
                return (is_file, size_bytes)
            return (is_file, float("inf"))
        if col == "time":
            # Sort by estimated time (values[3])
            values = gui.analysis_tree.item(item_id, "values")
            if values and len(values) >= 4:  # noqa: PLR2004 - column index bounds check
                time_seconds = parse_time_to_seconds(values[3])
                return (is_file, time_seconds)
            return (is_file, float("inf"))
        if col == "efficiency":
            # Sort by efficiency (values[4], higher is better, so negate for default ascending sort)
            values = gui.analysis_tree.item(item_id, "values")
            if values and len(values) >= 5:  # noqa: PLR2004 - column index bounds check
                eff_value = parse_efficiency_to_value(values[4])
                # Negate so higher efficiency sorts first in ascending order
                return (is_file, -eff_value)
            return (is_file, float("inf"))
        return (is_file, "")

    def sort_children(parent_id: str):
        """Sort children of a parent node recursively."""
        children = list(gui.analysis_tree.get_children(parent_id))
        if not children:
            return

        # Sort children
        children_sorted = sorted(children, key=get_sort_key, reverse=gui._sort_reverse)  # noqa: SLF001

        # Reorder in tree
        for index, item_id in enumerate(children_sorted):
            gui.analysis_tree.move(item_id, parent_id, index)

        # Recursively sort children of each child (for folders)
        for child_id in children_sorted:
            if gui.analysis_tree.get_children(child_id):  # Has children (is a folder)
                sort_children(child_id)

    # Sort root level items and their children recursively
    sort_children("")

    # Update column headers to show sort indicator
    update_sort_indicators(gui)

    logger.debug(f"Sorted analysis tree by {col}, reverse={gui._sort_reverse}")  # noqa: SLF001


def update_sort_indicators(gui):
    """Update column headers to show sort direction indicator.

    Args:
        gui: The GUI instance (VideoConverterGUI)
    """
    indicator = " ▼" if gui._sort_reverse else " ▲"  # noqa: SLF001

    for col, base_text in ANALYSIS_TREE_HEADINGS.items():
        if col == gui._sort_col:  # noqa: SLF001
            gui.analysis_tree.heading(col, text=base_text + indicator)
        else:
            gui.analysis_tree.heading(col, text=base_text)


def clear_sort_state(gui):
    """Clear sort state and reset column headers.

    Args:
        gui: The GUI instance (VideoConverterGUI)
    """
    gui._sort_col = None  # noqa: SLF001
    gui._sort_reverse = False  # noqa: SLF001
    for col, base_text in ANALYSIS_TREE_HEADINGS.items():
        gui.analysis_tree.heading(col, text=base_text)
