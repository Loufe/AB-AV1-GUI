# src/gui/tree_display.py
"""
Shared display helpers for tree views.

Provides consistent status formatting and tag mapping for both
the analysis tree and queue tree, reducing code duplication.
"""

from src.models import FileStatus, QueueItemStatus

# =============================================================================
# Status Tag Mappings
# =============================================================================

# Queue tree status → CSS-like tag for styling
QUEUE_STATUS_TAGS: dict[QueueItemStatus, str] = {
    QueueItemStatus.PENDING: "file_pending",
    QueueItemStatus.CONVERTING: "file_converting",
    QueueItemStatus.COMPLETED: "file_done",
    QueueItemStatus.STOPPED: "file_skipped",
    QueueItemStatus.ERROR: "file_error",
}

# Analysis tree file status → CSS-like tag for styling
ANALYSIS_STATUS_TAGS: dict[FileStatus, str] = {
    FileStatus.CONVERTED: "done",
    FileStatus.NOT_WORTHWHILE: "skip",
}


# =============================================================================
# Queue Status Display Formatting
# =============================================================================


def format_queue_status_display(
    status: QueueItemStatus,
    *,
    stopping: bool = False,
    total_files: int = 0,
    processed_files: int = 0,
    error_message: str | None = None,
) -> tuple[str, str]:
    """Format queue item status for display.

    Args:
        status: The queue item status.
        stopping: Whether a stop has been requested.
        total_files: Total files in a folder item (for progress display).
        processed_files: Files processed so far in a folder item.
        error_message: Error message if status is ERROR.

    Returns:
        Tuple of (display_text, tag_name).
    """
    # Handle stop-requested state for pending items
    if stopping and status == QueueItemStatus.PENDING:
        return "Will skip", "file_skipped"

    tag = QUEUE_STATUS_TAGS.get(status, "file_pending")

    if status == QueueItemStatus.COMPLETED:
        return "Done", tag
    if status == QueueItemStatus.CONVERTING:
        if total_files > 0:
            return f"Converting ({processed_files}/{total_files})", tag
        return "Converting...", tag
    if status == QueueItemStatus.ERROR:
        return error_message or "Error", tag
    if status == QueueItemStatus.STOPPED:
        return "Stopped", tag
    if status == QueueItemStatus.PENDING:
        return "", tag  # Pending items show no status text
    return "", tag


def format_queue_file_status(
    status: QueueItemStatus,
    *,
    stopping: bool = False,
    error_message: str | None = None,
) -> tuple[str, str]:
    """Format queue file item status for display (nested files in folder items).

    Args:
        status: The file item status.
        stopping: Whether a stop has been requested.
        error_message: Error message if status is ERROR.

    Returns:
        Tuple of (display_text, tag_name).
    """
    # Handle stop-requested state for pending items
    if stopping and status == QueueItemStatus.PENDING:
        return "Will skip", "file_skipped"

    tag = QUEUE_STATUS_TAGS.get(status, "file_pending")

    if status == QueueItemStatus.COMPLETED:
        return "Done", tag
    if status == QueueItemStatus.CONVERTING:
        return "Converting...", tag
    if status == QueueItemStatus.ERROR:
        return error_message or "Error", tag
    if status == QueueItemStatus.STOPPED:
        return "Stopped", tag
    if status == QueueItemStatus.PENDING:
        return "", tag  # Pending files show no status
    return "", tag


# =============================================================================
# Analysis Tree Status Display Formatting
# =============================================================================


def get_analysis_file_tag(status: FileStatus, video_codec: str | None = None) -> str:
    """Get the display tag for an analysis tree file.

    Args:
        status: The file status from history.
        video_codec: The video codec (to detect AV1 files).

    Returns:
        Tag name for styling, or empty string if no special tag.
    """
    if status == FileStatus.CONVERTED:
        return "done"
    if status == FileStatus.NOT_WORTHWHILE:
        return "skip"
    if video_codec and video_codec.lower() == "av1":
        return "av1"
    return ""


# =============================================================================
# Stream Format Display
# =============================================================================


def format_stream_display(
    video_codec: str | None,
    audio_codec: str | None = None,
    audio_stream_count: int = 1,
    subtitle_stream_count: int = 0,
) -> str:
    """Build format string showing codec and stream counts.

    Args:
        video_codec: Video codec name (e.g., "h264", "hevc").
        audio_codec: First audio stream codec name (e.g., "aac", "opus").
        audio_stream_count: Total number of audio streams.
        subtitle_stream_count: Total number of subtitle streams.

    Returns:
        Format string like "H264 / 3 audio [2 subs]".

    Examples:
        >>> format_stream_display("h264", "aac")
        'H264 / AAC'
        >>> format_stream_display("hevc", "aac", audio_stream_count=3)
        'HEVC / 3 audio'
        >>> format_stream_display("h264", "opus", audio_stream_count=2, subtitle_stream_count=3)
        'H264 / 2 audio [3 subs]'
    """
    video = (video_codec or "?").upper()

    # Audio portion: show count if multiple, codec name if single
    if audio_stream_count > 1:
        audio = f"{audio_stream_count} audio"
    elif audio_codec:
        audio = audio_codec.upper()
    else:
        audio = "no audio"

    # Subtitle portion: show count if any
    if subtitle_stream_count > 0:
        n = subtitle_stream_count
        subs = f" [{n} sub{'s' if n > 1 else ''}]"
    else:
        subs = ""

    return f"{video} / {audio}{subs}"
