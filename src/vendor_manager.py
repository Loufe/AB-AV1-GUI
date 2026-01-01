# src/vendor_manager.py
"""
Manages downloading and updating external vendor tools (ab-av1, FFmpeg).

All vendor binaries are stored in the vendor/ directory at the project root.
This directory is gitignored and created on demand.
"""

import json
import logging
import shutil
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path
from typing import Callable
from urllib.error import URLError

logger = logging.getLogger(__name__)

# --- Path Configuration ---


def _get_project_root() -> Path:
    """Get project root directory, handling both script and frozen exe."""
    if getattr(sys, "frozen", False):
        # Frozen exe: project root is directory containing the executable
        return Path(sys.executable).parent
    # Normal script: parent of src/
    return Path(__file__).parent.parent


PROJECT_ROOT = _get_project_root()
VENDOR_DIR = PROJECT_ROOT / "vendor"

# Tool-specific paths (platform-aware)
AB_AV1_DIR = VENDOR_DIR / "ab-av1"
AB_AV1_EXE_NAME = "ab-av1.exe" if sys.platform == "win32" else "ab-av1"
AB_AV1_EXE = AB_AV1_DIR / AB_AV1_EXE_NAME

FFMPEG_DIR = VENDOR_DIR / "ffmpeg"
FFMPEG_EXE_NAME = "ffmpeg.exe" if sys.platform == "win32" else "ffmpeg"
FFPROBE_EXE_NAME = "ffprobe.exe" if sys.platform == "win32" else "ffprobe"
FFMPEG_EXE = FFMPEG_DIR / FFMPEG_EXE_NAME
FFPROBE_EXE = FFMPEG_DIR / FFPROBE_EXE_NAME

# --- GitHub API Endpoints ---

AB_AV1_GITHUB_API = "https://api.github.com/repos/alexheretic/ab-av1/releases/latest"
FFMPEG_GYAN_GITHUB_API = "https://api.github.com/repos/GyanD/codexffmpeg/releases/latest"


def ensure_vendor_dir() -> None:
    """Create vendor directory structure if it doesn't exist."""
    AB_AV1_DIR.mkdir(parents=True, exist_ok=True)
    FFMPEG_DIR.mkdir(parents=True, exist_ok=True)


def get_ab_av1_path() -> Path | None:
    """Get path to ab-av1, checking vendor directory first, then system PATH.

    Returns:
        Path to ab-av1 executable or None if not found anywhere.
    """
    # Check vendor directory first
    if AB_AV1_EXE.exists():
        return AB_AV1_EXE
    # Fall back to system PATH (important for Linux/macOS where ab-av1 is installed via cargo)
    system_ab_av1 = shutil.which("ab-av1")
    if system_ab_av1:
        return Path(system_ab_av1)
    return None


def is_using_vendor_ab_av1() -> bool:
    """Check if we're using the vendor-provided ab-av1 (not system PATH)."""
    return AB_AV1_EXE.exists()


def get_ffmpeg_path() -> Path | None:
    """Get path to ffmpeg, checking vendor directory first, then system PATH.

    Returns:
        Path to ffmpeg executable or None if not found anywhere.
    """
    # Check vendor directory first
    if FFMPEG_EXE.exists():
        return FFMPEG_EXE
    # Fall back to system PATH
    system_ffmpeg = shutil.which("ffmpeg")
    if system_ffmpeg:
        return Path(system_ffmpeg)
    return None


def get_ffprobe_path() -> Path | None:
    """Get path to ffprobe, checking vendor directory first, then system PATH.

    Returns:
        Path to ffprobe executable or None if not found anywhere.
    """
    # Check vendor directory first
    if FFPROBE_EXE.exists():
        return FFPROBE_EXE
    # Fall back to system PATH
    system_ffprobe = shutil.which("ffprobe")
    if system_ffprobe:
        return Path(system_ffprobe)
    return None


