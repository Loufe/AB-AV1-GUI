# src/utils.py
"""
Utility functions for the AV1 Video Converter application.
"""

import ctypes
import dataclasses
import datetime
import hashlib
import json
import logging
import os
import re  # Need for parse_eta_text
import subprocess
import sys  # Needed for sys.argv access
import tkinter as tk
import urllib.request
from logging.handlers import RotatingFileHandler
from typing import Any, Callable
from urllib.error import URLError

from src.config import HISTORY_FILE_V2
from src.vendor_manager import get_ffmpeg_path, get_ffprobe_path

# Logging setup
logger = logging.getLogger(__name__)


# --- Windows Subprocess Helper ---


def get_windows_subprocess_startupinfo() -> tuple[Any, int]:
    """Get Windows subprocess startup info to hide console windows.

    Returns:
        Tuple of (startupinfo, creationflags). On non-Windows, returns (None, 0).
    """
    if sys.platform != "win32":
        return None, 0
    startupinfo = subprocess.STARTUPINFO()
    startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startupinfo.wShowWindow = subprocess.SW_HIDE
    creationflags = subprocess.CREATE_NO_WINDOW
    return startupinfo, creationflags


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


# --- Path Anonymization (BLAKE2b Hash-Based) ---

# Configured folders for special labeling (set via set_anonymization_folders)
_configured_input_folder: str | None = None
_configured_output_folder: str | None = None

# Cache for hash lookups (avoids recomputing)
_path_hash_cache: dict[str, str] = {}


def _normalize_path(path: str) -> str:
    """Normalize a path for consistent hashing across platforms.

    Args:
        path: File or directory path to normalize

    Returns:
        Normalized path string suitable for hashing
    """
    # Get absolute path and normalize separators
    normalized = os.path.normpath(os.path.abspath(path))
    # Lowercase on Windows (case-insensitive filesystem)
    if sys.platform == "win32":
        normalized = normalized.lower()
    # Use forward slashes consistently
    return normalized.replace("\\", "/")


def _compute_hash(value: str, length: int = 12) -> str:
    """Compute a BLAKE2b hash of a string, truncated for readability.

    Args:
        value: String to hash
        length: Number of hex characters to return (default 12)

    Returns:
        Truncated hex hash string
    """
    # Use BLAKE2b with 16-byte digest (32 hex chars), then truncate
    hash_bytes = hashlib.blake2b(value.encode("utf-8"), digest_size=16).digest()
    return hash_bytes.hex()[:length]


def set_anonymization_folders(input_folder: str | None, output_folder: str | None) -> None:
    """Set the configured input/output folders for special labeling.

    When these folders appear in paths, they'll be shown as [input_folder]
    or [output_folder] instead of hashes for readability.

    Args:
        input_folder: The configured input folder path
        output_folder: The configured output folder path
    """
    global _configured_input_folder, _configured_output_folder  # noqa: PLW0603
    _configured_input_folder = _normalize_path(input_folder) if input_folder else None
    _configured_output_folder = _normalize_path(output_folder) if output_folder else None


def anonymize_folder(folder_path: str) -> str:
    """Anonymize a folder path using BLAKE2b hash or special labels.

    Args:
        folder_path: Full path to the folder

    Returns:
        Anonymized folder identifier like '[input_folder]' or 'folder_7f3a9c2b1e4d'
    """
    if not folder_path:
        return "[unknown]"

    normalized = _normalize_path(folder_path)

    # Check for configured special folders
    if _configured_input_folder and normalized == _configured_input_folder:
        return "[input_folder]"
    if _configured_output_folder and normalized == _configured_output_folder:
        return "[output_folder]"

    # Check cache
    cache_key = f"folder:{normalized}"
    if cache_key in _path_hash_cache:
        return _path_hash_cache[cache_key]

    # Compute hash
    folder_hash = _compute_hash(normalized)
    result = f"folder_{folder_hash}"
    _path_hash_cache[cache_key] = result
    return result


def anonymize_file(filename: str) -> str:
    """Anonymize a filename (basename only) using BLAKE2b hash.

    Args:
        filename: Just the filename (not full path), e.g., 'video.mp4'

    Returns:
        Anonymized filename like 'file_7f3a9c2b1e4d.mp4'
    """
    if not filename:
        return "file_unknown"

    # Extract just the basename if a full path was passed
    basename = os.path.basename(filename)

    # Normalize for consistent hashing
    normalized = basename.lower() if sys.platform == "win32" else basename

    # Check cache
    cache_key = f"file:{normalized}"
    if cache_key in _path_hash_cache:
        return _path_hash_cache[cache_key]

    # Compute hash and preserve extension
    name_without_ext, ext = os.path.splitext(normalized)
    file_hash = _compute_hash(name_without_ext)
    result = f"file_{file_hash}{ext}"
    _path_hash_cache[cache_key] = result
    return result


