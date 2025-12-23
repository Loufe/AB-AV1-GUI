# src/cache_helpers.py
"""Helper functions for CRF cache validation and reuse logic.

The quality analysis (Layer 2) caches CRF search results in FileRecord.
These helpers determine when cached results can be reused during conversion,
avoiding redundant CRF searches.
"""

import logging
import os

from src.history_index import compute_path_hash
from src.models import FileRecord

logger = logging.getLogger(__name__)


def is_file_unchanged(record: FileRecord, file_path: str) -> bool:
    """Check if a file's size and mtime match the cached record.

    Args:
        record: FileRecord with cached file metadata.
        file_path: Path to the current file.

    Returns:
        True if file size and mtime match the record, False otherwise.
    """
    expected_hash = compute_path_hash(file_path)
    if record.path_hash != expected_hash:
        logger.error("Record/file path hash mismatch in cache validation")
        return False

    try:
        stat_info = os.stat(file_path)
        return (
            record.file_size_bytes == stat_info.st_size
            and record.file_mtime == stat_info.st_mtime
        )
    except OSError as e:
        logger.warning(f"Could not stat file for cache validation: {e}")
        return False


def can_reuse_crf(
    record: FileRecord,
    desired_vmaf: int,
    desired_preset: int,
) -> bool:
    """Check if cached CRF can be reused for conversion.

    Cache is valid when:
    - CRF was found during analysis (best_crf is set)
    - Preset matches exactly (different presets produce different quality at same CRF)
    - Cached VMAF target >= desired VMAF target (if we achieved 95, we can definitely achieve 90)

    Args:
        record: FileRecord with cached analysis results.
        desired_vmaf: The VMAF target for the current conversion.
        desired_preset: The encoding preset for the current conversion.

    Returns:
        True if cached CRF can be reused, False otherwise.
    """
    if record.best_crf is None:
        return False

    if record.preset_when_analyzed is None:
        # Old cache entry without preset info - can't validate
        return False

    if record.vmaf_target_when_analyzed is None:
        return False

    # Preset must match exactly - different presets give different quality at same CRF
    if record.preset_when_analyzed != desired_preset:
        logger.debug(
            f"Cache invalid: preset mismatch (cached={record.preset_when_analyzed}, "
            f"desired={desired_preset})"
        )
        return False

    # If cached VMAF >= desired, the cached CRF will achieve at least the desired quality
    if record.vmaf_target_when_analyzed >= desired_vmaf:
        logger.debug(
            f"Cache valid: VMAF {record.vmaf_target_when_analyzed} >= desired {desired_vmaf}, "
            f"CRF={record.best_crf}"
        )
        return True

    logger.debug(
        f"Cache invalid: VMAF mismatch (cached={record.vmaf_target_when_analyzed}, "
        f"desired={desired_vmaf})"
    )
    return False
