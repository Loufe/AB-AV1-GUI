# src/gui/tabs/statistics_tab.py
"""
Statistics tab module for viewing conversion history analysis.

This tab displays:
- Size reduction distribution histogram
- Source codec breakdown pie chart
- Cumulative space saved over time line graph
- Summary statistics panel
"""

import logging
import statistics
import tkinter as tk
from tkinter import ttk
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from src.gui.main_window import VideoConverterGUI

from src.gui.base import ToolTip
from src.gui.charts import BarChart, LineGraph, PieChart
from src.history_index import get_history_index
from src.models import FileRecord
from src.utils import format_file_size

logger = logging.getLogger(__name__)


def create_statistics_tab(gui: "VideoConverterGUI") -> None:
    """Create the statistics tab for viewing conversion history analysis."""
    main = ttk.Frame(gui.statistics_tab)
    main.pack(fill="both", expand=True, padx=10, pady=10)

    # Row weights for layout
    main.columnconfigure(0, weight=1)
    main.columnconfigure(1, weight=1)
    main.rowconfigure(1, weight=1)  # Top charts row
    main.rowconfigure(2, weight=1)  # Line graph row

    # --- Row 0: Controls ---
    controls_frame = ttk.Frame(main)
    controls_frame.grid(row=0, column=0, columnspan=2, sticky="ew", pady=(0, 10))

    gui.refresh_stats_button = ttk.Button(
        controls_frame, text="Refresh Statistics", command=lambda: refresh_statistics(gui)
    )
    gui.refresh_stats_button.pack(side="left")

    gui.stats_status_label = ttk.Label(controls_frame, text="Click 'Refresh Statistics' to load data", anchor="w")
    gui.stats_status_label.pack(side="left", padx=15)

    # --- Row 1: Top charts (bar + pie side by side) ---
    # Bar chart (left)
    bar_frame = ttk.LabelFrame(main, text="Size Reduction Distribution")
    bar_frame.grid(row=1, column=0, sticky="nsew", padx=(0, 5), pady=(0, 5))
    bar_frame.columnconfigure(0, weight=1)
    bar_frame.rowconfigure(0, weight=1)

    gui.histogram_canvas = tk.Canvas(bar_frame, bg="white", highlightthickness=0)
    gui.histogram_canvas.pack(fill="both", expand=True, padx=5, pady=5)
    gui.histogram_chart = BarChart(gui.histogram_canvas, histogram_mode=True, color_gradient=True)

    # Pie chart (right)
    pie_frame = ttk.LabelFrame(main, text="Source Codecs")
    pie_frame.grid(row=1, column=1, sticky="nsew", padx=(5, 0), pady=(0, 5))
    pie_frame.columnconfigure(0, weight=1)
    pie_frame.rowconfigure(0, weight=1)

    gui.codec_canvas = tk.Canvas(pie_frame, bg="white", highlightthickness=0)
    gui.codec_canvas.pack(fill="both", expand=True, padx=5, pady=5)
    gui.codec_chart = PieChart(gui.codec_canvas)

    # --- Row 2: Line graph (full width) ---
    line_frame = ttk.LabelFrame(main, text="Cumulative Space Saved Over Time")
    line_frame.grid(row=2, column=0, columnspan=2, sticky="nsew", pady=(5, 5))
    line_frame.columnconfigure(0, weight=1)
    line_frame.rowconfigure(0, weight=1)

    gui.savings_canvas = tk.Canvas(line_frame, bg="white", highlightthickness=0)
    gui.savings_canvas.pack(fill="both", expand=True, padx=5, pady=5)
    gui.savings_chart = LineGraph(gui.savings_canvas)

    # --- Row 3: Summary statistics panel ---
    _create_summary_panel(gui, main, row=3)


