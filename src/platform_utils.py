# src/platform_utils.py
"""Platform-specific utilities for Windows subprocess handling and power management."""

import ctypes
import logging
import os
import signal
import subprocess
import sys
import time
from typing import Any

from src.config import TASKKILL_NOT_FOUND_RC

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


def terminate_process_tree(pid: int) -> bool:
    """Forcefully terminate a process and its whole child tree.

    On Windows uses ``taskkill /F /T`` so ab-av1's ffmpeg children die with it.
    On POSIX signals the process *group* (SIGTERM, escalating to SIGKILL) -
    the runner spawns ab-av1 as a session leader so its group covers ffmpeg
    children even when they are hung and cannot forward signals.

    Returns:
        True if the process is gone (or was already gone), False on error.
    """
    if sys.platform == "win32":
        try:
            startupinfo, creationflags = get_windows_subprocess_startupinfo()
            result = subprocess.run(
                ["taskkill", "/F", "/T", "/PID", str(pid)],
                capture_output=True,
                text=True,
                check=False,
                startupinfo=startupinfo,
                creationflags=creationflags,
            )
            if result.returncode == 0:
                logger.info(f"Terminated PID {pid} and its child processes via taskkill.")
                return True
            if result.returncode == TASKKILL_NOT_FOUND_RC:
                logger.info(f"Process PID {pid} not found during termination attempt (already gone).")
                return True
            logger.warning(f"taskkill failed for PID {pid} (rc={result.returncode}): {result.stderr.strip()}")
            return False
        except Exception:
            logger.exception(f"Failed to terminate process tree for PID {pid}")
            return False

    try:
        # SIGKILL cannot be forwarded by ab-av1, so signal the whole group -
        # otherwise a hung ffmpeg child survives the very kill that the
        # silence-timeout path exists for.
        pgid = os.getpgid(pid)
        os.killpg(pgid, signal.SIGTERM)
        time.sleep(0.5)
        try:
            os.kill(pid, 0)  # Check if process still exists
        except ProcessLookupError:
            logger.info(f"Process group {pgid} terminated successfully with SIGTERM.")
            return True
        logger.warning(f"Process {pid} still alive after SIGTERM, sending SIGKILL to group {pgid}.")
        os.killpg(pgid, signal.SIGKILL)
        return True
    except ProcessLookupError:
        logger.info(f"Process PID {pid} not found during termination attempt (already gone).")
        return True
    except Exception:
        logger.exception(f"Failed to terminate process PID {pid}")
        return False


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
