# src/config.py
"""
Central configuration constants for the AV1 Video Converter application.
"""

from typing import TypedDict

# --- Application Version ---
try:
    import tomllib
    from pathlib import Path

    _pyproject = Path(__file__).parent.parent / "pyproject.toml"
    with open(_pyproject, "rb") as _f:
        APP_VERSION = tomllib.load(_f)["project"]["version"]
except Exception:
    APP_VERSION = "dev"

# --- Encoding Settings ---
DEFAULT_VMAF_TARGET = 95  # Target VMAF score for quality-based encoding
DEFAULT_ENCODING_PRESET = 6  # Corresponds to SVT-AV1 "--preset 6" (Balanced speed/quality)

# --- VMAF Fallback Settings ---
MIN_VMAF_FALLBACK_TARGET = 90  # Minimum VMAF target to attempt if initial target fails
VMAF_FALLBACK_STEP = 1  # How much to decrement VMAF target on each fallback attempt

# --- Progress Logging ---
# Dynamic log interval tiers based on estimated duration (requires ab-av1 with --log-interval support)
# Format: (max_duration_minutes, log_interval) - uses first matching tier
# Set to None to disable (falls back to ab-av1's exponential backoff)
LOG_INTERVAL_TIERS: list[tuple[int | None, str]] = [
    (30, "5%"),    # < 30 min: every 5% (~20 updates)
    (120, "2%"),   # 30 min - 2 hr: every 2% (~50 updates)
    (240, "1%"),   # 2 - 4 hr: every 1% (~100 updates)
    (None, "1%"),  # > 4 hr: every 1% (~100 updates)
]

# --- Resolution Settings ---
MIN_RESOLUTION_WIDTH = 1280  # Minimum width to consider for conversion (720p: 1280x720)
MIN_RESOLUTION_HEIGHT = 720  # Minimum height to consider for conversion

# --- Parsing Thresholds ---
SIZE_REDUCTION_CHANGE_THRESHOLD = 0.1  # Percentage change to trigger size reduction update
VMAF_CHANGE_THRESHOLD = 0.01  # VMAF score difference to trigger update

# --- File Validation ---
MIN_OUTPUT_FILE_SIZE = 1024  # Minimum bytes for valid output file (1 KB)

# --- Record Validation ---
MAX_CRF_VALUE = 63  # Maximum valid CRF value for AV1/HEVC/H264
MAX_VMAF_VALUE = 100  # Maximum valid VMAF score

# --- History File ---
HISTORY_FILE = "conversion_history.json"

# --- Settings File ---
CONFIG_FILE = "ab_av1_gui_config.json"


class ConfigDict(TypedDict):
    """Type-safe configuration dictionary."""

    input_folder: str
    output_folder: str
    log_folder: str
    overwrite: bool
    ext_mp4: bool
    ext_mkv: bool
    ext_avi: bool
    ext_wmv: bool
    convert_audio: bool
    audio_codec: str
    anonymize_logs: bool
    anonymize_history: bool
    hw_decode_enabled: bool
    default_output_mode: str
    default_suffix: str
    default_output_folder: str


# Default configuration values (used for merging with loaded config)
CONFIG_DEFAULTS: ConfigDict = {
    "input_folder": "",
    "output_folder": "",
    "log_folder": "",
    "overwrite": False,
    "ext_mp4": True,
    "ext_mkv": True,
    "ext_avi": True,
    "ext_wmv": True,
    "convert_audio": True,
    "audio_codec": "opus",
    "anonymize_logs": True,
    "anonymize_history": False,
    "hw_decode_enabled": True,
    "default_output_mode": "replace",
    "default_suffix": "_av1",
    "default_output_folder": "",
}

# --- UI Batching ---
TREE_UPDATE_BATCH_SIZE = 50  # Number of items to batch before updating UI
MIN_FILES_FOR_PERCENT_UPDATES = 20  # Minimum files before using percentage-based update intervals

# --- Time Estimation ---
MIN_SAMPLES_FOR_ESTIMATE = 5  # Minimum conversion history samples needed for estimates
MIN_SAMPLES_HIGH_CONFIDENCE = 10  # Samples needed for "high" vs "medium" confidence
DEFAULT_REDUCTION_ESTIMATE_PERCENT = 45.0  # Default file size reduction estimate if no history data
RESOLUTION_TOLERANCE_PERCENT = 0.2  # Tolerance for resolution matching (20%)

# --- Duplicate Detection ---
# Tolerance for duration matching - must account for rounding differences between code paths:
# - folder_analysis.py stores raw ffprobe duration (e.g., 384.533313)
# - worker.py rounds to 1 decimal (e.g., 384.5)
# A tolerance of 0.1 safely covers rounding while avoiding false positives on different files.
DURATION_TOLERANCE_SEC = 0.1

# --- Tree Display Formatting ---
EFFICIENCY_DECIMAL_THRESHOLD = 10  # Show GB/hr without decimals above this value

# Analysis tree column headings (base text without sort indicators)
ANALYSIS_TREE_HEADINGS: dict[str, str] = {
    "#0": "Name",
    "format": "Format",
    "size": "Size",
    "savings": "Est. Savings",
    "time": "Est. Time",
    "efficiency": "Efficiency",
}

# History tree column headings
HISTORY_TREE_HEADINGS: dict[str, str] = {
    "date": "Date",
    "#0": "Name",
    "status": "Status",
    "resolution": "Resolution",
    "codec": "Codec",
    "bitrate": "Bitrate",
    "duration": "Duration",
    "audio": "Audio",
    "input_size": "Input",
    "output_size": "Output",
    "reduction": "Reduction",
    "vmaf": "VMAF",
    "crf": "CRF",
}

# --- Hardware Decoder Settings ---
# Hardware decoder mapping (source codec -> preferred decoders in priority order)
HW_DECODER_MAP: dict[str, list[str]] = {
    "h264": ["h264_cuvid", "h264_qsv"],
    "hevc": ["hevc_cuvid", "hevc_qsv"],
    "vp9": ["vp9_cuvid", "vp9_qsv"],
    "av1": ["av1_cuvid", "av1_qsv"],
}