def _create_summary_panel(gui: "VideoConverterGUI", parent: ttk.Frame, row: int) -> None:
    """Create the summary statistics panel."""
    summary_frame = ttk.LabelFrame(parent, text="Summary Statistics")
    summary_frame.grid(row=row, column=0, columnspan=2, sticky="ew", pady=(5, 0))

    stats_grid = ttk.Frame(summary_frame)
    stats_grid.pack(fill="x", padx=10, pady=10)

    # Left column
    left_col = ttk.Frame(stats_grid)
    left_col.pack(side="left", fill="x", expand=True, padx=(0, 20))

    # Total files row
    files_row = ttk.Frame(left_col)
    files_row.grid(row=0, column=0, sticky="w", pady=3)
    ttk.Label(files_row, text="Total Files Converted:").pack(side="left")
    gui.stats_total_files_label = ttk.Label(files_row, text="-")
    gui.stats_total_files_label.pack(side="left", padx=(5, 0))

    # VMAF row
    vmaf_row = ttk.Frame(left_col)
    vmaf_row.grid(row=1, column=0, sticky="w", pady=3)
    vmaf_label = ttk.Label(vmaf_row, text="Avg VMAF Score:")
    vmaf_label.pack(side="left")
    ToolTip(vmaf_label, "Average video quality score (0-100).\n95+ is visually lossless.")
    gui.vmaf_stats_label = ttk.Label(vmaf_row, text="-")
    gui.vmaf_stats_label.pack(side="left", padx=(5, 5))
    gui.vmaf_range_label = ttk.Label(vmaf_row, text="", foreground="#666")
    gui.vmaf_range_label.pack(side="left")

    # CRF row
    crf_row = ttk.Frame(left_col)
    crf_row.grid(row=2, column=0, sticky="w", pady=3)
    crf_label = ttk.Label(crf_row, text="Avg CRF Value:")
    crf_label.pack(side="left")
    ToolTip(crf_label, "Average compression level (0-63).\nLower = higher quality encoding.")
    gui.crf_stats_label = ttk.Label(crf_row, text="-")
    gui.crf_stats_label.pack(side="left", padx=(5, 5))
    gui.crf_range_label = ttk.Label(crf_row, text="", foreground="#666")
    gui.crf_range_label.pack(side="left")

    # Right column
    right_col = ttk.Frame(stats_grid)
    right_col.pack(side="right", fill="x", expand=True, padx=(20, 0))

    # Space saved row
    space_row = ttk.Frame(right_col)
    space_row.grid(row=0, column=0, sticky="w", pady=3)
    ttk.Label(space_row, text="Total Space Saved:").pack(side="left")
    gui.total_saved_label = ttk.Label(space_row, text="-")
    gui.total_saved_label.pack(side="left", padx=(5, 0))

    # Size reduction row
    size_row = ttk.Frame(right_col)
    size_row.grid(row=1, column=0, sticky="w", pady=3)
    ttk.Label(size_row, text="Avg Size Reduction:").pack(side="left")
    gui.size_stats_label = ttk.Label(size_row, text="-")
    gui.size_stats_label.pack(side="left", padx=(5, 5))
    gui.size_range_label = ttk.Label(size_row, text="", foreground="#666")
    gui.size_range_label.pack(side="left")

    # Throughput row
    throughput_row = ttk.Frame(right_col)
    throughput_row.grid(row=2, column=0, sticky="w", pady=3)
    throughput_label = ttk.Label(throughput_row, text="Avg Throughput:")
    throughput_label.pack(side="left")
    ToolTip(
        throughput_label,
        "Encoding speed in GB of source video per hour.\n"
        "Depends on hardware and video complexity.",
    )
    gui.throughput_stats_label = ttk.Label(throughput_row, text="-")
    gui.throughput_stats_label.pack(side="left", padx=(5, 0))

    # Date range row
    date_row = ttk.Frame(right_col)
    date_row.grid(row=3, column=0, sticky="w", pady=3)
    ttk.Label(date_row, text="History Range:").pack(side="left")
    gui.stats_date_range_label = ttk.Label(date_row, text="-")
    gui.stats_date_range_label.pack(side="left", padx=(5, 0))


# --- Data Aggregation Functions ---


