# src/gui/charts.py
"""
Pure Tkinter Canvas chart drawing utilities.

No external dependencies - uses only tkinter.Canvas primitives.
"""

import logging
import tkinter as tk
from datetime import date
from typing import Any

logger = logging.getLogger(__name__)

# Color palettes
BAR_COLORS = ["#4e79a7", "#59a14f", "#9c755f", "#f28e2b", "#e15759", "#76b7b2"]
LINE_COLOR = "#4e79a7"
LINE_FILL_COLOR = "#d8e2ed"  # Light version of LINE_COLOR for area fill
PIE_COLORS = [
    "#4e79a7",
    "#f28e2b",
    "#e15759",
    "#76b7b2",
    "#59a14f",
    "#edc948",
    "#b07aa1",
    "#ff9da7",
    "#9c755f",
    "#bab0ac",
]


class BarChart:
    """Canvas-based vertical bar chart."""

    def __init__(
        self, canvas: tk.Canvas, *, histogram_mode: bool = False, color_gradient: bool = False
    ):
        """Initialize bar chart.

        Args:
            canvas: Tkinter canvas to draw on.
            histogram_mode: If True, draw edge labels (for histograms with range buckets).
                Labels should be "start-end" format (e.g., "0-10%"). Edge labels will
                show boundary values between bars instead of centered labels.
            color_gradient: If True, color bars from red (first) to green (last) based
                on position. Useful for histograms where higher values are "better".
        """
        self.canvas = canvas
        self.data: dict[str, int] = {}
        self.histogram_mode = histogram_mode
        self.color_gradient = color_gradient
        self._margin_left = 50
        self._margin_right = 20
        self._margin_top = 20
        self._margin_bottom = 30 if histogram_mode else 50
        self._resize_timer: str | None = None

        # Bind resize
        self.canvas.bind("<Configure>", self._on_resize)

    @staticmethod
    def _gradient_color(ratio: float) -> str:
        """Interpolate from red (0) through yellow (0.5) to green (1)."""
        ratio = max(0.0, min(1.0, ratio))
        if ratio <= 0.5:  # noqa: PLR2004
            # Red to yellow
            r = 255
            g = int(255 * (ratio * 2))
        else:
            # Yellow to green
            r = int(255 * (1 - (ratio - 0.5) * 2))
            g = 255
        return f"#{r:02x}{g:02x}00"

    def set_data(self, data: dict[str, int]) -> None:
        """Set chart data and redraw."""
        self.data = data
        self._draw()

    def clear(self) -> None:
        """Clear the chart."""
        self.data = {}
        try:
            self.canvas.delete("all")
        except tk.TclError as e:
            logger.debug(f"TclError clearing BarChart (widget likely destroyed): {e}")

    def _on_resize(self, event: Any) -> None:
        """Handle canvas resize with debouncing."""
        if self._resize_timer is not None:
            self.canvas.after_cancel(self._resize_timer)
        self._resize_timer = self.canvas.after(100, self._draw)

    def _draw(self) -> None:
        """Draw the bar chart."""
        self._resize_timer = None
        try:
            self.canvas.delete("all")

            width = self.canvas.winfo_width()
            height = self.canvas.winfo_height()

            # Skip rendering if canvas too small for meaningful display
            if width < 100 or height < 100 or not self.data:  # noqa: PLR2004
                if not self.data:
                    self.canvas.create_text(width / 2, height / 2, text="No data", fill="#888", font=("Arial", 10))
                return

            # Calculate drawing area
            chart_left = self._margin_left
            chart_right = width - self._margin_right
            chart_top = self._margin_top
            chart_bottom = height - self._margin_bottom
            chart_width = chart_right - chart_left
            chart_height = chart_bottom - chart_top

            # Minimum chart area needed for readable bars
            if chart_width < 50 or chart_height < 50:  # noqa: PLR2004
                return

            # Get data
            labels = list(self.data.keys())
            values = list(self.data.values())
            max_val = max(values) if values else 1

            # Draw Y-axis
            self.canvas.create_line(chart_left, chart_top, chart_left, chart_bottom, width=1, fill="#666")

            # Draw Y-axis labels (0 and max)
            self.canvas.create_text(chart_left - 5, chart_bottom, text="0", anchor="e", font=("Arial", 8), fill="#666")
            self.canvas.create_text(
                chart_left - 5, chart_top, text=str(max_val), anchor="e", font=("Arial", 8), fill="#666"
            )

            # Draw X-axis
            self.canvas.create_line(chart_left, chart_bottom, chart_right, chart_bottom, width=1, fill="#666")

            # Calculate bar dimensions
            num_bars = len(labels)
            total_bar_area = chart_width / num_bars
            bar_width = total_bar_area * 0.7
            gap = total_bar_area * 0.3

            # Draw bars
            for i, value in enumerate(values):
                x1 = chart_left + i * total_bar_area + gap / 2
                x2 = x1 + bar_width
                bar_height = (value / max_val) * chart_height if max_val > 0 else 0
                y1 = chart_bottom - bar_height
                y2 = chart_bottom

                if self.color_gradient and num_bars > 1:
                    color = self._gradient_color(i / (num_bars - 1))
                else:
                    color = BAR_COLORS[i % len(BAR_COLORS)]
                self.canvas.create_rectangle(x1, y1, x2, y2, fill=color, outline="")

                # Value on top of bar
                if value > 0:
                    label_x = (x1 + x2) / 2
                    self.canvas.create_text(
                        label_x, y1 - 5, text=str(value), anchor="s", font=("Arial", 8), fill="#444"
                    )

            # X-axis labels
            if self.histogram_mode:
                # Histogram mode: draw edge labels at bar boundaries
                # Parse range labels like "40-50%" to extract boundary values
                edge_labels: list[str] = []
                for label in labels:
                    if "-" in label:
                        start, end = label.split("-", 1)
                        if not edge_labels:
                            edge_labels.append(start.strip())
                        edge_labels.append(end.strip())
                    else:
                        # Fallback for non-range labels
                        edge_labels.append(label)

                # Draw edge labels at bar boundaries
                for i, edge_label in enumerate(edge_labels):
                    # Position at the edge between bars
                    x = chart_left + i * total_bar_area + gap / 2
                    self.canvas.create_text(
                        x, chart_bottom + 5, text=edge_label, anchor="n", font=("Arial", 8), fill="#444"
                    )
            else:
                # Standard mode: centered labels under each bar
                for i, label in enumerate(labels):
                    x1 = chart_left + i * total_bar_area + gap / 2
                    x2 = x1 + bar_width
                    label_x = (x1 + x2) / 2
                    self.canvas.create_text(
                        label_x, chart_bottom + 5, text=label, anchor="n", font=("Arial", 8), fill="#444"
                    )

        except tk.TclError as e:
            logger.debug(f"TclError drawing BarChart (widget likely destroyed): {e}")
        except Exception:
            logger.exception("Unexpected error drawing BarChart")


