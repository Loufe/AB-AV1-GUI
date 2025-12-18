"""
Data models for the AV1 Video Converter application.

These dataclasses replace ad-hoc dictionaries for type-safe data passing
throughout the conversion pipeline.
"""

from dataclasses import dataclass
from typing import Optional

from src.config import DEFAULT_VMAF_TARGET


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
class HistoryRecord:
    """Record of a completed conversion for history persistence."""

    timestamp: str
    input_file: str
    output_file: str
    input_size_mb: float | None
    output_size_mb: float | None
    reduction_percent: float | None
    duration_sec: float | None
    time_sec: float | None
    input_vcodec: str
    input_acodec: str
    output_acodec: str
    input_codec: str  # Duplicate of input_vcodec for compatibility with estimation
    final_crf: int | None
    final_vmaf: float | None
    final_vmaf_target: int