def is_using_vendor_ffmpeg() -> bool:
    """Check if we're using the vendor-provided FFmpeg (not system PATH)."""
    return FFMPEG_EXE.exists()


# --- GitHub API Helpers ---


def _github_request(url: str) -> dict:
    """Make a GitHub API request and return JSON response."""
    request = urllib.request.Request(  # noqa: S310 - hardcoded https GitHub URL is safe
        url, headers={"Accept": "application/vnd.github.v3+json", "User-Agent": "Auto-AV1-Converter"}
    )
    with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310
        return json.loads(response.read().decode("utf-8"))


def _download_file(url: str, dest_path: Path, progress_callback: Callable[[int, int], None] | None = None) -> None:
    """Download a file from URL to destination path.

    Args:
        url: URL to download from
        dest_path: Destination file path
        progress_callback: Optional callback(bytes_downloaded, total_bytes)
    """
    request = urllib.request.Request(  # noqa: S310 - downloading from known GitHub URLs
        url, headers={"User-Agent": "Auto-AV1-Converter"}
    )

    with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
        total_size = int(response.headers.get("Content-Length", 0))
        downloaded = 0

        with open(dest_path, "wb") as f:
            while True:
                chunk = response.read(8192)
                if not chunk:
                    break
                f.write(chunk)
                downloaded += len(chunk)
                if progress_callback:
                    progress_callback(downloaded, total_size)


# --- ab-av1 Download ---


def get_ab_av1_latest_release() -> tuple[str | None, str | None, str | None]:
    """Get information about the latest ab-av1 release from GitHub.

    Returns:
        Tuple of (version, download_url, release_page_url) or (None, None, None) on error.
    """
    try:
        data = _github_request(AB_AV1_GITHUB_API)
        tag_name = data.get("tag_name", "")
        html_url = data.get("html_url", "")

        # Find the Windows executable asset
        download_url = None
        for asset in data.get("assets", []):
            name = asset.get("name", "")
            if name == "ab-av1.exe" or (name.endswith(".exe") and "windows" in name.lower()):
                download_url = asset.get("browser_download_url")
                break

        version = tag_name.lstrip("v") if tag_name else None
        return version, download_url, html_url

    except (URLError, json.JSONDecodeError, KeyError) as e:
        logger.warning(f"Failed to get ab-av1 release info: {e}")
        return None, None, None


def download_ab_av1(progress_callback: Callable[[int, int], None] | None = None) -> tuple[bool, str]:
    """Download the latest ab-av1.exe to vendor directory.

    Args:
        progress_callback: Optional callback(bytes_downloaded, total_bytes)

    Returns:
        Tuple of (success, message)
    """
    version, download_url, _ = get_ab_av1_latest_release()

    if not download_url:
        return False, "Could not find ab-av1 download URL"

    try:
        ensure_vendor_dir()

        # Download to temp file first, then move
        with tempfile.NamedTemporaryFile(delete=False, suffix=".exe") as tmp:
            tmp_path = Path(tmp.name)

        try:
            logger.info(f"Downloading ab-av1 {version} from {download_url}")
            _download_file(download_url, tmp_path, progress_callback)

            # Move to final location
            shutil.move(str(tmp_path), str(AB_AV1_EXE))
            logger.info(f"ab-av1 {version} installed to {AB_AV1_EXE}")
            return True, f"Successfully installed ab-av1 {version}"

        finally:
            # Clean up temp file if it still exists
            if tmp_path.exists():
                tmp_path.unlink()

    except URLError as e:
        logger.exception("Network error downloading ab-av1")
        return False, f"Network error: {e.reason}"
    except OSError as e:
        logger.exception("File error installing ab-av1")
        return False, f"File error: {e}"
    except Exception as e:
        logger.exception("Unexpected error downloading ab-av1")
        return False, f"Error: {e}"


# --- FFmpeg Download ---


