"""
Tree formatting and parsing utilities for tree view display.

Pure functions for formatting data (time, size, efficiency) into compact
display strings and parsing those strings back for sorting/comparison.
"""

from src.config import EFFICIENCY_DECIMAL_THRESHOLD


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
        return value * multipliers.get(unit, 1)
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
