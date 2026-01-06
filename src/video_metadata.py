# src/video_metadata.py
"""
Video metadata extraction from ffprobe output.

Consolidates the stream iteration pattern that was duplicated across 6+ files
into a single, well-tested extraction function.
"""

from typing import Any

from src.models import AudioStreamInfo, VideoMetadata


def extract_video_metadata(video_info: dict[str, Any] | None) -> VideoMetadata:
    """Extract video metadata from ffprobe output into a structured dataclass.

    Consolidates the stream iteration pattern that was duplicated across 6+ files.
    Handles all error cases gracefully, returning a VideoMetadata with appropriate
    None values for any fields that couldn't be extracted.

    Args:
        video_info: Dictionary from ffprobe (via get_video_info) or None

    Returns:
        VideoMetadata dataclass with all extracted fields.
        If video_info is None, returns VideoMetadata with all defaults (has_video=False).
    """
    if not video_info:
        return VideoMetadata()

    # First video stream metadata
    video_codec: str | None = None
    width: int | None = None
    height: int | None = None
    fps: float | None = None
    profile: str | None = None
    pix_fmt: str | None = None

    # Audio streams (all streams with full metadata)
    audio_streams: list[AudioStreamInfo] = []

    # Convenience fields from first audio stream
    audio_channels: int | None = None
    audio_sample_rate: int | None = None
    audio_bitrate_kbps: float | None = None

    # Stream counts
    video_stream_count = 0
    subtitle_stream_count = 0

    # Total audio bitrate (sum across all audio streams)
    total_audio_bitrate_kbps: float = 0.0
    has_any_audio_bitrate = False

    for stream in video_info.get("streams", []):
        codec_type = stream.get("codec_type")

        if codec_type == "video":
            video_stream_count += 1
            # Only extract details from first video stream
            if video_stream_count == 1:
                video_codec = stream.get("codec_name")
                width = stream.get("width")
                height = stream.get("height")
                profile = stream.get("profile")
                pix_fmt = stream.get("pix_fmt")

                # Parse FPS from r_frame_rate (e.g., "24000/1001" or "30")
                fps_str = stream.get("r_frame_rate", "")
                if fps_str:
                    try:
                        if "/" in fps_str:
                            num, den = map(int, fps_str.split("/"))
                            fps = num / den if den != 0 else None
                        else:
                            fps = float(fps_str)
                    except (ValueError, TypeError, ZeroDivisionError):
                        fps = None

        elif codec_type == "audio":
            # Extract per-stream metadata
            try:
                stream_sample_rate = int(stream.get("sample_rate", 0)) or None
            except (ValueError, TypeError):
                stream_sample_rate = None

            try:
                stream_bitrate = float(stream.get("bit_rate", 0)) / 1000 or None
            except (ValueError, TypeError):
                stream_bitrate = None

            tags = stream.get("tags", {})
            stream_info = AudioStreamInfo(
                codec=stream.get("codec_name", "unknown"),
                language=tags.get("language"),
                title=tags.get("title"),
                channels=stream.get("channels"),
                sample_rate=stream_sample_rate,
                bitrate_kbps=stream_bitrate,
            )
            audio_streams.append(stream_info)

            # Sum bitrates from ALL audio streams
            if stream_bitrate and stream_bitrate > 0:
                total_audio_bitrate_kbps += stream_bitrate
                has_any_audio_bitrate = True

            # First stream info for convenience fields
            if len(audio_streams) == 1:
                audio_channels = stream.get("channels")
                audio_sample_rate = stream_sample_rate
                audio_bitrate_kbps = stream_bitrate

        elif codec_type == "subtitle":
            subtitle_stream_count += 1

    # Extract format-level info
    fmt = video_info.get("format", {})
    file_size_bytes = video_info.get("file_size")

    try:
        duration_sec = float(fmt.get("duration", 0)) or None
    except (ValueError, TypeError):
        duration_sec = None

    try:
        bitrate_kbps = int(fmt.get("bit_rate", 0)) / 1000 or None
    except (ValueError, TypeError):
        bitrate_kbps = None

    return VideoMetadata(
        has_video=video_stream_count > 0,
        has_audio=len(audio_streams) > 0,
        video_codec=video_codec,
        width=width,
        height=height,
        fps=fps,
        profile=profile,
        pix_fmt=pix_fmt,
        audio_streams=audio_streams,
        audio_channels=audio_channels,
        audio_sample_rate=audio_sample_rate,
        audio_bitrate_kbps=audio_bitrate_kbps,
        duration_sec=duration_sec,
        bitrate_kbps=bitrate_kbps,
        file_size_bytes=file_size_bytes,
        video_stream_count=video_stream_count,
        subtitle_stream_count=subtitle_stream_count,
        total_audio_bitrate_kbps=total_audio_bitrate_kbps if has_any_audio_bitrate else None,
    )
