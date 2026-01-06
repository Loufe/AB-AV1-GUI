# src/utils.py
"""
Utility functions for the AV1 Video Converter application.

Note: This module has been split into focused modules:
- src/platform_utils.py - Windows subprocess handling, power management
- src/privacy.py - Path anonymization, BLAKE2b hashing
- src/logging_setup.py - Logging configuration
"""

import dataclasses
import json
import logging
import os
import re
import subprocess
import tkinter as tk
import urllib.request
from collections.abc import Callable
from typing import Any
from urllib.error import URLError

from src.logging_setup import get_script_directory
from src.platform_utils import get_windows_subprocess_startupinfo
from src.privacy import PATH_PATTERNS, _anonymize_path_match, anonymize_filename
from src.vendor_manager import get_ffmpeg_path, get_ffprobe_path

# Logging setup
logger = logging.getLogger(__name__)


# --- ETA Parsing Function ---


def parse_eta_text(eta_text: str) -> float:
    """Parse ETA text from AB-AV1 into seconds.

    Args:
        eta_text: ETA string like "2 hours", "87 minutes", "3h 20m", etc.
    Returns:
        Seconds remaining, or 0 if unparseable
    """
    if not eta_text:
        return 0

    try:
        eta_lower = eta_text.lower()

        # Handle simple formats first
        if "hour" in eta_lower and "min" not in eta_lower:
            # Format: "2 hours" or "1 hour"
            match = re.search(r"(\d+(\.\d+)?)", eta_lower)
            if match:
                hours = float(match.group(1))
                return hours * 3600
        if "minute" in eta_lower and "hour" not in eta_lower:
            # Format: "87 minutes" or "1 minute"
            match = re.search(r"(\d+(\.\d+)?)", eta_lower)
            if match:
                minutes = float(match.group(1))
                return minutes * 60
        if "second" in eta_lower and "hour" not in eta_lower and "min" not in eta_lower:
            # Format: "30 seconds"
            match = re.search(r"(\d+(\.\d+)?)", eta_lower)
            if match:
                return float(match.group(1))
        if "h" in eta_lower and "m" in eta_lower:
            # Format: "3h 20m"
            match = re.match(r"(\d+)h\s*(\d+)m", eta_lower)
            if match:
                hours = int(match.group(1))
                minutes = int(match.group(2))
                return hours * 3600 + minutes * 60

        # More complex formats
        # Extract all numbers and units
        parts = re.findall(r"(\d+(?:\.\d+)?)\s*(hour|minute|second|h|m|s)", eta_lower)
        total_seconds = 0

        for value_str, unit in parts:
            value = float(value_str)
            if unit.startswith("h"):
                total_seconds += value * 3600
            elif unit.startswith("m"):
                total_seconds += value * 60
            elif unit.startswith("s"):
                total_seconds += value

        return max(0, total_seconds)
    except Exception as e:
        logger.warning(f"Could not parse ETA text '{eta_text}': {e}")
        return 0


# --- Formatting Functions ---


