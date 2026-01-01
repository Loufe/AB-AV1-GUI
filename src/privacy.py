# src/privacy.py
"""Path anonymization utilities using BLAKE2b hashing for privacy protection."""

import hashlib
import logging
import os
import re
import sys

logger = logging.getLogger(__name__)


# --- Path Anonymization (BLAKE2b Hash-Based) ---

# Configured folders for special labeling (set via set_anonymization_folders)
_configured_input_folder: str | None = None
_configured_output_folder: str | None = None

# Cache for hash lookups (avoids recomputing)
_path_hash_cache: dict[str, str] = {}


def normalize_path(path: str) -> str:
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


def compute_hash(value: str, length: int = 12) -> str:
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
    _configured_input_folder = normalize_path(input_folder) if input_folder else None
    _configured_output_folder = normalize_path(output_folder) if output_folder else None


def anonymize_folder(folder_path: str) -> str:
    """Anonymize a folder path using BLAKE2b hash or special labels.

    Args:
        folder_path: Full path to the folder

    Returns:
        Anonymized folder identifier like '[input_folder]' or 'folder_7f3a9c2b1e4d'
    """
    if not folder_path:
        return "[unknown]"

    normalized = normalize_path(folder_path)

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
    folder_hash = compute_hash(normalized)
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
    file_hash = compute_hash(name_without_ext)
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
    - Full paths -> anonymize_path() (hashes both folder and filename)
    - Bare filenames -> anonymize_file() (hashes just the filename)

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
PATH_PATTERNS = [
    # Windows drive paths: C:\... or C:/...
    re.compile(r"[A-Za-z]:[\\\/][^\s\"'<>|*?\n]+"),
    # UNC paths: \\server\share\...
    re.compile(r"\\\\[^\s\"'<>|*?\n]+"),
    # Unix absolute paths with common roots
    re.compile(r"/(?:home|Users|mnt|media|var|tmp|opt|usr|root|srv|run|data)[^\s\"'<>|*?\n]*"),
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
            for pattern in PATH_PATTERNS:
                temp_msg = pattern.sub(_anonymize_path_match, temp_msg)
            record.msg = temp_msg
        return True