def anonymize_path(file_path: str) -> str:
    """Anonymize a full file path (folder + filename).

    Args:
        file_path: Full path to the file

    Returns:
        Anonymized path like '[input_folder]/file_7f3a9c2b1e4d.mp4'
    """
    if not file_path:
        return "[unknown]/file_unknown"

    folder = os.path.dirname(file_path)
    filename = os.path.basename(file_path)

    anon_folder = anonymize_folder(folder)
    anon_file = anonymize_file(filename)

    return f"{anon_folder}/{anon_file}"


def anonymize_filename(filename: str) -> str:
    """Anonymize a file path or filename for privacy.

    Convenience function that dispatches to the appropriate anonymization:
    - Full paths → anonymize_path() (hashes both folder and filename)
    - Bare filenames → anonymize_file() (hashes just the filename)

    Args:
        filename: File path or bare filename to anonymize

    Returns:
        Anonymized string like '[input_folder]/file_7f3a9c2b1e4d.mp4'
    """
    if not filename:
        return filename

    # If it looks like a full path, anonymize the whole thing
    if os.path.dirname(filename):
        return anonymize_path(filename)

    # Just a filename, anonymize it alone
    return anonymize_file(filename)


# Regex patterns for detecting paths and filenames in log messages
_PATH_PATTERNS = [
    # Windows drive paths: C:\... or C:/...
    re.compile(r"[A-Za-z]:[\\\/][^\s\"'<>|*?\n]+"),
    # UNC paths: \\server\share\...
    re.compile(r"\\\\[^\s\"'<>|*?\n]+"),
    # Unix absolute paths with common roots
    re.compile(r"/(?:home|Users|mnt|media|var|tmp|opt|usr)[^\s\"'<>|*?\n]*"),
    # Video filenames (may contain spaces, captured after common delimiters)
    # Matches filenames after: colon, arrow, equals, quotes
    re.compile(
        r"(?:(?<=: )|(?<=-> )|(?<== )|(?<=\")|(?<='))[^<>|*?\n\"']+\.(?:mp4|mkv|avi|wmv|mov|webm)", re.IGNORECASE
    ),
    # Fallback: video filenames without spaces (catches simple cases)
    re.compile(r"[^\s\"'<>|*?\n/\\]+\.(?:mp4|mkv|avi|wmv|mov|webm)", re.IGNORECASE),
]


_MAX_EXTENSION_LENGTH = 5  # Maximum reasonable file extension length


def _anonymize_path_match(match: re.Match) -> str:
    """Replacement function for regex path matching."""
    path = match.group(0)
    # Determine if it's a file or directory
    # If it has an extension, treat as file path
    _, ext = os.path.splitext(path)
    if ext and len(ext) <= _MAX_EXTENSION_LENGTH:
        # Check if it's a standalone filename (no directory component)
        if not os.path.dirname(path):
            return anonymize_file(path)
        return anonymize_path(path)
    return anonymize_folder(path)


class PathPrivacyFilter(logging.Filter):
    """Log filter that anonymizes file paths using BLAKE2b hashes.

    This filter proactively detects path patterns in log messages and
    replaces them with anonymized versions, rather than relying on
    pre-registered paths.
    """

    def filter(self, record: logging.LogRecord) -> bool:
        if hasattr(record, "msg") and isinstance(record.msg, str):
            temp_msg = record.msg
            for pattern in _PATH_PATTERNS:
                temp_msg = pattern.sub(_anonymize_path_match, temp_msg)
            record.msg = temp_msg
        return True


# Custom filter to suppress excessive sled::pagecache trace messages
class SledTraceFilter(logging.Filter):
    def filter(self, record):
        return not (
            hasattr(record, "msg")
            and isinstance(record.msg, str)
            and "sled::pagecache" in record.msg
            and "TRACE" in record.msg
        )


# --- Logging Setup and Utilities ---


def get_script_directory() -> str:
    """Get the directory containing the main script/executable.

    Returns:
        Absolute path to the directory containing the main script or executable
    """
    if getattr(sys, "frozen", False):
        return os.path.dirname(sys.executable)
    if "__file__" in globals():
        script_path = os.path.abspath(__file__)
        # Navigate up one level from src/utils.py to the script directory
        return os.path.dirname(os.path.dirname(script_path))
    if sys.argv and sys.argv[0]:
        # Fallback using argv[0], might be less reliable depending on how it's run
        return os.path.dirname(os.path.abspath(sys.argv[0]))
    # Last resort fallback
    return os.getcwd()


