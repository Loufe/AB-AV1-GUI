# src/conversion_engine/cleanup.py
"""
Contains cleanup functions related to the conversion process,
like scheduling the removal of temporary folders.
"""
import logging

# Project Imports
from src.ab_av1.cleaner import clean_ab_av1_temp_folders  # Use the specific cleaner

logger = logging.getLogger(__name__)


def schedule_temp_folder_cleanup(directory: str) -> None:
    """Schedules cleanup of ab-av1 temporary folders (e.g., '.ab-av1-*').

    Logs the attempt and result of cleaning temporary folders typically found
    in the output directory. This is often called after a batch completes or
    is stopped.

    Args:
        directory: The directory where temporary folders might be located
                   (usually the output directory).
    """
    if not directory:
        logger.warning("Cleanup requested but directory is not specified. Skipping.")
        return

    try:
        logger.info(f"Attempting cleanup of '.ab-av1-*' temp folders in: {directory}")
        # Call the specific cleaner function from the ab_av1 package
        cleaned_count = clean_ab_av1_temp_folders(directory)
        if cleaned_count > 0:
            logger.info(f"Successfully cleaned up {cleaned_count} '.ab-av1-*' temp folder(s) in {directory}.")
        else:
            logger.info(f"No '.ab-av1-*' temp folders found to clean in {directory}.")
    except Exception as e:
        # Catch potential errors during the cleanup process itself
        logger.warning(f"Error occurred during scheduled cleanup in {directory}: {e!s}", exc_info=True)
