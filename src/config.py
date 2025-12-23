# src/config.py
"""
Central configuration constants for the AV1 Video Converter application.
"""

# --- Encoding Settings ---
DEFAULT_VMAF_TARGET = 95  # Target VMAF score for quality-based encoding
DEFAULT_ENCODING_PRESET = 6  # Corresponds to SVT-AV1 "--preset 6" (Balanced speed/quality)

# --- VMAF Fallback Settings ---
MIN_VMAF_FALLBACK_TARGET = 90  # Minimum VMAF target to attempt if initial target fails
VMAF_FALLBACK_STEP = 1  # How much to decrement VMAF target on each fallback attempt

# --- Resolution Settings ---
MIN_RESOLUTION_WIDTH = 1280  # Minimum width to consider for conversion (720p: 1280x720)
MIN_RESOLUTION_HEIGHT = 720  # Minimum height to consider for conversion

# --- Parsing Thresholds ---
SIZE_REDUCTION_CHANGE_THRESHOLD = 0.1  # Percentage change to trigger size reduction update
VMAF_CHANGE_THRESHOLD = 0.01  # VMAF score difference to trigger update

# --- File Validation ---
MIN_OUTPUT_FILE_SIZE = 1024  # Minimum bytes for valid output file (1 KB)

# --- History File ---
HISTORY_FILE_V2 = "conversion_history_v2.json"  # New unified history format

# --- UI Batching ---
TREE_UPDATE_BATCH_SIZE = 50  # Number of items to batch before updating UI
MIN_FILES_FOR_PERCENT_UPDATES = 20  # Minimum files before using percentage-based update intervals

# --- Time Estimation ---
MIN_SAMPLES_FOR_ESTIMATE = 5  # Minimum conversion history samples needed for estimates
MIN_SAMPLES_FOR_QUARTILES = 4  # Minimum samples required by statistics.quantiles(n=4)
DEFAULT_REDUCTION_ESTIMATE_PERCENT = 45.0  # Default file size reduction estimate if no history data
RESOLUTION_TOLERANCE_PERCENT = 0.2  # Tolerance for resolution matching (20%)
