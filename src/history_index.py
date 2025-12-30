# src/history_index.py
"""
In-memory index for the conversion history system.

Provides O(1) lookups by path hash with thread-safe access and automatic
cache validation based on file size and modification time.
"""

import contextlib
import dataclasses
import json
import logging
import os
import threading
from typing import Optional

from src.config import HISTORY_FILE_V2, MAX_CRF_VALUE, MAX_VMAF_VALUE, RESOLUTION_TOLERANCE_PERCENT
from src.models import FileRecord, FileStatus
from src.utils import _compute_hash, _normalize_path, get_script_directory

logger = logging.getLogger(__name__)


def _validate_record(record: FileRecord) -> bool:
    """Validate that a FileRecord has sensible field values.

    Args:
        record: The FileRecord to validate.

    Returns:
        True if all fields are valid, False otherwise.
    """
    # Validate file_size_bytes (required field)
    if record.file_size_bytes < 0:
        logger.warning(f"Invalid file_size_bytes ({record.file_size_bytes}) for record {record.path_hash}")
        return False

    # Validate optional numeric fields
    if record.duration_sec is not None and record.duration_sec < 0:
        logger.warning(f"Invalid duration_sec ({record.duration_sec}) for record {record.path_hash}")
        return False

    if record.best_crf is not None and not (0 <= record.best_crf <= MAX_CRF_VALUE):
        logger.warning(f"Invalid best_crf ({record.best_crf}) for record {record.path_hash}")
        return False

    if record.best_vmaf_achieved is not None and not (0 <= record.best_vmaf_achieved <= MAX_VMAF_VALUE):
        logger.warning(f"Invalid best_vmaf_achieved ({record.best_vmaf_achieved}) for record {record.path_hash}")
        return False

    if record.predicted_size_reduction is not None and not (
        0 <= record.predicted_size_reduction <= 100  # noqa: PLR2004
    ):
        logger.warning(
            f"Invalid predicted_size_reduction ({record.predicted_size_reduction}) for record {record.path_hash}"
        )
        return False

    if record.width is not None and record.width <= 0:
        logger.warning(f"Invalid width ({record.width}) for record {record.path_hash}")
        return False

    if record.height is not None and record.height <= 0:
        logger.warning(f"Invalid height ({record.height}) for record {record.path_hash}")
        return False

    return True


def compute_path_hash(file_path: str) -> str:
    """Compute a unique hash for a file path.

    Uses BLAKE2b hash of the normalized path. The hash is 16 characters
    for sufficient uniqueness across large file collections.

    Args:
        file_path: Absolute or relative path to the file.

    Returns:
        16-character hex hash string.
    """
    normalized = _normalize_path(file_path)
    return _compute_hash(normalized, length=16)


def get_history_v2_path() -> str:
    """Get the path to the v2 history file.

    Returns:
        Absolute path to conversion_history_v2.json.
    """
    return os.path.join(get_script_directory(), HISTORY_FILE_V2)