def format_time(seconds: float) -> str:
    """Format time in seconds to hours:minutes:seconds.

    Args:
        seconds: Time duration in seconds

    Returns:
        Formatted time string in the format of "h:mm:ss" or "m:ss"
    """
    if seconds is None or seconds < 0:
        return "--:--:--"
    hours = int(seconds // 3600)
    minutes = int((seconds % 3600) // 60)
    secs = int(seconds % 60)
    if hours > 0:
        return f"{hours}:{minutes:02d}:{secs:02d}"
    return f"{minutes}:{secs:02d}"


def format_file_size(size_bytes: int) -> str:
    """Format file size from bytes to appropriate unit (KB, MB, GB).

    Args:
        size_bytes: File size in bytes

    Returns:
        Formatted size string with appropriate unit (B, KB, MB, GB)
    """
    if size_bytes is None or size_bytes < 0:
        return "-"
    if size_bytes < 1024:  # noqa: PLR2004
        return f"{size_bytes} B"
    if size_bytes < 1024**2:
        return f"{size_bytes / 1024:.2f} KB"
    if size_bytes < 1024**3:
        return f"{size_bytes / (1024**2):.2f} MB"
    return f"{size_bytes / (1024**3):.2f} GB"


# --- Video and FFmpeg Utilities ---


def log_video_properties(video_info: dict, prefix: str = "Input") -> None:
    """Log video file properties including format, codecs, resolution, etc.

    Args:
        video_info: Dictionary containing video metadata from ffprobe
        prefix: Prefix to use in log messages (e.g., "Input" or "Output")
    """
    if not video_info:
        logger.warning(f"{prefix} video info not available")
        return
    file_size = video_info.get("file_size", 0)
    format_info = video_info.get("format", {})
    duration_str = format_info.get("duration", "0")
    bit_rate_str = format_info.get("bit_rate", "0")
    try:
        duration = float(duration_str)
        bit_rate = int(bit_rate_str)
    except (ValueError, TypeError):
        duration = 0
        bit_rate = 0
    logger.info(
        f"{prefix} File - Size: {format_file_size(file_size)}, Duration: {format_time(duration)}, "
        f"Bitrate: {bit_rate / 1000:.2f} kbps"
    )
    for stream in video_info.get("streams", []):
        codec_type = stream.get("codec_type", "unknown")
        codec_name = stream.get("codec_name", "unknown")
        if codec_type == "video":
            width = stream.get("width", 0)
            height = stream.get("height", 0)
            fps = stream.get("r_frame_rate", "0/1")
            try:
                if "/" in fps:
                    num, den = map(int, fps.split("/"))
                    fps_value = num / den if den != 0 else 0
                else:
                    fps_value = float(fps)
            except (ValueError, TypeError, ZeroDivisionError) as e:
                logger.debug(f"Error parsing FPS value '{fps}': {e}")
                fps_value = 0
            profile = stream.get("profile", "unknown")
            pix_fmt = stream.get("pix_fmt", "unknown")
            logger.info(
                f"{prefix} Video - {width}x{height} ({width * height / 1000000:.2f} MP), "
                f"{fps_value:.3f} fps, Codec: {codec_name}, Profile: {profile}, Pixel Format: {pix_fmt}"
            )
        elif codec_type == "audio":
            channels = stream.get("channels", 0)
            sample_rate = stream.get("sample_rate", 0)
            audio_bitrate_str = stream.get("bit_rate", "0")
            try:
                audio_bitrate = int(audio_bitrate_str) / 1000  # kbps
            except (ValueError, TypeError):
                audio_bitrate = 0
            logger.info(
                f"{prefix} Audio - Codec: {codec_name}, Channels: {channels}, "
                f"Sample Rate: {sample_rate} Hz, Bitrate: {audio_bitrate:.1f} kbps"
            )


def get_video_info(video_path: str, timeout: int = 30) -> dict[str, Any] | None:
    """Get video file information using ffprobe.

    Args:
        video_path: Path to the video file to analyze
        timeout: Maximum seconds to wait for ffprobe (default 30)

    Returns:
        Dictionary containing video metadata or None if analysis failed
    """
    # Guard against directories being passed (corrupted state)
    if not os.path.isfile(video_path):
        logger.warning(f"get_video_info called with non-file path: {anonymize_filename(video_path)}")
        return None

    ffprobe_path = get_ffprobe_path()
    if not ffprobe_path:
        logger.error("ffprobe not found")
        return None
    cmd = [str(ffprobe_path), "-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", video_path]
    try:
        startupinfo, _ = get_windows_subprocess_startupinfo()
        result = subprocess.run(
            cmd, capture_output=True, text=True, check=True, startupinfo=startupinfo, encoding="utf-8", timeout=timeout
        )
        info = json.loads(result.stdout)
        try:
            info["file_size"] = os.path.getsize(video_path)
        except Exception as e:
            info["file_size"] = 0
            logger.debug(f"No size for {video_path}: {e}")
        return info
    except subprocess.TimeoutExpired:
        logger.warning(f"ffprobe timed out after {timeout}s for {anonymize_filename(video_path)}")
        return None
    except subprocess.CalledProcessError:
        logger.exception(f"ffprobe failed for {anonymize_filename(video_path)}")
        return None
    except json.JSONDecodeError:
        logger.exception(f"ffprobe JSON error for {anonymize_filename(video_path)}")
        return None
    except FileNotFoundError:
        logger.exception("ffprobe not found")
        return None
    except Exception:
        logger.exception(f"ffprobe unexpected error {anonymize_filename(video_path)}")
        return None


def check_ffmpeg_availability() -> tuple:
    """Check if FFmpeg is installed and has SVT-AV1 support.

    Returns:
        Tuple of (ffmpeg_available, svt_av1_available, version_info, error_message)
    """
    ffmpeg_path = get_ffmpeg_path()
    if ffmpeg_path is None:
        return False, False, None, "ffmpeg not found (not in vendor/ or PATH)"
    try:
        ffmpeg_str = str(ffmpeg_path)
        startupinfo, _ = get_windows_subprocess_startupinfo()
        result = subprocess.run(
            [ffmpeg_str, "-encoders"],
            capture_output=True,
            text=True,
            check=True,
            startupinfo=startupinfo,
            encoding="utf-8",
        )

        svt_av1_available = "libsvtav1" in result.stdout
        version_info = None
        try:
            version_result = subprocess.run(
                [ffmpeg_str, "-version"],
                check=False,
                capture_output=True,
                text=True,
                startupinfo=startupinfo,
                encoding="utf-8",
            )
            if version_result.stdout:
                lines = version_result.stdout.splitlines()
                if lines:
                    version_info = lines[0]
        except Exception as version_err:
            # Just log and continue if version check fails
            logger.debug(f"Failed to get FFmpeg version: {version_err}")
        return True, svt_av1_available, version_info, None
    except Exception as e:
        return True, False, None, str(e)


# GitHub API endpoints for FFmpeg builds
FFMPEG_GYAN_GITHUB_API = "https://api.github.com/repos/GyanD/codexffmpeg/releases/latest"
FFMPEG_BTBN_GITHUB_API = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest"


def parse_ffmpeg_version(version_string: str | None) -> tuple[str | None, str | None, str | None]:
    """Parse FFmpeg version string to extract version number, source, and build type.

    Args:
        version_string: Full version string like "ffmpeg version 7.1.1-full_build-www.gyan.dev ..."

    Returns:
        Tuple of (version, source, build_type) where:
        - version: Version number (e.g., "7.1.1") or None
        - source: Detected source ("gyan.dev", "BtbN", or None for unknown)
        - build_type: Build variant ("full", "essentials", etc.) or None
    """
    if not version_string:
        return None, None, None

    # Extract version number - pattern: "ffmpeg version X.Y.Z" or "ffmpeg version N-xxxxx"
    match = re.search(r"ffmpeg version (\d+\.\d+(?:\.\d+)?)", version_string, re.IGNORECASE)
    version = match.group(1) if match else None

    # Detect source and build type
    source = None
    build_type = None
    if "gyan.dev" in version_string.lower():
        source = "gyan.dev"
        # Extract build type: "full_build", "essentials_build", etc.
        build_match = re.search(r"(full|essentials)(?:[_-]build)?", version_string.lower())
        if build_match:
            build_type = build_match.group(1)
    elif "btbn" in version_string.lower():
        source = "BtbN"

    return version, source, build_type


def check_ffmpeg_latest_gyan() -> tuple[str | None, str | None, str]:
    """Check GyanD's GitHub for the latest FFmpeg release version.

    This function makes a network request and should only be called
    when the user explicitly requests a version check.

    Returns:
        Tuple of (latest_version, release_url, message) where:
        - latest_version: Version string (e.g., "8.0.1") or None on error
        - release_url: URL to the release page, or None on error
        - message: Descriptive message about the result
    """
    try:
        request = urllib.request.Request(  # noqa: S310 - hardcoded https URL is safe
            FFMPEG_GYAN_GITHUB_API,
            headers={"Accept": "application/vnd.github.v3+json", "User-Agent": "Auto-AV1-Converter"},
        )
        with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310
            data = json.loads(response.read().decode("utf-8"))
            tag_name = data.get("tag_name", "")
            html_url = data.get("html_url", "")

            # Tag should be version like "8.0.1"
            version = tag_name.lstrip("v") if tag_name else None

            if version:
                return version, html_url, f"Latest version: {version}"
            return None, None, "Could not parse version from GitHub response"

    except URLError as e:
        logger.warning(f"Failed to check GitHub for FFmpeg updates: {e}")
        return None, None, f"Network error: {e.reason}"
    except json.JSONDecodeError as e:
        logger.warning(f"Failed to parse GitHub response: {e}")
        return None, None, "Failed to parse GitHub response"
    except Exception as e:
        logger.warning(f"Unexpected error checking GitHub: {e}")
        return None, None, f"Error: {e}"


def check_ffmpeg_latest_btbn() -> tuple[str | None, str | None, str]:
    """Check BtbN's GitHub for the latest FFmpeg build.

    BtbN uses date-based tags (e.g., "autobuild-2025-12-18-12-50") rather than
    semantic versions, so we just return the tag for display without comparison.

    Returns:
        Tuple of (latest_tag, release_url, message) where:
        - latest_tag: Tag name (e.g., "autobuild-2025-12-18-12-50") or None on error
        - release_url: URL to the release page, or None on error
        - message: Descriptive message about the result
    """
    try:
        request = urllib.request.Request(  # noqa: S310 - hardcoded https URL is safe
            FFMPEG_BTBN_GITHUB_API,
            headers={"Accept": "application/vnd.github.v3+json", "User-Agent": "Auto-AV1-Converter"},
        )
        with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310
            data = json.loads(response.read().decode("utf-8"))
            tag_name = data.get("tag_name", "")
            html_url = data.get("html_url", "")

            if tag_name:
                # Extract date from tag like "autobuild-2025-12-18-12-50"
                if "autobuild" in tag_name:
                    display_name = tag_name.replace("autobuild-", "").rsplit("-", 2)[0]
                else:
                    display_name = tag_name
                return tag_name, html_url, f"Latest: {display_name}"
            return None, None, "Could not parse release from GitHub response"

    except URLError as e:
        logger.warning(f"Failed to check GitHub for BtbN FFmpeg updates: {e}")
        return None, None, f"Network error: {e.reason}"
    except json.JSONDecodeError as e:
        logger.warning(f"Failed to parse GitHub response: {e}")
        return None, None, "Failed to parse GitHub response"
    except Exception as e:
        logger.warning(f"Unexpected error checking GitHub: {e}")
        return None, None, f"Error: {e}"


# For UI updates
def update_ui_safely(root: tk.Tk, update_function: Callable[..., Any], *args: Any, **kwargs: Any) -> None:
    """Thread-safe UI update with extra safety checks and logging.

    Args:
        root: Tkinter root window object
        update_function: Function to call on the UI thread
        *args: Arguments to pass to the update function
        **kwargs: Keyword arguments to pass to the update function
    """
    if root and root.winfo_exists():
        try:
            # Create a wrapper to catch errors in UI updates
            def _safe_update_wrapper():
                try:
                    return update_function(*args, **kwargs)
                except Exception:
                    func_name = update_function.__name__ if hasattr(update_function, "__name__") else "lambda"
                    logger.exception(f"Error in UI update function {func_name}")

            # Schedule on the main thread
            root.after(0, _safe_update_wrapper)
        except Exception:
            func_name = update_function.__name__ if hasattr(update_function, "__name__") else "lambda"
            logger.exception(f"Error scheduling UI update for {func_name}")


def log_conversion_result(input_path: str, output_path: str, elapsed_time: float) -> None:
    """Log the results of a successful conversion including size reduction and time.

    Args:
        input_path: Path to the input video file
        output_path: Path to the output converted video
        elapsed_time: Time taken for conversion in seconds
    """
    if not os.path.exists(output_path):
        logger.error(f"Result log failed - Output missing: {anonymize_filename(output_path)}")
        return
    try:
        input_size = os.path.getsize(input_path)
        output_size = os.path.getsize(output_path)

        # Calculate ratio and reduction percentage
        if input_size <= 0:
            ratio = 0
            size_reduction_percent = 0
        else:
            ratio = (output_size / input_size) * 100
            size_reduction_percent = 100 - ratio

        size_reduction = input_size - output_size
        input_info = get_video_info(input_path)
        output_info = get_video_info(output_path)
        input_bitrate = 0
        output_bitrate = 0
        resolution = ""
        if input_info and "format" in input_info and "bit_rate" in input_info["format"]:
            try:
                input_bitrate = int(input_info["format"]["bit_rate"]) / 1000
            except (ValueError, TypeError):
                # Handle potential conversion errors
                logger.debug("Could not convert input bitrate to integer")
            for stream in input_info.get("streams", []):
                if stream.get("codec_type") == "video":
                    width = stream.get("width", 0)
                    height = stream.get("height", 0)
                    resolution = f"{width}x{height}"
                    break
        if output_info and "format" in output_info and "bit_rate" in output_info["format"]:
            try:
                output_bitrate = int(output_info["format"]["bit_rate"]) / 1000
            except (ValueError, TypeError, KeyError) as e:
                logger.debug(f"Error parsing output bitrate: {e}")
        logger.info(
            f"Conversion Result [{anonymize_filename(output_path)}]: Input: {format_file_size(input_size)}, "
            f"Output: {format_file_size(output_size)}, Reduction: {format_file_size(size_reduction)} "
            f"({size_reduction_percent:.2f}%), Time: {format_time(elapsed_time)}"
        )
        if input_bitrate > 0 and output_bitrate > 0:
            bitrate_reduction = input_bitrate - output_bitrate
            bitrate_ratio = (output_bitrate / input_bitrate) * 100 if input_bitrate > 0 else 0
            logger.info(
                f"Bitrate Details - Input: {input_bitrate:.2f} kbps, Output: {output_bitrate:.2f} kbps, "
                f"Reduction: {bitrate_reduction:.2f} kbps ({100 - bitrate_ratio:.2f}%), Res: {resolution}"
            )
        print(
            f"Conversion complete [{anonymize_filename(output_path)}] - Size reduced by "
            f"{size_reduction_percent:.2f}% from {format_file_size(input_size)} to "
            f"{format_file_size(output_size)} in {format_time(elapsed_time)}"
        )
    except Exception:
        logger.exception(f"Error logging conversion result for {anonymize_filename(output_path)}")


# --- History Management Functions ---
# Note: The main history logic is in src/history_index.py.
# Use get_history_path() from there for the history file path.


def scrub_history_paths() -> tuple[int, int]:
    """Anonymize file paths in existing history entries.

    Sets original_path to None for all records that have it set.
    This ensures privacy by only keeping the path_hash.

    Returns:
        Tuple of (total_records, records_modified)
    """
    # Import here to avoid circular imports (history_index imports from utils)
    from src.history_index import get_history_index  # noqa: PLC0415

    index = get_history_index()
    all_records = index.get_all_records()

    if not all_records:
        return 0, 0

    modified_count = 0
    for record in all_records:
        if record.original_path is not None:
            # Create a new record with original_path set to None
            updated_record = dataclasses.replace(record, original_path=None)
            index.upsert(updated_record)
            modified_count += 1

    # Save if any records were modified
    if modified_count > 0:
        index.save()
        logger.info(f"Scrubbed {modified_count} of {len(all_records)} history records")

    return len(all_records), modified_count


def scrub_log_files(log_directory: str | None = None) -> tuple[int, int]:
    """Anonymize file paths in existing log files.

    Replaces full file paths with BLAKE2b hashes for privacy.

    Args:
        log_directory: Path to logs directory. If None, uses default logs folder.

    Returns:
        Tuple of (total_files, files_modified)
    """
    if log_directory is None:
        log_directory = os.path.join(get_script_directory(), "logs")

    if not os.path.isdir(log_directory):
        return 0, 0

    log_files = [f for f in os.listdir(log_directory) if f.endswith(".log")]
    if not log_files:
        return 0, 0

    modified_count = 0
    for log_filename in log_files:
        log_path = os.path.join(log_directory, log_filename)
        try:
            with open(log_path, encoding="utf-8", errors="replace") as f:
                original_content = f.read()

            # Apply path anonymization patterns
            scrubbed_content = original_content
            for pattern in PATH_PATTERNS:
                scrubbed_content = pattern.sub(_anonymize_path_match, scrubbed_content)

            # Only write if content changed
            if scrubbed_content != original_content:
                with open(log_path, "w", encoding="utf-8") as f:
                    f.write(scrubbed_content)
                modified_count += 1

        except OSError:
            logger.exception(f"Error scrubbing log file: {log_filename}")
            continue

    if modified_count > 0:
        logger.info(f"Scrubbed {modified_count} of {len(log_files)} log files")

    return len(log_files), modified_count
