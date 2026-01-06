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

from src.config import DURATION_TOLERANCE_SEC, HISTORY_FILE, MAX_CRF_VALUE, MAX_VMAF_VALUE, RESOLUTION_TOLERANCE_PERCENT
from src.logging_setup import get_script_directory
from src.models import AudioStreamInfo, FileRecord, FileStatus
from src.privacy import compute_hash, normalize_path

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
    normalized = normalize_path(file_path)
    return compute_hash(normalized, length=16)


def compute_filename_hash(file_path: str) -> str:
    """Compute hash of filename (basename with extension) for duplicate detection.

    Used by ADR-001 duplicate detection to match files across different paths.
    The hash includes the extension (e.g., "movie.mp4" -> hash).

    Args:
        file_path: Path to the file (only basename is used).

    Returns:
        12-character hex hash of the lowercase filename.
    """
    filename = os.path.basename(file_path).lower()
    return compute_hash(filename, length=12)


def get_history_path() -> str:
    """Get the path to the history file.

    Returns:
        Absolute path to conversion_history.json.
    """
    return os.path.join(get_script_directory(), HISTORY_FILE)


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
        self._size_index: dict[int, list[str]] = {}  # file_size -> [path_hash, ...] (ADR-001)
        self._lock = threading.RLock()
        self._loaded = False
        self._dirty = False
        self._converted_cache: list[FileRecord] | None = None

    def get(self, path_hash: str) -> FileRecord | None:
        """Get a record by its path hash.

        Args:
            path_hash: The 16-character hash of the normalized path.

        Returns:
            The FileRecord if found, None otherwise.
        """
        with self._lock:
            self._ensure_loaded()
            return self._records.get(path_hash)

    def lookup_file(self, file_path: str) -> FileRecord | None:
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
            old_record = self._records.get(record.path_hash)

            # Invalidate converted cache if this affects converted records
            if record.status == FileStatus.CONVERTED or (old_record and old_record.status == FileStatus.CONVERTED):
                self._converted_cache = None

            # Maintain size index (ADR-001)
            if old_record and old_record.file_size_bytes != record.file_size_bytes:
                # Remove from old size bucket
                old_bucket = self._size_index.get(old_record.file_size_bytes, [])
                if record.path_hash in old_bucket:
                    old_bucket.remove(record.path_hash)

            # Add to new size bucket (if not already there)
            if record.file_size_bytes not in self._size_index:
                self._size_index[record.file_size_bytes] = []
            if record.path_hash not in self._size_index[record.file_size_bytes]:
                self._size_index[record.file_size_bytes].append(record.path_hash)

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

    def find_by_size(self, file_size_bytes: int) -> list[FileRecord]:
        """Find all records with a given file size.

        O(1) lookup using the size index. Used by ADR-001 duplicate detection.

        Args:
            file_size_bytes: Exact file size to match.

        Returns:
            List of FileRecords with the specified size.
        """
        with self._lock:
            self._ensure_loaded()
            hashes = self._size_index.get(file_size_bytes, [])
            return [self._records[h] for h in hashes if h in self._records]

    def find_better_duplicate(
        self, file_path: str, file_size: int, duration_sec: float | None
    ) -> FileRecord | None:
        """Find a higher-status record representing the same physical file.

        Implements ADR-001 duplicate detection using a 3-step metadata cascade:
        1. Size + duration + filename literal (when original_path available)
        2. Size + duration + filename_hash (for anonymized records)
        3. Size + duration uniqueness (if globally unique)

        Args:
            file_path: Path to the file being checked.
            file_size: File size in bytes.
            duration_sec: Video duration (for matching).

        Returns:
            A FileRecord with higher status priority if duplicate found, else None.
        """
        # Status priority: higher number = better status
        status_priority = {
            FileStatus.SCANNED: 1,
            FileStatus.ANALYZED: 2,
            FileStatus.NOT_WORTHWHILE: 3,
            FileStatus.CONVERTED: 4,
        }

        # Pre-filter: find candidates by size
        candidates = self.find_by_size(file_size)
        if not candidates:
            return None

        # Filter by duration (within tolerance)
        if duration_sec is not None:
            candidates = [
                c for c in candidates
                if c.duration_sec is not None
                and abs(c.duration_sec - duration_sec) <= DURATION_TOLERANCE_SEC
            ]

        if not candidates:
            return None

        # Sort by priority descending (check highest priority first)
        candidates.sort(key=lambda c: status_priority.get(c.status, 0), reverse=True)

        # Compute filename info for matching
        filename_lower = os.path.basename(file_path).lower()
        file_hash = compute_filename_hash(file_path)

        best_record: FileRecord | None = None
        best_priority = 0
        uncertain_candidates: list[FileRecord] = []

        for candidate in candidates:
            priority = status_priority.get(candidate.status, 0)
            if priority <= best_priority:
                continue  # Already found better or equal

            # Step 1: Filename literal match (requires original_path)
            if candidate.original_path:
                candidate_filename = os.path.basename(candidate.original_path).lower()
                if filename_lower == candidate_filename:
                    best_record = candidate
                    best_priority = priority
                    continue

            # Step 2: Filename hash match (for anonymized records)
            if candidate.filename_hash and file_hash == candidate.filename_hash:
                best_record = candidate
                best_priority = priority
                continue

            # Couldn't confirm via filename - mark as uncertain for step 3
            uncertain_candidates.append(candidate)

        # If we found a match via steps 1-2, return it
        if best_record:
            return best_record

        # Step 3: Uniqueness fallback
        # If exactly one uncertain candidate AND size+duration is globally unique
        if len(uncertain_candidates) == 1:
            # Check if this size+duration combo is unique in the entire index
            all_with_size = self.find_by_size(file_size)
            matching_duration = [
                c for c in all_with_size
                if duration_sec is not None and c.duration_sec is not None
                and abs(c.duration_sec - duration_sec) <= DURATION_TOLERANCE_SEC
            ]
            if len(matching_duration) == 1:
                return uncertain_candidates[0]

        return None

    def delete(self, path_hash: str) -> bool:
        """Delete a record by path_hash.

        Removes the record from the main index and size index.
        Changes are not persisted until save() is called.

        Args:
            path_hash: The hash of the record to delete.

        Returns:
            True if the record was deleted, False if not found.
        """
        with self._lock:
            self._ensure_loaded()
            if path_hash not in self._records:
                return False

            record = self._records[path_hash]

            # Remove from size index
            bucket = self._size_index.get(record.file_size_bytes, [])
            if path_hash in bucket:
                bucket.remove(path_hash)

            # Invalidate converted cache if needed
            if record.status == FileStatus.CONVERTED:
                self._converted_cache = None

            # Remove from records
            del self._records[path_hash]
            self._dirty = True
            return True

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
        history_path = get_history_path()

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
                    # Detect old format that needs migration
                    if "audio_codec" in record_dict and "audio_streams" not in record_dict:
                        raise RuntimeError(
                            "History file uses old format (audio_codec field).\n"
                            "Please run migration before starting the app:\n"
                            "  python tools/migrate_audio_streams.py\n"
                            "See docs/PLAN_AUDIO_STREAMS_AND_BITRATE.md for details."
                        )

                    # Convert status string back to enum
                    if "status" in record_dict:
                        record_dict["status"] = FileStatus(record_dict["status"])

                    # Convert audio_streams dicts to AudioStreamInfo objects
                    audio_streams_data = record_dict.get("audio_streams")
                    if audio_streams_data:
                        record_dict["audio_streams"] = [
                            AudioStreamInfo.from_dict(s) for s in audio_streams_data
                        ]

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

            # Build size index for ADR-001 duplicate detection
            self._size_index = {}
            for path_hash, record in self._records.items():
                if record.file_size_bytes not in self._size_index:
                    self._size_index[record.file_size_bytes] = []
                self._size_index[record.file_size_bytes].append(path_hash)

        except json.JSONDecodeError:
            logger.exception(f"Failed to parse history file: {history_path}")
            self._records = {}
        except OSError:
            logger.exception(f"Failed to read history file: {history_path}")
            self._records = {}

    def _save_to_disk(self) -> None:
        """Save records to the JSON history file with atomic write."""
        history_path = get_history_path()
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

    instance: HistoryIndex | None = None
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
