# src/gui/constants.py
"""
Centralized UI constants for colors and fonts.

This module provides semantic naming for all UI styling constants,
making it easier to maintain consistent appearance and update themes.
"""

# =============================================================================
# COLORS - Background
# =============================================================================
COLOR_BACKGROUND = "#f0f0f0"  # Main application background (ttk frames, labels)
COLOR_BACKGROUND_WHITE = "#ffffff"  # Context menus, tooltips

# =============================================================================
# COLORS - Status Indicators
# =============================================================================
COLOR_STATUS_SUCCESS = "#2E7D32"  # Dark green - completed, success, up-to-date
COLOR_STATUS_SUCCESS_LIGHT = "#008000"  # Standard green - HW decoder available
COLOR_STATUS_WARNING = "#C65D00"  # Muted amber - skipped, not worthwhile
COLOR_STATUS_ERROR = "#C62828"  # Dark red - errors, failures
COLOR_STATUS_ERROR_TEXT = "#c00000"  # Red text for warnings/errors in dialogs
COLOR_STATUS_INFO = "#1565C0"  # Dark blue - converting, queued, update available
COLOR_STATUS_INFO_LIGHT = "#64B5F6"  # Light blue - partial queue (folder with some queued)
COLOR_STATUS_PENDING = "#000000"  # Black - pending items, neutral/default
COLOR_STATUS_DISABLED = "#888888"  # Gray - already AV1, no action needed
COLOR_STATUS_NEUTRAL = "#808080"  # Gray - HW decoder not detected, unknown status

# =============================================================================
# COLORS - Context Menu
# =============================================================================
COLOR_MENU_BACKGROUND = COLOR_BACKGROUND_WHITE
COLOR_MENU_ACTIVE_BG = "#0078d4"  # Windows accent blue
COLOR_MENU_ACTIVE_FG = "#ffffff"  # White text on active

# =============================================================================
# COLORS - Tooltip
# =============================================================================
COLOR_TOOLTIP_BACKGROUND = "lightyellow"

# =============================================================================
# COLORS - Scanning Badge
# =============================================================================
COLOR_BADGE_BACKGROUND = "#505050"  # Dark gray for floating badge
COLOR_BADGE_TEXT = "#f0f0f0"  # Light text on dark background

# =============================================================================
# COLORS - Text Colors (foreground parameter)
# =============================================================================
COLOR_TEXT_MUTED = "#606060"  # Dark gray for range labels

# =============================================================================
# COLORS - Chart Palettes
# =============================================================================
# Primary bar colors (6-color categorical palette)
CHART_BAR_COLORS = ["#4e79a7", "#59a14f", "#9c755f", "#f28e2b", "#e15759", "#76b7b2"]

# Line chart colors
CHART_LINE_COLOR = "#4e79a7"  # Primary line color
CHART_LINE_FILL_COLOR = "#d8e2ed"  # Light version for area fill

# Pie chart colors (10-color categorical palette)
CHART_PIE_COLORS = [
    "#4e79a7",  # Blue
    "#f28e2b",  # Orange
    "#e15759",  # Red
    "#76b7b2",  # Teal
    "#59a14f",  # Green
    "#edc948",  # Yellow
    "#b07aa1",  # Purple
    "#ff9da7",  # Pink
    "#9c755f",  # Brown
    "#bab0ac",  # Gray
]

# Chart text colors
CHART_TEXT_NO_DATA = "#888"  # "No data" placeholder text
CHART_TEXT_AXIS = "#666"  # Axis labels and ticks
CHART_TEXT_VALUE = "#444"  # Values on bars, labels
CHART_TEXT_LEGEND = "#444"  # Legend labels
CHART_TEXT_LEGEND_PCT = "#999"  # Legend percentage text

# =============================================================================
# FONTS - Family
# =============================================================================
FONT_FAMILY_DEFAULT = "Arial"
FONT_FAMILY_MONOSPACE = "Consolas"
FONT_FAMILY_SYSTEM = "TkDefaultFont"  # System default (for version labels, etc.)

# =============================================================================
# FONTS - Sizes
# =============================================================================
FONT_SIZE_SMALL = 8  # Chart axis labels, small text
FONT_SIZE_NORMAL = 9  # Checkbox labels, secondary text
FONT_SIZE_BODY = 10  # Main body text, labels
FONT_SIZE_TAB = 11  # Tab headers, overlay text

# =============================================================================
# FONTS - Complete Tuples (family, size, weight)
# =============================================================================
FONT_BODY = (FONT_FAMILY_DEFAULT, FONT_SIZE_BODY)
FONT_BODY_BOLD = (FONT_FAMILY_DEFAULT, FONT_SIZE_BODY, "bold")
FONT_SMALL = (FONT_FAMILY_DEFAULT, FONT_SIZE_NORMAL)
FONT_TAB = (FONT_FAMILY_DEFAULT, FONT_SIZE_TAB)

# Chart fonts
FONT_CHART_AXIS = (FONT_FAMILY_DEFAULT, FONT_SIZE_SMALL)
FONT_CHART_LABEL = (FONT_FAMILY_DEFAULT, FONT_SIZE_NORMAL)
FONT_CHART_NO_DATA = (FONT_FAMILY_DEFAULT, FONT_SIZE_BODY)

# System font variations
FONT_SYSTEM_BOLD = (FONT_FAMILY_SYSTEM, FONT_SIZE_NORMAL, "bold")
FONT_SYSTEM_NORMAL = (FONT_FAMILY_SYSTEM, FONT_SIZE_NORMAL)
FONT_SYSTEM_UNDERLINE = (FONT_FAMILY_SYSTEM, FONT_SIZE_NORMAL, "underline")
FONT_SYSTEM_OVERLAY = (FONT_FAMILY_SYSTEM, FONT_SIZE_TAB)
FONT_DIALOG_HEADER = (FONT_FAMILY_SYSTEM, FONT_SIZE_BODY, "bold")
FONT_MONOSPACE = (FONT_FAMILY_MONOSPACE, FONT_SIZE_NORMAL)

# =============================================================================
# LAYOUT - Sizing Constants
# =============================================================================
# Scrollbar width for aligning total rows with scrollable trees above them.
# This is approximate and may vary slightly by OS/theme (typically 15-20px on Windows).
SCROLLBAR_WIDTH_PADDING = 17

# =============================================================================
# TOOLTIPS - Column Header Tooltips
# =============================================================================
TOOLTIP_TIME_COLUMN = (
    "Estimated conversion time.\n"
    "No prefix = precise (from CRF analysis or similar file).\n"
    "'~' prefix = medium confidence (similar file match).\n"
    "'~~' prefix = low confidence (statistical estimate)."
)

# =============================================================================
# OUTPUT MODE - Display Labels
# =============================================================================
# Mapping between human-readable display strings and internal enum values.
# Used by the queue tab's output mode dropdown.
OUTPUT_MODE_DISPLAY_TO_VALUE = {
    "Replace Original": "replace",
    "Add Suffix": "suffix",
    "Separate Folder": "separate_folder",
}
OUTPUT_MODE_VALUE_TO_DISPLAY = {v: k for k, v in OUTPUT_MODE_DISPLAY_TO_VALUE.items()}
