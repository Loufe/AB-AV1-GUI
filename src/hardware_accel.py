# src/hardware_accel.py
"""
Hardware-accelerated decoding support for the AV1 Video Converter application.

Detects available hardware decoders (NVIDIA CUVID, Intel QSV) and provides
mapping from source video codecs to appropriate hardware decoders.
"""

import logging
import subprocess
from functools import lru_cache

from src.config import HW_DECODER_MAP
from src.utils import get_windows_subprocess_startupinfo
from src.vendor_manager import get_ffmpeg_path

logger = logging.getLogger(__name__)


@lru_cache(maxsize=1)
def get_available_hw_decoders() -> frozenset[str]:
    """Query FFmpeg for available hardware decoders (cuvid, qsv).

    This function is cached to avoid repeated FFmpeg queries.
    Call clear_hw_decoder_cache() to invalidate the cache.

    Returns:
        Frozenset of decoder names like {"h264_cuvid", "hevc_cuvid", "h264_qsv", etc.}
    """
    ffmpeg_path = get_ffmpeg_path()
    if not ffmpeg_path:
        logger.warning("FFmpeg not found, cannot detect hardware decoders")
        return frozenset()

    try:
        startupinfo, _ = get_windows_subprocess_startupinfo()
        result = subprocess.run(
            [str(ffmpeg_path), "-decoders"],
            capture_output=True,
            text=True,
            check=True,
            startupinfo=startupinfo,
            encoding="utf-8",
            timeout=10,
        )

        # Parse decoder list - format is like:
        # V..... h264_cuvid           Nvidia CUVID H264 decoder (codec h264)
        # V..... hevc_qsv             H.265 / HEVC (Intel Quick Sync Video acceleration) (codec hevc)
        decoders = set()
        for line in result.stdout.splitlines():
            line = line.strip()
            if not line or line.startswith("=") or line.startswith("-"):
                continue

            # Look for lines with decoder names containing "cuvid" or "qsv"
            if "cuvid" in line.lower() or "qsv" in line.lower():
                # Extract decoder name (second field after capability flags)
                parts = line.split()
                if len(parts) >= 2:
                    decoder_name = parts[1]
                    if decoder_name.endswith("_cuvid") or decoder_name.endswith("_qsv"):
                        decoders.add(decoder_name)

        logger.info(f"Detected hardware decoders: {', '.join(sorted(decoders)) if decoders else 'none'}")
        return frozenset(decoders)

    except subprocess.TimeoutExpired:
        logger.warning("FFmpeg decoder query timed out after 10s")
        return frozenset()
    except subprocess.CalledProcessError as e:
        logger.warning(f"FFmpeg decoder query failed: {e}")
        return frozenset()
    except Exception as e:
        logger.exception(f"Unexpected error detecting hardware decoders: {e}")
        return frozenset()


def get_video_codec_from_info(video_info: dict) -> str | None:
    """Extract video codec from ffprobe output safely.

    Args:
        video_info: Dictionary from get_video_info() containing streams

    Returns:
        Codec name (e.g., "h264", "hevc") or None if not found
    """
    if not video_info:
        return None

    streams = video_info.get("streams", [])
    for stream in streams:
        if stream.get("codec_type") == "video":
            codec_name = stream.get("codec_name")
            if codec_name:
                return codec_name.lower()

    return None


def get_hw_decoder_for_codec(source_codec: str) -> str | None:
    """Return best available hardware decoder for source codec, or None.

    Args:
        source_codec: Source video codec (e.g., "h264", "hevc", "vp9")

    Returns:
        Hardware decoder name (e.g., "h264_cuvid") or None if unavailable
    """
    if not source_codec:
        return None

    # Normalize codec name
    source_codec = source_codec.lower()

    # Get available decoders
    available = get_available_hw_decoders()
    if not available:
        return None

    # Get preferred decoders for this codec from config
    preferred_decoders = HW_DECODER_MAP.get(source_codec, [])
    if not preferred_decoders:
        logger.debug(f"No hardware decoder mapping for codec: {source_codec}")
        return None

    # Return first available decoder from preference list
    for decoder in preferred_decoders:
        if decoder in available:
            logger.debug(f"Selected hardware decoder for {source_codec}: {decoder}")
            return decoder

    logger.debug(f"No available hardware decoder for {source_codec} (wanted: {preferred_decoders})")
    return None


def clear_hw_decoder_cache() -> None:
    """Clear the cached decoder list (for settings refresh).

    Call this when you want to re-detect hardware decoders, for example
    after the user changes FFmpeg installations or updates drivers.
    """
    get_available_hw_decoders.cache_clear()
    logger.debug("Hardware decoder cache cleared")
