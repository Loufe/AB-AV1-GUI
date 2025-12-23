"""
Data models for the AV1 Video Converter application.

These dataclasses replace ad-hoc dictionaries for type-safe data passing
throughout the conversion pipeline.
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Literal, Optional

from src.config import DEFAULT_VMAF_TARGET


@dataclass
class TimeEstimate:
    """Time estimation with confidence level and optional range.

    For pre-conversion estimates: provides min/max range from percentiles.
    For in-progress estimates: min/max equal best (single value, high confidence).

    Usage:
        estimate = estimate_file_time(file_path)
        if estimate.confidence == "none":
            display("Building estimate...")
        elif estimate.confidence == "high":
            display(f"{estimate.best_seconds}s remaining")
        else:
            display(f"~{estimate.min_seconds}-{estimate.max_seconds}s")
    """

    min_seconds: float  # Optimistic estimate (P25-based or same as best)
    max_seconds: float  # Pessimistic estimate (P75-based or same as best)
    best_seconds: float  # Best guess (P50-based or extrapolated)
    confidence: Literal["high", "medium", "low", "none"]
    source: str  # Origin: "similar_file", "codec:h264", "global", "in_progress", "insufficient_data"


class FileStatus(str, Enum):
    """Status of a file in the history system.

    Inherits from str for easy JSON serialization.
    """

    SCANNED = "scanned"  # Metadata only (Layer 1 analysis)
    NOT_WORTHWHILE = "not_worthwhile"  # VMAF analysis showed conversion isn't beneficial
    CONVERTED = "converted"  # Successfully converted


@dataclass
class FileRecord:
    """Universal record for any file in the history system.

    Supports all file states: scanned (metadata only), not_worthwhile (VMAF
    analysis showed no benefit), and converted (successful conversion).

    The path_hash is the primary key for lookups. original_path is only stored
    if anonymization is disabled at the time of recording.
    """

    # === Identity ===
    path_hash: str  # BLAKE2b hash of normalized path (primary key)
    original_path: str | None  # Full path if anonymization OFF, else None
    status: FileStatus

    # === Cache Validation ===
    file_size_bytes: int  # Used to detect if file changed
    file_mtime: float  # Used to detect if file changed

    # === Video Metadata (from ffprobe, Layer 1) ===
    duration_sec: float | None = None
    video_codec: str | None = None
    audio_codec: str | None = None
    width: int | None = None
    height: int | None = None
    bitrate_kbps: float | None = None

    # === Estimation (Layer 1) ===
    estimated_reduction_percent: float | None = None  # Based on similar files
    estimated_from_similar: int | None = None  # Count of similar files used for estimate

    # === VMAF Analysis (Layer 2 - CRF search results) ===
    vmaf_target_when_analyzed: int | None = None  # VMAF target used when Layer 2 analysis was performed
    best_crf: int | None = None  # CRF that gave best VMAF (from crf-search)
    best_vmaf_achieved: float | None = None  # Best VMAF score we could achieve (from crf-search)
    predicted_output_size: int | None = None  # Predicted output size in bytes (from crf-search)
    predicted_size_reduction: float | None = None  # Predicted size reduction % (from crf-search)

    # === For not_worthwhile status (failed CRF search) ===
    vmaf_target_attempted: int | None = None  # Target VMAF we tried to achieve
    min_vmaf_attempted: int | None = None  # Lowest VMAF target we tried (e.g., 90)
    skip_reason: str | None = None  # Why conversion was skipped

    # === Conversion Results (for converted status) ===
    output_path: str | None = None  # Path or hash depending on anonymization
    output_size_bytes: int | None = None
    reduction_percent: float | None = None  # Actual reduction (not estimated)
    conversion_time_sec: float | None = None
    final_crf: int | None = None
    final_vmaf: float | None = None
    vmaf_target_used: int | None = None
    output_audio_codec: str | None = None

    # === Timestamps ===
    first_seen: str | None = None  # ISO timestamp when first scanned
    last_updated: str | None = None  # ISO timestamp of last status change


@dataclass
class ProgressEvent:
    """Progress update event during video conversion.

    Fields are optional based on the conversion phase:
    - CRF search phase: vmaf, crf may be present; encoding progress is 0
    - Encoding phase: encoding_percent increases; eta_text may be present
    """

    # Core progress tracking
    progress_quality: float = 0.0  # 0-100, quality detection progress
    progress_encoding: float = 0.0  # 0-100, encoding progress
    phase: str = "crf-search"  # "crf-search" or "encoding"
    message: str = ""  # Human-readable status message

    # Quality metrics (available during/after CRF search)
    vmaf: Optional[float] = None  # VMAF score
    crf: Optional[int] = None  # CRF value
    vmaf_target_used: Optional[int] = None  # Target VMAF for this attempt
    used_fallback: Optional[bool] = None  # Whether fallback VMAF was used (unused in progress)

    # Size predictions
    size_reduction: Optional[float] = None  # Predicted size reduction percentage
    original_size: Optional[int] = None  # Original file size in bytes
    output_size: Optional[int] = None  # Estimated/actual output size in bytes
    is_estimate: Optional[bool] = None  # Whether output_size is an estimate

    # Encoding phase timing
    eta_text: Optional[str] = None  # ETA string from ab-av1 (e.g., "5m 30s")

    # Additional metadata
    file_size_mb: Optional[float] = None  # File size in MB (for initial file info)


@dataclass
class FileInfoEvent:
    """Initial file information sent at start of processing."""

    file_size_mb: float


@dataclass
class ConversionResult:
    """Result of a completed video conversion.

    Contains all metadata needed for history recording and statistics.
    """

    # File paths
    input_path: str
    output_path: str

    # Timing
    elapsed_seconds: float

    # Sizes
    input_size_bytes: int
    output_size_bytes: int

    # Quality metrics
    final_crf: int
    final_vmaf: float
    final_vmaf_target: int = DEFAULT_VMAF_TARGET  # Target VMAF used (may differ from default)

    # Calculated metrics (optional, can be computed from other fields)
    reduction_percent: Optional[float] = None


@dataclass
class ErrorInfo:
    """Information about a conversion error."""

    message: str
    error_type: str = "unknown"
    details: str = ""
    stack_trace: Optional[str] = None


@dataclass
class RetryInfo:
    """Information about a retry attempt with fallback VMAF."""

    message: str
    fallback_vmaf: Optional[int] = None  # The VMAF target being attempted


@dataclass
class SkippedInfo:
    """Information about a skipped file."""

    message: str
    original_size: Optional[int] = None
    min_vmaf_attempted: Optional[int] = None


@dataclass
class ConversionConfig:
    """Configuration for a batch conversion job.

    Consolidates all settings needed by the conversion worker.
    """

    # Folder paths
    input_folder: str
    output_folder: str

    # File selection
    extensions: list[str]  # e.g., ["mp4", "mkv"]

    # Conversion behavior
    overwrite: bool
    delete_original: bool

    # Audio settings
    convert_audio: bool
    audio_codec: str  # e.g., "opus", "aac"


@dataclass
class ConversionSessionState:
    """All mutable state for an active conversion session.

    Always initialized (never None). Reset to defaults between conversions.
    Eliminates dynamic attribute assignment and hasattr checks.

    Note: Thread and Event objects stay on MainWindow, not in this dataclass,
    to avoid serialization issues and keep synchronization primitives separate.
    """

    # === Core State ===
    running: bool = False
    sleep_prevention_active: bool = False

    # === File Lists ===
    video_files: list[str] = field(default_factory=list)
    pending_files: list[str] = field(default_factory=list)
    output_folder_path: str = ""

    # === Progress Counters ===
    processed_files: int = 0
    successful_conversions: int = 0
    error_count: int = 0
    skipped_not_worth_count: int = 0
    skipped_low_resolution_count: int = 0

    # === Timing ===
    total_start_time: float | None = None
    current_file_start_time: float | None = None
    current_file_encoding_start_time: float | None = None
    elapsed_timer_id: int | None = None  # Tkinter after() timer ID

    # === ETA Tracking (use None instead of deleting attributes) ===
    last_encoding_progress: float = 0.0
    last_eta_seconds: float | None = None
    last_eta_timestamp: float | None = None

    # === Per-File State ===
    current_file_path: str | None = None
    current_process_info: dict[str, Any] | None = None
    last_input_size: int | None = None
    last_output_size: int | None = None
    last_elapsed_time: float | None = None
    last_skip_reason: str | None = None
    last_min_vmaf_attempted: int | None = None

    # === Statistics Accumulators ===
    vmaf_scores: list[float] = field(default_factory=list)
    crf_values: list[int] = field(default_factory=list)
    size_reductions: list[float] = field(default_factory=list)
    total_input_bytes_success: int = 0
    total_output_bytes_success: int = 0
    total_time_success: float = 0.0

    # === Tracking Lists ===
    error_details: list[dict[str, Any]] = field(default_factory=list)
    skipped_not_worth_files: list[str] = field(default_factory=list)
    skipped_low_resolution_files: list[str] = field(default_factory=list)