def get_ffmpeg_latest_release() -> tuple[str | None, str | None, str | None]:
    """Get information about the latest FFmpeg full build from gyan.dev.

    Returns:
        Tuple of (version, download_url, release_page_url) or (None, None, None) on error.
    """
    try:
        data = _github_request(FFMPEG_GYAN_GITHUB_API)
        tag_name = data.get("tag_name", "")
        html_url = data.get("html_url", "")

        # Find the full build zip asset (not essentials - it lacks libsvtav1)
        download_url = None
        for asset in data.get("assets", []):
            name = asset.get("name", "")
            # Look for: ffmpeg-X.Y.Z-full_build.zip (prefer zip over 7z for stdlib support)
            if "full" in name.lower() and name.endswith(".zip"):
                download_url = asset.get("browser_download_url")
                break

        # If no zip found, try 7z (will need external tool to extract)
        if not download_url:
            for asset in data.get("assets", []):
                name = asset.get("name", "")
                if "full" in name.lower() and name.endswith(".7z"):
                    download_url = asset.get("browser_download_url")
                    break

        version = tag_name.lstrip("v") if tag_name else None
        return version, download_url, html_url

    except (URLError, json.JSONDecodeError, KeyError) as e:
        logger.warning(f"Failed to get FFmpeg release info: {e}")
        return None, None, None


def download_ffmpeg(progress_callback: Callable[[int, int], None] | None = None) -> tuple[bool, str]:
    """Download the latest FFmpeg full build to vendor/ffmpeg/.

    Args:
        progress_callback: Optional callback(bytes_downloaded, total_bytes)

    Returns:
        Tuple of (success, message)
    """
    version, download_url, _ = get_ffmpeg_latest_release()

    if not download_url:
        return False, "Could not find FFmpeg download URL"

    if download_url.endswith(".7z"):
        return False, "FFmpeg 7z extraction not supported. Please install FFmpeg manually or wait for zip release."

    ensure_vendor_dir()

    try:
        # Download to temp file
        with tempfile.NamedTemporaryFile(delete=False, suffix=".zip") as tmp:
            tmp_path = Path(tmp.name)

        try:
            logger.info(f"Downloading FFmpeg {version} from {download_url}")
            _download_file(download_url, tmp_path, progress_callback)

            # Extract zip
            logger.info("Extracting FFmpeg...")
            with tempfile.TemporaryDirectory() as extract_dir:
                extract_path = Path(extract_dir)

                with zipfile.ZipFile(tmp_path, "r") as zf:
                    zf.extractall(extract_path)

                # Find the bin directory (structure: ffmpeg-X.Y.Z-full_build/bin/)
                bin_dir = None
                for item in extract_path.iterdir():
                    if item.is_dir():
                        potential_bin = item / "bin"
                        if potential_bin.exists():
                            bin_dir = potential_bin
                            break

                if not bin_dir:
                    return False, "Could not find bin directory in FFmpeg archive"

                if not (bin_dir / "ffmpeg.exe").exists():
                    return False, "ffmpeg.exe not found in archive"

                # Copy all files from bin/ (exe + DLLs) to vendor/ffmpeg/
                for src_file in bin_dir.iterdir():
                    if src_file.is_file():
                        dest_file = FFMPEG_DIR / src_file.name
                        if dest_file.exists():
                            dest_file.unlink()
                        shutil.copy2(str(src_file), str(dest_file))

                logger.info(f"FFmpeg {version} installed to {FFMPEG_DIR}")
                return True, f"Successfully installed FFmpeg {version}"

        finally:
            # Clean up temp file
            if tmp_path.exists():
                tmp_path.unlink()

    except URLError as e:
        logger.exception("Network error downloading FFmpeg")
        return False, f"Network error: {e.reason}"
    except zipfile.BadZipFile:
        logger.exception("Invalid FFmpeg archive")
        return False, "Downloaded file is not a valid zip archive"
    except OSError as e:
        logger.exception("File error installing FFmpeg")
        return False, f"File error: {e}"
    except Exception as e:
        logger.exception("Unexpected error downloading FFmpeg")
        return False, f"Error: {e}"
