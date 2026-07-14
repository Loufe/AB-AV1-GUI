# src/history_index.py
"""
In-memory index for the conversion history system.

Provides O(1) lookups by path hash with thread-safe access and automatic
cache validation based on file size and modification time.
"""

import contextlib
import dataclasses
import datetime
import json
import logging
import os
import threading
import time

from src.config import DURATION_TOLERANCE_SEC, HISTORY_FILE, MAX_CRF_VALUE, MAX_VMAF_VALUE, RESOLUTION_TOLERANCE_PERCENT
from src.logging_setup import get_script_directory
from src.models import AudioStreamInfo, FileRecord, FileStatus, OperationType, VideoMetadata
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

    Used by duplicate detection to match files across different paths.
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


def create_alias_record(
    source: FileRecord,
    prior_first_seen: str | None,
    file_path: str,
    path_hash: str,
    file_size: int,
    file_mtime: float,
    meta: VideoMetadata,
    anonymize: bool,
) -> FileRecord:
    """Build a duplicate-path alias record: the source's decided result, re-keyed to this path.

    Status and Layer-2/3 results are mirrored from ``source`` (so lookup_file(file_path)
    resolves to the same outcome); identity, cache stamps, and Layer-1 metadata are this
    path's. ``duplicate_of`` keeps it out of the size index and the statistics accessors.

    Args:
        source: The decided record for the same physical file (status/results mirror it).
        prior_first_seen: first_seen of any existing record at this path_hash, else None.
        file_path: The current (duplicate) path.
        path_hash: Pre-computed hash of file_path.
        file_size: Current file size in bytes.
        file_mtime: Current file modification time.
        meta: Metadata from this file's ffprobe.
        anonymize: Whether to store the path or None.
    """
    now = datetime.datetime.now().isoformat(sep=" ", timespec="seconds")
    return dataclasses.replace(
        source,
        # Identity + cache validation - this path / this file on disk
        path_hash=path_hash,
        original_path=file_path if not anonymize else None,
        filename_hash=compute_filename_hash(file_path),
        duplicate_of=source.path_hash,
        file_size_bytes=file_size,
        file_mtime=file_mtime,
        # Layer-1 metadata - this file's fresh ffprobe, source as fallback (refreshes a stale source).
        duration_sec=meta.duration_sec or source.duration_sec,
        video_codec=meta.video_codec or source.video_codec,
        width=meta.width or source.width,
        height=meta.height or source.height,
        bitrate_kbps=meta.bitrate_kbps or source.bitrate_kbps,
        audio_streams=list(meta.audio_streams or source.audio_streams or []),
        # Estimates are meaningless on a decided record; preserve this path's first_seen.
        estimated_reduction_percent=None,
        estimated_from_similar=None,
        first_seen=prior_first_seen or now,
        last_updated=now,
    )


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
        self._size_index: dict[int, set[str]] = {}  # file_size -> {path_hash, ...} for duplicate detection
        self._lock = threading.RLock()
        self._loaded = False
        self._dirty = False
        self._last_save_time = float("-inf")  # monotonic timestamp of the last disk write
        self._converted_cache: list[FileRecord] | None = None
        self._percentiles_cache: dict[OperationType | None, dict] | None = None

    @contextlib.contextmanager
    def transaction(self):
        """Hold the index lock across a multi-step read-decide-write sequence.

        Individual methods are already thread-safe, but a lookup -> find_better_duplicate
        -> upsert sequence spanning several calls is not atomic on its own: two parallel
        scan workers processing two copies of the same physical file can both pass
        find_better_duplicate before either upserts (issue #22). Wrapping the sequence
        in ``with index.transaction():`` serializes it. The lock is reentrant, so index
        methods called inside the block keep working.
        """
        with self._lock:
            self._ensure_loaded()
            yield

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

            # Invalidate caches if this affects converted records
            if record.status == FileStatus.CONVERTED or (old_record and old_record.status == FileStatus.CONVERTED):
                self._converted_cache = None
                self._percentiles_cache = None

            # Maintain size index: re-home this path_hash; _add_to_size_index skips
            # aliases. Removing the OLD record (by its own size) handles an in-place
            # SCANNED->alias replacement at the same size, which the old size-change-only check
            # left as a stale entry.
            if old_record is not None:
                self._remove_from_size_index(old_record)
            self._add_to_size_index(record)

            self._records[record.path_hash] = record
            self._dirty = True

    def _add_to_size_index(self, record: FileRecord) -> None:
        """Register a record in the size index for duplicate detection.

        Alias records (duplicate_of set) are deliberately excluded - they mirror another
        path's file and must never be duplicate-detection candidates (they would corrupt
        find_by_size and the step-3 uniqueness check). They stay resolvable by exact
        path_hash via _records. Caller holds self._lock.
        """
        if record.duplicate_of is None:
            self._size_index.setdefault(record.file_size_bytes, set()).add(record.path_hash)

    def _remove_from_size_index(self, record: FileRecord) -> None:
        """Remove a record's path_hash from its size bucket; no-op if absent. Caller holds self._lock.

        Takes the record whose slot is being vacated (in upsert, the *old* record) so the
        signature mirrors _add_to_size_index and the call sites can't transpose loose args.
        """
        bucket = self._size_index.get(record.file_size_bytes)
        if bucket is not None:
            bucket.discard(record.path_hash)

    def get_by_status(self, status: FileStatus) -> list[FileRecord]:
        """Get all records with a given status.

        Args:
            status: The status to filter by.

        Returns:
            List of FileRecords with the specified status.
        """
        with self._lock:
            self._ensure_loaded()
            # Exclude alias records (duplicate_of set) - they mirror another path's status
            # and must not appear as independent results (e.g. a phantom History-tab row).
            return [r for r in self._records.values() if r.status == status and r.duplicate_of is None]

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
                # Exclude alias records (duplicate_of set) so statistics / estimation do not
                # double-count a single physical conversion reached via multiple paths.
                self._converted_cache = [
                    r for r in self._records.values() if r.status == FileStatus.CONVERTED and r.duplicate_of is None
                ]
            return self._converted_cache

    def get_cached_percentiles(self, operation_type: OperationType | None) -> dict | None:
        """Get cached percentiles for an operation type.

        Args:
            operation_type: ANALYZE, CONVERT, or None for default.

        Returns:
            Cached percentiles dict, or None if not cached.
        """
        with self._lock:
            if self._percentiles_cache is None:
                return None
            return self._percentiles_cache.get(operation_type)

    def cache_percentiles(self, operation_type: OperationType | None, percentiles: dict) -> None:
        """Store computed percentiles in cache.

        Args:
            operation_type: ANALYZE, CONVERT, or None for default.
            percentiles: The computed percentiles dict to cache.
        """
        with self._lock:
            if self._percentiles_cache is None:
                self._percentiles_cache = {}
            self._percentiles_cache[operation_type] = percentiles

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

        O(1) lookup using the size index. Used by duplicate detection.

        Args:
            file_size_bytes: Exact file size to match.

        Returns:
            List of FileRecords with the specified size.
        """
        with self._lock:
            self._ensure_loaded()
            hashes = self._size_index.get(file_size_bytes, set())
            return [self._records[h] for h in hashes if h in self._records]

    def find_better_duplicate(
        self, file_path: str, file_size: int, duration_sec: float | None
    ) -> FileRecord | None:
        """Find an equal-or-higher-status record representing the same physical file.

        Only non-alias records are ever considered: aliases (duplicate_of set) are excluded
        from the size index, so they never enter the candidate pool here. The queried path's
        own record is also excluded - a file is never its own duplicate.

        Implements duplicate detection using a 3-step metadata cascade:
        1. Size + duration + filename literal (when original_path available)
        2. Size + duration + filename_hash (for anonymized records)
        3. Size + duration uniqueness (if globally unique)

        Background/rationale: docs/adr/001-use-metadata-for-duplicate-detection.md

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

        # Pre-filter: candidates sharing this size, excluding this path's own record - a
        # file is never its own duplicate (a decided record re-entering detection after an
        # mtime-only change would otherwise match itself and become a self-alias).
        own_hash = compute_path_hash(file_path)
        candidates = [c for c in self.find_by_size(file_size) if c.path_hash != own_hash]
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

        # Sort by priority descending, then most-recently-updated, then path_hash: a total
        # order, so the winner among equal-priority candidates is determined by the data
        # rather than per-process set iteration order. last_updated is ISO "YYYY-MM-DD
        # HH:MM:SS" everywhere, so lexicographic == chronological; None sorts last.
        candidates.sort(
            key=lambda c: (status_priority.get(c.status, 0), c.last_updated or "", c.path_hash),
            reverse=True,
        )

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
                if c.path_hash != own_hash
                and duration_sec is not None and c.duration_sec is not None
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
            self._remove_from_size_index(record)

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

    def save_if_stale(self, min_interval_sec: float) -> None:
        """Persist changes only if the last disk write is older than min_interval_sec.

        Debounces the per-file saves in the conversion worker: rewriting the whole
        multi-MB history JSON after every processed file makes batch cost grow
        quadratically with history size (issue #22). Callers must still call save()
        at hard checkpoints (queue-item completion, stop, worker exit) so a crash
        loses at most min_interval_sec of records, never a completed item.

        Args:
            min_interval_sec: Minimum seconds between disk writes.
        """
        with self._lock:
            if not self._dirty:
                return
            if time.monotonic() - self._last_save_time < min_interval_sec:
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

            # Rebuild the size index; aliases are excluded by _add_to_size_index.
            self._size_index = {}
            for record in self._records.values():
                self._add_to_size_index(record)

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
            self._last_save_time = time.monotonic()
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
