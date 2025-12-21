# src/ab_av1/checker.py
"""
Checks for the availability and version of the ab-av1 executable.
"""

import json
import logging
import os
import subprocess
import urllib.request
from urllib.error import URLError

from src.utils import get_windows_subprocess_startupinfo

logger = logging.getLogger(__name__)

# GitHub API endpoint for ab-av1 releases
AB_AV1_GITHUB_API = "https://api.github.com/repos/alexheretic/ab-av1/releases/latest"


def check_ab_av1_available() -> tuple:
    """Check if ab-av1 executable is available in the parent 'src' directory.

    Returns:
        Tuple of (is_available, path, message) where:
        - is_available: Boolean indicating whether ab-av1 is available
        - path: Path to the ab-av1 executable if found, otherwise the expected path.
        - message: Descriptive message about the result
    """
    # Determine expected path relative to *this* file's location
    # Assuming this file is src/ab_av1/checker.py, go up one level to src/
    script_dir = os.path.dirname(os.path.abspath(__file__))  # src/ab_av1/
    src_dir = os.path.dirname(script_dir)  # src/
    expected_path = os.path.join(src_dir, "ab-av1.exe")
    expected_path = os.path.abspath(expected_path)  # Get absolute path

    if os.path.exists(expected_path):
        logger.info(f"ab-av1 found: {expected_path}")
        return True, expected_path, f"ab-av1 available at {expected_path}"
    # Adjusted error message
    error_msg = f"ab-av1.exe not found. Place inside 'src' dir.\nExpected: {expected_path}"
    logger.error(error_msg)
    return False, expected_path, error_msg


def get_ab_av1_version() -> str | None:
    """Get the version of the local ab-av1 executable.

    Returns:
        Version string (e.g., "0.9.4") or None if unavailable.
    """
    available, exe_path, _ = check_ab_av1_available()
    if not available:
        return None

    try:
        startupinfo, creationflags = get_windows_subprocess_startupinfo()
        result = subprocess.run(
            [exe_path, "--version"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
            startupinfo=startupinfo,
            creationflags=creationflags,
        )
        if result.returncode == 0 and result.stdout:
            # Output is "ab-av1 X.Y.Z", extract version
            parts = result.stdout.strip().split(maxsplit=1)
            if len(parts) > 1:
                return parts[1]
        return None
    except (subprocess.TimeoutExpired, OSError) as e:
        logger.warning(f"Failed to get ab-av1 version: {e}")
        return None


def check_ab_av1_latest_github() -> tuple[str | None, str | None, str]:
    """Check GitHub for the latest ab-av1 release version.

    This function makes a network request and should only be called
    when the user explicitly requests a version check.

    Returns:
        Tuple of (latest_version, release_url, message) where:
        - latest_version: Version string (e.g., "0.9.4") or None on error
        - release_url: URL to the release page, or None on error
        - message: Descriptive message about the result
    """
    try:
        request = urllib.request.Request(  # noqa: S310 - hardcoded https URL is safe
            AB_AV1_GITHUB_API,
            headers={"Accept": "application/vnd.github.v3+json", "User-Agent": "Auto-AV1-Converter"},
        )
        with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310
            data = json.loads(response.read().decode("utf-8"))
            tag_name = data.get("tag_name", "")
            html_url = data.get("html_url", "")

            # Tag is usually "vX.Y.Z", strip the 'v' prefix if present
            version = tag_name.lstrip("v") if tag_name else None

            if version:
                return version, html_url, f"Latest version: {version}"
            return None, None, "Could not parse version from GitHub response"

    except URLError as e:
        logger.warning(f"Failed to check GitHub for ab-av1 updates: {e}")
        return None, None, f"Network error: {e.reason}"
    except json.JSONDecodeError as e:
        logger.warning(f"Failed to parse GitHub response: {e}")
        return None, None, "Failed to parse GitHub response"
    except Exception as e:
        logger.warning(f"Unexpected error checking GitHub: {e}")
        return None, None, f"Error: {e}"
