# src/cache_helpers.py
"""Helper functions for CRF cache validation and reuse logic.

The quality analysis (Layer 2) caches CRF search results in FileRecord.
These helpers determine when cached results can be reused during conversion,
avoiding redundant CRF searches.
"""

import logging
import os

from src.config import MTIME_TOLERANCE
from src.history_index import compute_path_hash
from src.models import FileRecord

logger = logging.getLogger(__name__)


def mtimes_match(mtime1: float, mtime2: float) -> bool:
    """Compare two modification times with tolerance for JSON precision loss.

    File mtimes are floats that can lose precision when serialized to JSON.
    This function compares them with a tolerance of 1 second.

    Args:
        mtime1: First modification time.
        mtime2: Second modification time.

    Returns:
        True if mtimes are within tolerance, False otherwise.
    """
    return abs(mtime1 - mtime2) < MTIME_TOLERANCE


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
        return record.file_size_bytes == stat_info.st_size and mtimes_match(record.file_mtime, stat_info.st_mtime)
    except OSError as e:
        logger.warning(f"Could not stat file for cache validation: {e}")
        return False


def converted_verdict_applies(record: FileRecord, file_path: str) -> bool:
    """Check if a CONVERTED record still describes the file at this path.

    A plain stamp check is wrong for CONVERTED records: in replace mode the AV1
    output sits at the input path, so changed stamps are the expected steady
    state of a completed conversion, not evidence of new content. The steady
    state is recognized without ffprobe: replace mode always outputs
    ``input.with_suffix(".mkv")``, so only a .mkv path can hold our own output,
    identified by its size matching the recorded output size.

    Args:
        record: The CONVERTED FileRecord for this path.
        file_path: Path to the current file.

    Returns:
        True if the verdict still applies (skip the file), False if the
        content at the path genuinely changed (eligible for re-processing).
    """
    if is_file_unchanged(record, file_path):
        # Untouched input (suffix/separate-folder modes), or a replace-mode
        # output whose stamps a later scan refreshed onto the record.
        return True

    if record.output_size_bytes is None:
        # Legacy record without Layer-3 size: can't discriminate output from
        # changed content - stay conservative and keep the verdict.
        return True

    if not file_path.lower().endswith(".mkv"):
        return False  # Our output never sits at a non-mkv path

    try:
        return os.path.getsize(file_path) == record.output_size_bytes
    except OSError as e:
        logger.warning(f"Could not stat file for converted-verdict check: {e}")
        return True  # Conservative: keep the verdict rather than re-queue


def can_reuse_crf(record: FileRecord, desired_vmaf: int, desired_preset: int) -> bool:
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
        logger.debug(f"Cache invalid: preset mismatch (cached={record.preset_when_analyzed}, desired={desired_preset})")
        return False

    # If cached VMAF >= desired, the cached CRF will achieve at least the desired quality
    if record.vmaf_target_when_analyzed >= desired_vmaf:
        logger.debug(
            f"Cache valid: VMAF {record.vmaf_target_when_analyzed} >= desired {desired_vmaf}, CRF={record.best_crf}"
        )
        return True

    logger.debug(f"Cache invalid: VMAF mismatch (cached={record.vmaf_target_when_analyzed}, desired={desired_vmaf})")
    return False