class PieChart:
    """Canvas-based pie chart with legend."""

    def __init__(self, canvas: tk.Canvas):
        self.canvas = canvas
        self.data: dict[str, int] = {}
        self._resize_timer: str | None = None

        # Bind resize
        self.canvas.bind("<Configure>", self._on_resize)

    def set_data(self, data: dict[str, int]) -> None:
        """Set chart data and redraw."""
        self.data = data
        self._draw()

    def clear(self) -> None:
        """Clear the chart."""
        self.data = {}
        try:
            self.canvas.delete("all")
        except tk.TclError as e:
            logger.debug(f"TclError clearing PieChart (widget likely destroyed): {e}")

    def _on_resize(self, event: Any) -> None:
        """Handle canvas resize with debouncing."""
        if self._resize_timer is not None:
            self.canvas.after_cancel(self._resize_timer)
        self._resize_timer = self.canvas.after(100, self._draw)

    def _draw(self) -> None:
        """Draw the pie chart."""
        self._resize_timer = None
        try:
            self.canvas.delete("all")

            width = self.canvas.winfo_width()
            height = self.canvas.winfo_height()

            # Skip rendering if canvas too small for meaningful display
            if width < 100 or height < 100 or not self.data:  # noqa: PLR2004
                if not self.data:
                    self.canvas.create_text(width / 2, height / 2, text="No data", fill="#888", font=("Arial", 10))
                return

            # Calculate pie dimensions
            margin = 20
            legend_width = min(120, width * 0.4)
            pie_area_width = width - legend_width - margin * 2
            pie_size = min(pie_area_width, height - margin * 2)

            # Minimum pie diameter for visible segments
            if pie_size < 50:  # noqa: PLR2004
                return

            cx = margin + pie_size / 2
            cy = height / 2
            radius = pie_size / 2 - 10

            # Calculate total and filter zero values
            total = sum(self.data.values())
            if total == 0:
                self.canvas.create_text(width / 2, height / 2, text="No data", fill="#888", font=("Arial", 10))
                return

            # Draw pie segments - filter out items with zero entries
            start_angle = 90  # Start from top
            items = [(k, v) for k, v in self.data.items() if v > 0]

            for i, (_label, value) in enumerate(items):
                extent = (value / total) * 360
                color = PIE_COLORS[i % len(PIE_COLORS)]

                self.canvas.create_arc(
                    cx - radius,
                    cy - radius,
                    cx + radius,
                    cy + radius,
                    start=start_angle,
                    extent=-extent,  # Negative for clockwise
                    fill=color,
                    outline="white",
                    width=2,
                )

                start_angle -= extent

            # Draw legend
            legend_x = width - legend_width + 10
            legend_y = margin + 10
            line_height = 22

            for i, (label, value) in enumerate(items):
                if legend_y + i * line_height > height - margin:
                    break  # Stop if legend would overflow

                color = PIE_COLORS[i % len(PIE_COLORS)]
                pct = (value / total) * 100

                # Color swatch
                self.canvas.create_rectangle(
                    legend_x,
                    legend_y + i * line_height,
                    legend_x + 14,
                    legend_y + i * line_height + 14,
                    fill=color,
                    outline="",
                )

                # Label text - truncate long codec names for legend fit
                display_label = label[:12] + "..." if len(label) > 12 else label  # noqa: PLR2004
                label_id = self.canvas.create_text(
                    legend_x + 20,
                    legend_y + i * line_height + 7,
                    text=display_label,
                    anchor="w",
                    font=("Arial", 9),
                    fill="#444",
                )
                # Draw percentage in grey after the label
                label_bbox = self.canvas.bbox(label_id)
                if label_bbox:
                    self.canvas.create_text(
                        label_bbox[2] + 3,
                        legend_y + i * line_height + 7,
                        text=f"({pct:.0f}%)",
                        anchor="w",
                        font=("Arial", 9),
                        fill="#999",
                    )

        except tk.TclError as e:
            logger.debug(f"TclError drawing PieChart (widget likely destroyed): {e}")
        except Exception:
            logger.exception("Unexpected error drawing PieChart")