class HistoryIndex:
    """Thread-safe in-memory index for conversion history.

    Provides:
    - O(1) lookup by path_hash
    - Cache validation based on file size and mtime
    - Thread-safe access for concurrent conversion/analysis
    - Lazy loading from disk on first access
    - Atomic saves to prevent corruption

    Usage:
        index = get_history_index()
        record = index.lookup_file("/path/to/video.mp4")
        if record and index.is_valid("/path/to/video.mp4"):
            # Use cached data
            ...
    """

    def __init__(self):
        """Initialize an empty index."""
        self._records: dict[str, FileRecord] = {}
        self._lock = threading.RLock()
        self._loaded = False
        self._dirty = False
        self._converted_cache: list[FileRecord] | None = None

    def get(self, path_hash: str) -> Optional[FileRecord]:
        """Get a record by its path hash.

        Args:
            path_hash: The 16-character hash of the normalized path.

        Returns:
            The FileRecord if found, None otherwise.
        """
        with self._lock:
            self._ensure_loaded()
            return self._records.get(path_hash)

    def lookup_file(self, file_path: str) -> Optional[FileRecord]:
        """Look up a record by file path.

        Computes the path hash and retrieves the record.

        Args:
            file_path: Path to the file to look up.

        Returns:
            The FileRecord if found, None otherwise.
        """
        path_hash = compute_path_hash(file_path)
        return self.get(path_hash)

    def upsert(self, record: FileRecord) -> None:
        """Insert or update a record.

        The record's path_hash is used as the key. If a record with the
        same path_hash already exists, it is replaced.

        Args:
            record: The FileRecord to insert or update.
        """
        with self._lock:
            self._ensure_loaded()
            # Invalidate converted cache if this affects converted records
            old_record = self._records.get(record.path_hash)
            if record.status == FileStatus.CONVERTED or (old_record and old_record.status == FileStatus.CONVERTED):
                self._converted_cache = None
            self._records[record.path_hash] = record
            self._dirty = True

    def get_by_status(self, status: FileStatus) -> list[FileRecord]:
        """Get all records with a given status.

        Args:
            status: The status to filter by.

        Returns:
            List of FileRecords with the specified status.
        """
        with self._lock:
            self._ensure_loaded()
            return [r for r in self._records.values() if r.status == status]

    def get_converted_records(self) -> list[FileRecord]:
        """Get all successfully converted records.

        Convenience method for time/size estimation based on historical data.
        Results are cached until a CONVERTED record is added or modified.

        Returns:
            List of FileRecords with CONVERTED status.
        """
        with self._lock:
            self._ensure_loaded()
            if self._converted_cache is None:
                self._converted_cache = [r for r in self._records.values() if r.status == FileStatus.CONVERTED]
            return self._converted_cache

    def get_all_records(self) -> list[FileRecord]:
        """Get all records in the index.

        Returns:
            List of all FileRecords.
        """
        with self._lock:
            self._ensure_loaded()
            return list(self._records.values())

    def find_similar(
        self, video_codec: str, width: int, resolution_tolerance: float = RESOLUTION_TOLERANCE_PERCENT
    ) -> list[FileRecord]:
        """Find converted records with similar video properties.

        Used for estimating conversion results for new files.

        Args:
            video_codec: The video codec to match (e.g., "h264").
            width: The video width in pixels.
            resolution_tolerance: Maximum relative difference in width (default from config).

        Returns:
            List of converted FileRecords with similar properties.
        """
        similar = []
        for record in self.get_converted_records():
            if record.video_codec != video_codec:
                continue
            if record.width is None or width == 0:
                continue
            # Check resolution similarity
            width_diff = abs(record.width - width) / max(record.width, width)
            if width_diff <= resolution_tolerance:
                similar.append(record)
        return similar

    def save(self) -> None:
        """Persist changes to disk.

        Uses atomic write (write to temp file, then rename) to prevent
        corruption on crash.
        """
        with self._lock:
            if not self._dirty:
                return
            self._save_to_disk()
            self._dirty = False

    def _ensure_loaded(self) -> None:
        """Load from disk if not already loaded."""
        if not self._loaded:
            self._load_from_disk()
            self._loaded = True

    def _load_from_disk(self) -> None:
        """Load records from the JSON history file."""
        history_path = get_history_v2_path()

        if not os.path.exists(history_path):
            logger.info(f"History file not found, starting fresh: {history_path}")
            self._records = {}
            return

        try:
            with open(history_path, encoding="utf-8") as f:
                content = f.read()
                if not content:
                    self._records = {}
                    return
                data = json.loads(content)

            self._records = {}
            for record_dict in data:
                try:
                    # Convert status string back to enum
                    if "status" in record_dict:
                        record_dict["status"] = FileStatus(record_dict["status"])
                    record = FileRecord(**record_dict)
                    # Validate record fields
                    if not _validate_record(record):
                        logger.warning(f"Skipping record with invalid field values: {record.path_hash}")
                        continue
                    self._records[record.path_hash] = record
                except (TypeError, ValueError) as e:
                    logger.warning(f"Skipping invalid record: {e}")
                    continue

            logger.info(f"Loaded {len(self._records)} records from {history_path}")

        except json.JSONDecodeError:
            logger.exception(f"Failed to parse history file: {history_path}")
            self._records = {}
        except OSError:
            logger.exception(f"Failed to read history file: {history_path}")
            self._records = {}

    def _save_to_disk(self) -> None:
        """Save records to the JSON history file with atomic write."""
        history_path = get_history_v2_path()
        temp_path = history_path + ".tmp"

        try:
            # Convert records to dictionaries
            records_list = []
            for record in self._records.values():
                record_dict = dataclasses.asdict(record)
                # Convert enum to string for JSON serialization
                record_dict["status"] = record.status.value
                records_list.append(record_dict)

            # Write to temp file
            with open(temp_path, "w", encoding="utf-8") as f:
                json.dump(records_list, f, indent=2)

            # Atomic rename
            os.replace(temp_path, history_path)
            logger.debug(f"Saved {len(self._records)} records to {history_path}")

        except OSError:
            logger.exception(f"Failed to save history file: {history_path}")
            # Clean up temp file if it exists
            with contextlib.suppress(OSError):
                os.remove(temp_path)


# Singleton holder class to avoid global statement
class _IndexHolder:
    """Holds the singleton HistoryIndex instance."""

    instance: Optional[HistoryIndex] = None
    lock: threading.Lock = threading.Lock()


def get_history_index() -> HistoryIndex:
    """Get the singleton HistoryIndex instance.

    The index is created on first call and reused thereafter.
    Thread-safe.

    Returns:
        The singleton HistoryIndex instance.
    """
    with _IndexHolder.lock:
        if _IndexHolder.instance is None:
            _IndexHolder.instance = HistoryIndex()
        return _IndexHolder.instance
