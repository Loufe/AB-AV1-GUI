# src/ab_av1/cleaner.py
"""
Utility functions for cleaning up temporary files/folders created by ab-av1.
"""
import os
import shutil
import logging
from pathlib import Path

logger = logging.getLogger(__name__)

def clean_ab_av1_temp_folders(base_dir: str = None) -> int:
    """Clean up temporary folders created by ab-av1 (typically named '.ab-av1-*').

    Args:
        base_dir: Directory to search for temp folders. Defaults to current working directory
                  if None, but ideally the output directory should be provided.

    Returns:
        Number of temporary folders successfully cleaned up.
    """
    # Determine base directory
    if base_dir is None:
        # Using CWD as a fallback is less reliable; prefer explicit output dir
        base_dir = os.getcwd()
        logger.warning(f"Cleaning temp folders in fallback CWD: {base_dir}. Prefer providing output directory.")
    else:
        logger.debug(f"Cleaning temp folders in: {base_dir}")

    # Find temp folders matching the pattern '.ab-av1-*'
    try:
        base_path = Path(base_dir)
        # Ensure the base directory exists and is a directory
        if not base_path.is_dir():
            logger.warning(f"Base directory for cleanup is invalid or does not exist: {base_dir}")
            return 0

        pattern = ".ab-av1-*"
        # Use glob to find items matching the pattern directly within the base_path
        temp_items = list(base_path.glob(pattern))
        logger.debug(f"Found {len(temp_items)} potential temp items matching '{pattern}' in {base_dir}")

    except Exception as e:
        logger.error(f"Error finding temp folders in {base_dir}: {e}")
        return 0

    # Remove the found folders
    cleaned_count = 0
    for item in temp_items:
        try:
            # Ensure we are only removing directories, not files with similar names
            if item.is_dir():
                shutil.rmtree(item)
                logger.info(f"Cleaned temp folder: {item}")
                cleaned_count += 1
            else:
                # Log if a matching item wasn't a directory (unexpected but possible)
                logger.debug(f"Skipping non-directory item found matching pattern: {item}")
        except Exception as e:
            # Log specific errors during removal
            logger.warning(f"Failed to clean up temporary item {item}: {str(e)}")

    if cleaned_count == 0 and temp_items:
        logger.info(f"Found {len(temp_items)} potential temp items but none were removed (check permissions or if they were directories).")
    elif cleaned_count > 0:
        logger.info(f"Successfully cleaned {cleaned_count} temporary folder(s) in {base_dir}.")

    return cleaned_count