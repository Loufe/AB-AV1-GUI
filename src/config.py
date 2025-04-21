# src/config.py
"""
Central configuration constants for the AV1 Video Converter application.
"""

# --- Encoding Settings ---
DEFAULT_VMAF_TARGET = 95          # Target VMAF score for quality-based encoding
DEFAULT_ENCODING_PRESET = "6"     # Corresponds to SVT-AV1 "--preset 6" (Balanced speed/quality)

# --- VMAF Fallback Settings ---
MIN_VMAF_FALLBACK_TARGET = 90     # Minimum VMAF target to attempt if initial target fails
VMAF_FALLBACK_STEP = 1            # How much to decrement VMAF target on each fallback attempt

# --- Resolution Settings ---
MIN_RESOLUTION_WIDTH = 1280       # Minimum width to consider for conversion (720p: 1280x720)
MIN_RESOLUTION_HEIGHT = 720       # Minimum height to consider for conversion