def setup_logging(log_directory: str | None = None, anonymize: bool = True) -> str | None:
    """Set up logging to file and console. Defaults log dir next to script.

    Args:
        log_directory: Optional path to log directory. If None, uses 'logs' folder next to script
        anonymize: Whether to anonymize filenames in logs for privacy

    Returns:
        The actual log directory path used, or None if setup failed
    """
    # (Unchanged from previous version)
    actual_log_directory_used = None
    try:
        if log_directory and os.path.isdir(log_directory):
            logs_dir = os.path.abspath(log_directory)
            print(f"Using custom log directory: {logs_dir}")
        else:
            script_dir = get_script_directory()
            logs_dir = os.path.join(script_dir, "logs")
            logs_dir = os.path.abspath(logs_dir)
            if log_directory:
                print(f"Warning: Custom log dir '{log_directory}' invalid. Using default: {logs_dir}")
            else:
                print(f"Using default log directory: {logs_dir}")
        actual_log_directory_used = logs_dir
        os.makedirs(logs_dir, exist_ok=True)
    except Exception as e:
        print(f"ERROR: Cannot create/access log directory '{logs_dir}': {e}", file=sys.stderr)
        actual_log_directory_used = None
    log_file = None
    if actual_log_directory_used:
        timestamp = datetime.datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
        log_file = os.path.join(actual_log_directory_used, f"av1_convert_{timestamp}.log")
    log_formatter = logging.Formatter("%(asctime)s - %(levelname)s - %(message)s")
    file_handler = None
    if log_file:
        try:
            file_handler = RotatingFileHandler(log_file, maxBytes=10 * 1024 * 1024, backupCount=5, encoding="utf-8")
            file_handler.setFormatter(log_formatter)
            file_handler.setLevel(logging.DEBUG)
            if anonymize:
                file_handler.addFilter(PathPrivacyFilter())
            print(f"Log anonymization: {'Enabled' if anonymize else 'Disabled'}")
        except Exception as e:
            print(f"ERROR: Cannot create log file handler: {e}", file=sys.stderr)
            file_handler = None
    console_handler = logging.StreamHandler()
    console_handler.setFormatter(log_formatter)
    console_handler.setLevel(logging.INFO)
    if anonymize:
        console_handler.addFilter(PathPrivacyFilter())
    logger = logging.getLogger()
    logger.setLevel(logging.DEBUG)
    for handler in logger.handlers[:]:
        try:
            handler.close()
            logger.removeHandler(handler)
        except Exception as e:
            logger.debug(f"Error removing handler: {e}")
    if file_handler:
        logger.addHandler(file_handler)
    logger.addHandler(console_handler)

    # Add the SledTraceFilter to reduce noise from sled::pagecache messages
    sled_filter = SledTraceFilter()
    logger.addFilter(sled_filter)

    logger.info(f"Filename anonymization in logs is {'ENABLED' if anonymize else 'DISABLED'}.")
    return actual_log_directory_used


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


def log_encoding_parameters(crf: int, preset: str, width: int, height: int, vmaf_target: float) -> None:
    """Log encoding parameters used for the video conversion.

    Args:
        crf: Constant Rate Factor value
        preset: Encoding preset name/value
        width: Video width in pixels
        height: Video height in pixels
        vmaf_target: Target VMAF score for quality
    """
    resolution_name = (
        "4K"
        if width >= 3840 or height >= 2160  # noqa: PLR2004
        else "1080p"
        if width >= 1920 or height >= 1080  # noqa: PLR2004
        else "720p"
        if width >= 1280 or height >= 720  # noqa: PLR2004
        else "SD"
    )
    logger.info(
        f"Encoding Parameters - Res: {resolution_name} ({width}x{height}), CRF: {crf}, "
        f"Preset: {preset}, VMAF Target: {vmaf_target}"
    )  # Log actual target used


def get_video_info(video_path: str, timeout: int = 30) -> dict[str, Any] | None:
    """Get video file information using ffprobe.

    Args:
        video_path: Path to the video file to analyze
        timeout: Maximum seconds to wait for ffprobe (default 30)

    Returns:
        Dictionary containing video metadata or None if analysis failed
    """
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
# Note: The main history logic is now in src/history_index.py
# These functions provide utility access for GUI and file operations.


def get_history_file_path() -> str:
    """Get the path to the conversion history JSON file (v2 format).

    Returns:
        Absolute path to the history file
    """
    return os.path.join(get_script_directory(), HISTORY_FILE_V2)


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
            for pattern in _PATH_PATTERNS:
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


# --- Power Management Functions ---
# Windows constants for SetThreadExecutionState
ES_CONTINUOUS = 0x80000000
ES_SYSTEM_REQUIRED = 0x00000001
ES_DISPLAY_REQUIRED = 0x00000002


def prevent_sleep_mode() -> bool:
    """Prevent the system from going to sleep while conversion is running.

    Returns:
        True if sleep prevention was successfully enabled, False otherwise
    """
    if sys.platform != "win32":
        logger.warning("Sleep prevention only supported on Windows")
        return False

    try:
        logger.info("Preventing system sleep during conversion")
        ctypes.windll.kernel32.SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)
        return True
    except Exception:
        logger.exception("Failed to prevent system sleep")
        return False


def allow_sleep_mode() -> bool:
    """Restore normal power management behavior.

    Returns:
        True if sleep settings were successfully restored, False otherwise
    """
    if sys.platform != "win32":
        return False

    try:
        logger.info("Restoring normal power management")
        ctypes.windll.kernel32.SetThreadExecutionState(ES_CONTINUOUS)
        return True
    except Exception:
        logger.exception("Failed to restore normal power management")
        return False
