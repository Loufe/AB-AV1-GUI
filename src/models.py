"""
Data models for the AV1 Video Converter application.

These dataclasses replace ad-hoc dictionaries for type-safe data passing
throughout the conversion pipeline.
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Literal


@dataclass
class AudioStreamInfo:
    """Information about a single audio stream.

    Extracted from ffprobe stream data. Used to track all audio streams
    in a file rather than just the first one.

    Serialization: Use dataclasses.asdict() to convert to dict.
    Deserialization: Use AudioStreamInfo.from_dict() to create from dict.
    """

    codec: str  # e.g., "aac", "ac3", "opus"
    language: str | None = None  # e.g., "eng", "jpn", None
    title: str | None = None  # e.g., "English 5.1", "Commentary"
    channels: int | None = None  # e.g., 2 (stereo), 6 (5.1)
    sample_rate: int | None = None  # e.g., 48000
    bitrate_kbps: float | None = None  # e.g., 128.0, 640.0

    @classmethod
    def from_dict(cls, d: dict) -> "AudioStreamInfo":
        """Create from dict (for JSON deserialization)."""
        return cls(
            codec=d.get("codec", "unknown"),
            language=d.get("language"),
            title=d.get("title"),
            channels=d.get("channels"),
            sample_rate=d.get("sample_rate"),
            bitrate_kbps=d.get("bitrate_kbps"),
        )


@dataclass(frozen=True)
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


@dataclass(frozen=True)
class VideoMetadata:
    """Extracted metadata from ffprobe video_info dict.

    Consolidates the stream iteration pattern used across 6+ files.
    All fields are optional since extraction may partially fail.

    Usage:
        meta = extract_video_metadata(video_info)
        if not meta.has_video:
            raise InputFileError("No video stream")
        print(f"Codec: {meta.video_codec}, Resolution: {meta.width}x{meta.height}")
    """

    # Core flags
    has_video: bool = False  # Whether file has at least one video stream
    has_audio: bool = False  # Whether file has at least one audio stream

    # Video stream info (from first video stream)
    video_codec: str | None = None  # e.g., "h264", "hevc", "av1"
    width: int | None = None
    height: int | None = None
    fps: float | None = None  # Frames per second
    profile: str | None = None  # e.g., "High", "Main"
    pix_fmt: str | None = None  # e.g., "yuv420p"

    # Audio stream info - all streams
    audio_streams: list[AudioStreamInfo] = field(default_factory=list)

    # Convenience fields from first audio stream (commonly accessed)
    audio_channels: int | None = None
    audio_sample_rate: int | None = None
    audio_bitrate_kbps: float | None = None

    # Format-level info
    duration_sec: float | None = None
    bitrate_kbps: float | None = None
    file_size_bytes: int | None = None

    # Stream counts (for multi-stream awareness)
    video_stream_count: int = 0
    subtitle_stream_count: int = 0

    # Total audio bitrate across all streams (for size estimation)
    total_audio_bitrate_kbps: float | None = None

    @property
    def is_av1(self) -> bool:
        """Check if video codec is AV1."""
        return self.video_codec is not None and self.video_codec.lower() == "av1"

    @property
    def resolution_str(self) -> str | None:
        """Get resolution as 'WIDTHxHEIGHT' string, or None if unknown."""
        if self.width is not None and self.height is not None:
            return f"{self.width}x{self.height}"
        return None


class FileStatus(str, Enum):
    """Status of a file in the history system.

    Inherits from str for easy JSON serialization.
    """

    SCANNED = "scanned"  # Layer 1 - ffprobe metadata only
    ANALYZED = "analyzed"  # Layer 2 - CRF search complete (has best_crf)
    NOT_WORTHWHILE = "not_worthwhile"  # VMAF analysis showed conversion isn't beneficial
    CONVERTED = "converted"  # Successfully converted


class AnalysisLevel(int, Enum):
    """Analysis level of a file, representing how much processing has been done.

    Each level builds on the previous:
    - Level 0: File discovered via folder scan (os.scandir), no metadata yet
    - Level 1: Basic scan complete (ffprobe) - has codec, duration, resolution, rough estimates (~)
    - Level 2: Quality analysis complete (CRF search) - has optimal CRF, accurate predictions
    - Level 3: Conversion complete - actual output file exists

    Used to determine:
    - What data is available for display (~ prefix for estimates vs accurate values)
    - What processing is needed (Analyze+Convert vs Convert-only)
    - Queue operation display ("Analyze", "Convert", "Analyze+Convert")
    """

    DISCOVERED = 0  # File found, no analysis yet (values show "—")
    SCANNED = 1  # ffprobe metadata available (estimates show "~")
    ANALYZED = 2  # CRF search complete (accurate predictions, no "~")
    CONVERTED = 3  # Full conversion complete (actual output file)


class OutputMode(str, Enum):
    """Output mode for queue items."""

    REPLACE = "replace"  # Same folder, same name, deletes original
    SUFFIX = "suffix"  # Same folder, adds suffix, keeps original
    SEPARATE_FOLDER = "separate_folder"  # Different output folder


class OperationType(str, Enum):
    """Operation type for queue items."""

    CONVERT = "convert"  # Full conversion (with CRF search if needed)
    ANALYZE = "analyze"  # CRF search only, no encoding


class QueueItemStatus(str, Enum):
    """Status of a queue item.

    Inherits from str for easy JSON serialization.
    """

    PENDING = "pending"  # Waiting to be processed
    CONVERTING = "converting"  # Currently being processed
    COMPLETED = "completed"  # All files processed
    ERROR = "error"  # Failed with error
    STOPPED = "stopped"  # Interrupted by user stop


@dataclass
class QueueFileItem:
    """Individual file within a folder QueueItem."""

    path: str
    status: QueueItemStatus = QueueItemStatus.PENDING
    size_bytes: int = 0
    error_message: str | None = None


@dataclass
class QueueItem:
    """A file or folder in the conversion queue."""

    id: str  # UUID
    source_path: str  # File or folder path
    is_folder: bool  # True if folder, False if single file
    output_mode: OutputMode = OutputMode.REPLACE
    output_suffix: str | None = None  # Override default suffix (for SUFFIX mode)
    output_folder: str | None = None  # For SEPARATE_FOLDER mode
    operation_type: OperationType = OperationType.CONVERT  # Operation type (convert or analyze)

    # Runtime state (persisted for queue restoration)
    status: QueueItemStatus = QueueItemStatus.PENDING
    total_files: int = 0  # For folders: count after scan
    processed_files: int = 0

    # Outcome tracking (persisted for display after completion)
    files_succeeded: int = 0  # Files successfully converted/analyzed
    files_skipped: int = 0  # Files skipped (low-res, not-worthwhile, already-exists, already AV1)
    files_failed: int = 0  # Files that encountered errors
    last_error: str | None = None  # Most recent error message for display

    # File-level tracking (for folders)
    files: list[QueueFileItem] = field(default_factory=list)
    current_file_index: int = -1

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "id": self.id,
            "source_path": self.source_path,
            "is_folder": self.is_folder,
            "output_mode": self.output_mode.value,
            "output_suffix": self.output_suffix,
            "output_folder": self.output_folder,
            "operation_type": self.operation_type.value,
            "status": self.status.value,
            "total_files": self.total_files,
            "processed_files": self.processed_files,
            "files_succeeded": self.files_succeeded,
            "files_skipped": self.files_skipped,
            "files_failed": self.files_failed,
            "last_error": self.last_error,
            "files": [
                {"path": f.path, "status": f.status.value, "size_bytes": f.size_bytes, "error_message": f.error_message}
                for f in self.files
            ],
            "current_file_index": self.current_file_index,
        }

    @classmethod
    def from_dict(cls, data: dict) -> "QueueItem":
        """Create from dictionary (JSON deserialization)."""
        files_data = data.get("files", [])
        files = [
            QueueFileItem(
                path=f["path"],
                status=QueueItemStatus(f.get("status", "pending")),
                size_bytes=f.get("size_bytes", 0),
                error_message=f.get("error_message"),
            )
            for f in files_data
        ]
        return cls(
            id=data["id"],
            source_path=data["source_path"],
            is_folder=data["is_folder"],
            output_mode=OutputMode(data.get("output_mode", "replace")),
            output_suffix=data.get("output_suffix"),
            output_folder=data.get("output_folder"),
            operation_type=OperationType(data.get("operation_type", "convert")),
            status=QueueItemStatus(data.get("status", "pending")),
            total_files=data.get("total_files", 0),
            processed_files=data.get("processed_files", 0),
            files_succeeded=data.get("files_succeeded", 0),
            files_skipped=data.get("files_skipped", 0),
            files_failed=data.get("files_failed", 0),
            last_error=data.get("last_error"),
            files=files,
            current_file_index=data.get("current_file_index", -1),
        )

    def format_status_display(self) -> str:
        """Format status for display in queue tree.

        Returns human-readable status with outcome counts for terminal states.
        Examples: "Done (3✓ 1⊘ 1✗)", "Stopped (2✓)", "Error (1✗)", "Converting"
        """
        if self.status == QueueItemStatus.COMPLETED:
            parts = []
            if self.files_succeeded > 0:
                parts.append(f"{self.files_succeeded}✓")
            if self.files_skipped > 0:
                parts.append(f"{self.files_skipped}⊘")
            if self.files_failed > 0:
                parts.append(f"{self.files_failed}✗")
            return f"Done ({' '.join(parts)})" if parts else "Done"
        if self.status == QueueItemStatus.STOPPED:
            parts = []
            if self.files_succeeded > 0:
                parts.append(f"{self.files_succeeded}✓")
            if self.files_skipped > 0:
                parts.append(f"{self.files_skipped}⊘")
            if self.files_failed > 0:
                parts.append(f"{self.files_failed}✗")
            return f"Stopped ({' '.join(parts)})" if parts else "Stopped"
        if self.status == QueueItemStatus.ERROR:
            return f"Error ({self.files_failed}✗)" if self.files_failed else "Error"
        return self.status.value.capitalize()


@dataclass
class QueueConversionConfig:
    """Configuration for queue-based batch conversion.

    Holds a list of QueueItems, each with its own output settings.
    """

    queue_items: list[QueueItem]
    extensions: list[str]  # For folder scanning (e.g., ["mp4", "mkv"])
    convert_audio: bool
    audio_codec: str  # e.g., "opus", "aac"
    default_suffix: str = "_av1"


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
    width: int | None = None
    height: int | None = None
    bitrate_kbps: float | None = None
    audio_streams: list[AudioStreamInfo] = field(default_factory=list)

    # === Estimation (Layer 1) ===
    estimated_reduction_percent: float | None = None  # Based on similar files
    estimated_from_similar: int | None = None  # Count of similar files used for estimate

    # === VMAF Analysis (Layer 2 - CRF search results) ===
    vmaf_target_when_analyzed: int | None = None  # VMAF target achieved (may be lower than requested due to fallback)
    preset_when_analyzed: int | None = None  # Encoding preset used during analysis
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
    conversion_time_sec: float | None = None  # Legacy: combined time (for old records)
    crf_search_time_sec: float | None = None  # CRF search phase (ANALYZED, CONVERTED, NOT_WORTHWHILE)
    encoding_time_sec: float | None = None  # Encoding phase (CONVERTED only)
    final_crf: int | None = None
    final_vmaf: float | None = None
    vmaf_target_used: int | None = None
    output_audio_codec: str | None = None

    # === Timestamps ===
    first_seen: str | None = None  # ISO timestamp when first scanned
    last_updated: str | None = None  # ISO timestamp of last status change

    def get_analysis_level(self) -> AnalysisLevel:
        """Determine the analysis level of this file record.

        Returns:
            AnalysisLevel indicating how much processing has been done:
            - CONVERTED (3): Full conversion complete
            - ANALYZED (2): CRF search complete, has accurate predictions
            - SCANNED (1): ffprobe metadata available, has rough estimates
            - DISCOVERED (0): No analysis data yet
        """
        if self.status == FileStatus.CONVERTED:
            return AnalysisLevel.CONVERTED
        if self.status == FileStatus.ANALYZED:
            return AnalysisLevel.ANALYZED
        # Fallback for old records: check best_crf field (before ANALYZED status existed)
        if self.best_crf is not None and self.best_vmaf_achieved is not None:
            return AnalysisLevel.ANALYZED
        if self.video_codec is not None or self.duration_sec is not None:
            return AnalysisLevel.SCANNED
        return AnalysisLevel.DISCOVERED

    @property
    def total_time_sec(self) -> float | None:
        """Total processing time. Uses new fields if available, falls back to legacy."""
        if self.crf_search_time_sec is not None or self.encoding_time_sec is not None:
            return (self.crf_search_time_sec or 0) + (self.encoding_time_sec or 0)
        return self.conversion_time_sec  # Legacy fallback for old records


@dataclass(frozen=True)
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
    vmaf: float | None = None  # VMAF score
    crf: int | None = None  # CRF value
    vmaf_target_used: int | None = None  # Target VMAF for this attempt
    used_fallback: bool | None = None  # Whether fallback VMAF was used (unused in progress)

    # Size predictions
    size_reduction: float | None = None  # Predicted size reduction percentage
    original_size: int | None = None  # Original file size in bytes
    output_size: int | None = None  # Estimated/actual output size in bytes
    is_estimate: bool | None = None  # Whether output_size is an estimate

    # Encoding phase timing
    eta_text: str | None = None  # ETA string from ab-av1 (e.g., "5m 30s")

    # Additional metadata
    file_size_mb: float | None = None  # File size in MB (for initial file info)


@dataclass(frozen=True)
class ErrorInfo:
    """Information about a conversion error."""

    message: str
    error_type: str = "unknown"
    details: str = ""
    stack_trace: str | None = None


@dataclass(frozen=True)
class RetryInfo:
    """Information about a retry attempt with fallback VMAF."""

    message: str
    fallback_vmaf: int | None = None  # The VMAF target being attempted


@dataclass(frozen=True)
class SkippedInfo:
    """Information about a skipped file."""

    message: str
    original_size: int | None = None
    min_vmaf_attempted: int | None = None


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
    stopped_count: int = 0  # Files skipped due to user stop request

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
