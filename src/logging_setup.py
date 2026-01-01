# src/logging_setup.py
"""Logging configuration and setup for the AV1 Video Converter application."""

import datetime
import logging
import os
import sys
from logging.handlers import RotatingFileHandler

from src.privacy import PathPrivacyFilter

logger = logging.getLogger(__name__)


# Custom filter to suppress excessive sled::pagecache trace messages
class SledTraceFilter(logging.Filter):
    def filter(self, record):
        return not (
            hasattr(record, "msg")
            and isinstance(record.msg, str)
            and "sled::pagecache" in record.msg
            and "TRACE" in record.msg
        )


def get_script_directory() -> str:
    """Get the directory containing the main script/executable.

    Returns:
        Absolute path to the directory containing the main script or executable
    """
    if getattr(sys, "frozen", False):
        return os.path.dirname(sys.executable)
    if "__file__" in globals():
        script_path = os.path.abspath(__file__)
        # Navigate up one level from src/logging_setup.py to the script directory
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

    root_logger = logging.getLogger()
    root_logger.setLevel(logging.DEBUG)

    for handler in root_logger.handlers[:]:
        try:
            handler.close()
            root_logger.removeHandler(handler)
        except Exception as e:
            root_logger.debug(f"Error removing handler: {e}")

    if file_handler:
        root_logger.addHandler(file_handler)
    root_logger.addHandler(console_handler)

    # Add the SledTraceFilter to reduce noise from sled::pagecache messages
    sled_filter = SledTraceFilter()
    root_logger.addFilter(sled_filter)

    root_logger.info(f"Filename anonymization in logs is {'ENABLED' if anonymize else 'DISABLED'}.")
    return actual_log_directory_used