class LineGraph:
    """Canvas-based line graph for time series data."""

    def __init__(self, canvas: tk.Canvas):
        self.canvas = canvas
        self.data: list[tuple[str, float]] = []
        self._margin_left = 60
        self._margin_right = 20
        self._margin_top = 20
        self._margin_bottom = 40
        self._resize_timer: str | None = None

        # Bind resize
        self.canvas.bind("<Configure>", self._on_resize)

    def set_data(self, data: list[tuple[str, float]]) -> None:
        """Set chart data and redraw.

        Args:
            data: List of (date_string, value) tuples, sorted by date.
        """
        self.data = data
        self._draw()

    def clear(self) -> None:
        """Clear the chart."""
        self.data = []
        try:
            self.canvas.delete("all")
        except tk.TclError as e:
            logger.debug(f"TclError clearing LineGraph (widget likely destroyed): {e}")

    def _on_resize(self, event: Any) -> None:
        """Handle canvas resize with debouncing."""
        if self._resize_timer is not None:
            self.canvas.after_cancel(self._resize_timer)
        self._resize_timer = self.canvas.after(100, self._draw)

    def _draw(self) -> None:
        """Draw the line graph."""
        self._resize_timer = None
        try:
            self.canvas.delete("all")

            width = self.canvas.winfo_width()
            height = self.canvas.winfo_height()

            # Skip rendering if canvas too small for meaningful display
            if width < 100 or height < 80 or not self.data:  # noqa: PLR2004
                if not self.data:
                    self.canvas.create_text(width / 2, height / 2, text="No data", fill="#888", font=("Arial", 10))
                return

            # Calculate drawing area
            chart_left = self._margin_left
            chart_right = width - self._margin_right
            chart_top = self._margin_top
            chart_bottom = height - self._margin_bottom
            chart_width = chart_right - chart_left
            chart_height = chart_bottom - chart_top

            # Minimum chart area needed for readable line graph
            if chart_width < 50 or chart_height < 40:  # noqa: PLR2004
                return

            # Get data range
            values = [v for _, v in self.data]
            max_val = max(values) if values else 1

            # Draw axes
            self.canvas.create_line(chart_left, chart_top, chart_left, chart_bottom, width=1, fill="#666")
            self.canvas.create_line(chart_left, chart_bottom, chart_right, chart_bottom, width=1, fill="#666")

            # Y-axis labels - show decimal for small values, integer for large
            self.canvas.create_text(chart_left - 5, chart_bottom, text="0", anchor="e", font=("Arial", 8), fill="#666")
            max_label = f"{max_val:.1f}" if max_val < 100 else f"{max_val:.0f}"  # noqa: PLR2004
            self.canvas.create_text(
                chart_left - 5, chart_top, text=max_label, anchor="e", font=("Arial", 8), fill="#666"
            )

            # Draw Y-axis title
            self.canvas.create_text(15, height / 2, text="GB", anchor="w", font=("Arial", 9), fill="#666")

            # Draw grid lines (horizontal)
            for i in range(1, 4):
                y = chart_bottom - (chart_height * i / 4)
                self.canvas.create_line(chart_left, y, chart_right, y, fill="#eee", dash=(2, 2))

            # Calculate point positions - need at least 2 points for a line
            num_points = len(self.data)
            if num_points < 2:  # noqa: PLR2004
                # Single point - just draw a dot
                if num_points == 1:
                    x = chart_left + chart_width / 2
                    y = chart_bottom - (self.data[0][1] / max_val) * chart_height if max_val > 0 else chart_bottom
                    self.canvas.create_oval(x - 4, y - 4, x + 4, y + 4, fill=LINE_COLOR, outline="")
                return

            # Parse dates for proper time-based x-axis positioning
            parsed_dates: list[date] = []
            for date_str, _ in self.data:
                try:
                    parsed_dates.append(date.fromisoformat(date_str))
                except ValueError:
                    # Fallback: if date parsing fails, use index-based spacing
                    parsed_dates = []
                    break

            # Draw line and points
            points = []
            if parsed_dates:
                # Time-proportional x-axis: position based on actual dates
                first_date = parsed_dates[0]
                last_date = parsed_dates[-1]
                date_range_days = (last_date - first_date).days

                for i, (_, value) in enumerate(self.data):
                    if date_range_days > 0:
                        days_from_start = (parsed_dates[i] - first_date).days
                        x = chart_left + (days_from_start / date_range_days) * chart_width
                    else:
                        # All dates are the same - center the points
                        x = chart_left + chart_width / 2
                    y = chart_bottom - (value / max_val) * chart_height if max_val > 0 else chart_bottom
                    points.append((x, y))
            else:
                # Fallback: evenly space points (original behavior)
                x_step = chart_width / (num_points - 1)
                for i, (_, value) in enumerate(self.data):
                    x = chart_left + i * x_step
                    y = chart_bottom - (value / max_val) * chart_height if max_val > 0 else chart_bottom
                    points.append((x, y))

            # Draw filled area under line
            # Note: Tkinter doesn't support RGBA hex, so use pre-blended light color
            area_points = [(chart_left, chart_bottom)]
            area_points.extend(points)
            area_points.append((chart_right, chart_bottom))
            self.canvas.create_polygon(area_points, fill=LINE_FILL_COLOR, outline="")

            # Draw line
            for i in range(len(points) - 1):
                x1, y1 = points[i]
                x2, y2 = points[i + 1]
                self.canvas.create_line(x1, y1, x2, y2, fill=LINE_COLOR, width=2)

            # Draw point markers only when sparse enough to be distinct
            if num_points <= 50:  # noqa: PLR2004
                for x, y in points:
                    self.canvas.create_oval(x - 3, y - 3, x + 3, y + 3, fill=LINE_COLOR, outline="white")

            # X-axis labels (first and last date)
            # Use anchors that extend inward to prevent clipping at edges
            if self.data:
                first_date_label = self.data[0][0]
                last_date_label = self.data[-1][0]
                self.canvas.create_text(
                    chart_left, chart_bottom + 10, text=first_date_label, anchor="nw", font=("Arial", 8), fill="#666"
                )
                self.canvas.create_text(
                    chart_right, chart_bottom + 10, text=last_date_label, anchor="ne", font=("Arial", 8), fill="#666"
                )

        except tk.TclError as e:
            logger.debug(f"TclError drawing LineGraph (widget likely destroyed): {e}")
        except Exception:
            logger.exception("Unexpected error drawing LineGraph")
