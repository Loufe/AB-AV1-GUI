# src/platform_utils.py
"""Platform-specific utilities for Windows subprocess handling and power management."""

import ctypes
import logging
import subprocess
import sys
from typing import Any

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