def aggregate_size_reduction_histogram(records: list[FileRecord]) -> dict[str, int]:
    """Bucket reduction percentages into 10% ranges.

    Args:
        records: List of converted FileRecord objects.

    Returns:
        Dict mapping bucket labels to counts, e.g., {"0%-10%": 5, "10%-20%": 12, ...}
    """
    buckets = {f"{i * 10}%-{(i + 1) * 10}%": 0 for i in range(10)}

    for record in records:
        if record.reduction_percent is not None:
            pct = record.reduction_percent
            # Clamp to valid range
            pct = max(0, min(pct, 99.99))
            bucket_idx = int(pct // 10)
            bucket_key = f"{bucket_idx * 10}%-{(bucket_idx + 1) * 10}%"
            buckets[bucket_key] += 1

    # Remove empty buckets for cleaner display
    return {k: v for k, v in buckets.items() if v > 0}


def aggregate_codec_distribution(records: list[FileRecord]) -> dict[str, int]:
    """Count files by source video codec.

    Args:
        records: List of converted FileRecord objects.

    Returns:
        Dict mapping codec names to counts, e.g., {"h264": 150, "hevc": 45, ...}
    """
    codecs: dict[str, int] = {}

    for record in records:
        codec = record.video_codec or "unknown"
        codec = codec.lower()
        codecs[codec] = codecs.get(codec, 0) + 1

    # Sort by count descending
    return dict(sorted(codecs.items(), key=lambda x: x[1], reverse=True))


def aggregate_cumulative_savings(records: list[FileRecord]) -> list[tuple[str, float]]:
    """Calculate cumulative space saved over time.

    Args:
        records: List of converted FileRecord objects.

    Returns:
        List of (date_string, cumulative_gb_saved) tuples, sorted by date.
    """
    # Filter to records with valid data
    dated_records = []
    for r in records:
        if r.first_seen and r.file_size_bytes and r.output_size_bytes:
            saved = r.file_size_bytes - r.output_size_bytes
            if saved > 0:
                dated_records.append((r.first_seen[:10], saved))  # Use date only

    if not dated_records:
        return []

    # Sort by date
    dated_records.sort(key=lambda x: x[0])

    # Calculate cumulative sum, grouping by date
    daily_totals: dict[str, int] = {}
    for date_str, saved_bytes in dated_records:
        daily_totals[date_str] = daily_totals.get(date_str, 0) + saved_bytes

    # Convert to cumulative GB
    cumulative = []
    total = 0
    for date_str in sorted(daily_totals.keys()):
        total += daily_totals[date_str]
        cumulative.append((date_str, total / (1024**3)))  # Convert to GB

    return cumulative


def calculate_summary_statistics(records: list[FileRecord]) -> dict[str, Any]:
    """Calculate key summary metrics from conversion history.

    Args:
        records: List of converted FileRecord objects.

    Returns:
        Dict with summary statistics.
    """
    if not records:
        return {}

    total_input = sum(r.file_size_bytes for r in records if r.file_size_bytes)
    total_output = sum(r.output_size_bytes or 0 for r in records)
    total_saved = total_input - total_output
    total_time_sec = sum(r.total_time_sec or 0 for r in records)  # Use property with legacy fallback

    reductions = [r.reduction_percent for r in records if r.reduction_percent is not None]
    vmaf_scores = [r.final_vmaf for r in records if r.final_vmaf is not None]
    crf_values = [r.final_crf for r in records if r.final_crf is not None]

    # Calculate throughput in GB/hr
    gb_per_hour: float | None = None
    if total_time_sec > 0:
        total_gb = total_input / (1024**3)
        total_hours = total_time_sec / 3600
        gb_per_hour = total_gb / total_hours

    dates = [r.first_seen for r in records if r.first_seen]

    return {
        "total_files": len(records),
        "total_input_bytes": total_input,
        "total_output_bytes": total_output,
        "total_saved_bytes": total_saved,
        "total_time_sec": total_time_sec,
        "gb_per_hour": gb_per_hour,
        "reductions": reductions,
        "vmaf_scores": vmaf_scores,
        "crf_values": crf_values,
        "date_range": (min(dates)[:10] if dates else "-", max(dates)[:10] if dates else "-"),
    }


# --- Refresh Logic ---


def refresh_statistics(gui: "VideoConverterGUI") -> None:
    """Load data from history and update all charts."""
    gui.refresh_stats_button.config(state="disabled")
    gui.stats_status_label.config(text="Loading...")
    gui.root.update_idletasks()

    try:
        index = get_history_index()
        records = index.get_converted_records()

        if not records:
            gui.stats_status_label.config(text="No conversion history found")
            _clear_all_charts(gui)
            _clear_summary_labels(gui)
            return

        # Update charts
        histogram_data = aggregate_size_reduction_histogram(records)
        gui.histogram_chart.set_data(histogram_data)

        codec_data = aggregate_codec_distribution(records)
        gui.codec_chart.set_data(codec_data)

        savings_data = aggregate_cumulative_savings(records)
        gui.savings_chart.set_data(savings_data)

        # Update summary
        summary = calculate_summary_statistics(records)
        _update_summary_labels(gui, summary)

        gui.stats_status_label.config(text=f"Loaded {len(records)} conversion records")

    except Exception:
        logger.exception("Error loading statistics")
        gui.stats_status_label.config(text="Error loading statistics - check logs for details")
    finally:
        gui.refresh_stats_button.config(state="normal")


def _clear_all_charts(gui: "VideoConverterGUI") -> None:
    """Clear all chart canvases."""
    gui.histogram_chart.clear()
    gui.codec_chart.clear()
    gui.savings_chart.clear()


def _clear_summary_labels(gui: "VideoConverterGUI") -> None:
    """Reset all summary labels to default."""
    gui.stats_total_files_label.config(text="-")
    gui.vmaf_stats_label.config(text="-")
    gui.vmaf_range_label.config(text="")
    gui.crf_stats_label.config(text="-")
    gui.crf_range_label.config(text="")
    gui.size_stats_label.config(text="-")
    gui.size_range_label.config(text="")
    gui.total_saved_label.config(text="-")
    gui.throughput_stats_label.config(text="-")
    gui.stats_date_range_label.config(text="-")


def _update_summary_labels(gui: "VideoConverterGUI", summary: dict[str, Any]) -> None:
    """Update summary statistic labels."""
    if not summary:
        _clear_summary_labels(gui)
        return

    # Total files
    gui.stats_total_files_label.config(text=str(summary["total_files"]))

    # VMAF stats
    vmaf_scores = summary.get("vmaf_scores", [])
    if vmaf_scores:
        try:
            avg_vmaf = statistics.mean(vmaf_scores)
            min_vmaf = min(vmaf_scores)
            max_vmaf = max(vmaf_scores)
            gui.vmaf_stats_label.config(text=f"{avg_vmaf:.1f}")
            gui.vmaf_range_label.config(text=f"(min: {min_vmaf:.1f}, max: {max_vmaf:.1f})")
        except statistics.StatisticsError:
            gui.vmaf_stats_label.config(text="-")
            gui.vmaf_range_label.config(text="")
    else:
        gui.vmaf_stats_label.config(text="-")
        gui.vmaf_range_label.config(text="")

    # CRF stats
    crf_values = summary.get("crf_values", [])
    if crf_values:
        try:
            avg_crf = statistics.mean(crf_values)
            min_crf = min(crf_values)
            max_crf = max(crf_values)
            gui.crf_stats_label.config(text=f"{avg_crf:.1f}")
            gui.crf_range_label.config(text=f"(min: {min_crf}, max: {max_crf})")
        except statistics.StatisticsError:
            gui.crf_stats_label.config(text="-")
            gui.crf_range_label.config(text="")
    else:
        gui.crf_stats_label.config(text="-")
        gui.crf_range_label.config(text="")

    # Size reduction stats
    reductions = summary.get("reductions", [])
    if reductions:
        try:
            avg_reduction = statistics.mean(reductions)
            min_reduction = min(reductions)
            max_reduction = max(reductions)
            gui.size_stats_label.config(text=f"{avg_reduction:.1f}%")
            gui.size_range_label.config(text=f"(min: {min_reduction:.1f}%, max: {max_reduction:.1f}%)")
        except statistics.StatisticsError:
            gui.size_stats_label.config(text="-")
            gui.size_range_label.config(text="")
    else:
        gui.size_stats_label.config(text="-")
        gui.size_range_label.config(text="")

    # Total space saved
    total_saved = summary.get("total_saved_bytes", 0)
    if total_saved > 0:
        gui.total_saved_label.config(text=format_file_size(total_saved))
    else:
        gui.total_saved_label.config(text="-")

    # Throughput
    gb_per_hour = summary.get("gb_per_hour")
    if gb_per_hour is not None:
        gui.throughput_stats_label.config(text=f"{gb_per_hour:.2f} GB/hr")
    else:
        gui.throughput_stats_label.config(text="-")

    # Date range
    start_date, end_date = summary.get("date_range", ("-", "-"))
    if start_date != "-" and end_date != "-":
        gui.stats_date_range_label.config(text=f"{start_date} to {end_date}")
    else:
        gui.stats_date_range_label.config(text="-")
