# src/ab_av1/checker.py
"""
Checks for the availability of the ab-av1 executable.
"""
import logging
import os

logger = logging.getLogger(__name__)

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
    script_dir = os.path.dirname(os.path.abspath(__file__)) # src/ab_av1/
    src_dir = os.path.dirname(script_dir) # src/
    expected_path = os.path.join(src_dir, "ab-av1.exe")
    expected_path = os.path.abspath(expected_path) # Get absolute path

    if os.path.exists(expected_path):
        logger.info(f"ab-av1 found: {expected_path}")
        return True, expected_path, f"ab-av1 available at {expected_path}"
    # Adjusted error message
    error_msg = f"ab-av1.exe not found. Place inside 'src' dir.\nExpected: {expected_path}"
    logger.error(error_msg)
    return False, expected_path, error_msg
