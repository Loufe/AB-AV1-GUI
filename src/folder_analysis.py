# src/folder_analysis.py
"""
Folder analysis for the Analysis tab.

Provides Layer 1 (metadata-only) scanning that:
- Recursively scans folders for video files
- Checks the history index for cached data
- Runs ffprobe only for uncached/invalid entries
- Estimates reduction based on similar files in history
- Estimates conversion time based on historical data
- Returns structured results for the UI

This module does NOT perform VMAF analysis (Layer 2).
"""

import dataclasses
import datetime
import logging
import os
from dataclasses import dataclass, field
from pathlib import Path
from statistics import mean
from typing import Generator

from src.cache_helpers import mtimes_match
from src.config import DEFAULT_REDUCTION_ESTIMATE_PERCENT
from src.history_index import HistoryIndex, compute_path_hash
from src.models import FileRecord, FileStatus
from src.utils import get_video_info
from src.video_metadata import extract_video_metadata

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class FileAnalysisResult:
    """Result of analyzing a single file."""

    path: str
    path_hash: str
    status: str  # "needs_conversion", "already_done", "not_worthwhile", "skipped_*"
    file_size_bytes: int
    video_codec: str | None
    resolution: str | None  # "1920x1080"
    duration_sec: float | None
    estimated_reduction_percent: float | None
    estimated_savings_bytes: int | None
    status_detail: str | None = None  # Additional detail about status


@dataclass
class FolderAnalysisResult:
    """Aggregated analysis result for a folder."""

    folder_path: str
    relative_path: str  # Path relative to scan root
    total_files: int = 0
    convertible_count: int = 0
    already_done_count: int = 0
    not_worthwhile_count: int = 0
    skipped_count: int = 0
    total_size_bytes: int = 0
    estimated_savings_bytes: int = 0
    estimated_time_seconds: float = 0.0
    files: list[FileAnalysisResult] = field(default_factory=list)


@dataclass
class AnalysisSummary:
    """Summary of entire analysis scan."""

    root_folder: str
    total_folders: int = 0
    total_files: int = 0
    convertible_count: int = 0
    already_done_count: int = 0
    not_worthwhile_count: int = 0
    skipped_count: int = 0
    total_size_bytes: int = 0
    estimated_savings_bytes: int = 0
    estimated_time_seconds: float = 0.0
    folders: list[FolderAnalysisResult] = field(default_factory=list)


def scan_folder_fast(root_folder: str, extensions: list[str]) -> AnalysisSummary:
    """Fast filesystem scan - no ffprobe, no metadata.

    Returns folder/file structure immediately:
    - Scans for files matching extensions
    - Gets file size via os.stat (fast)
    - All metadata fields set to None
    - Status set to "pending" (not analyzed yet)

    Should complete in milliseconds for typical folders.

    Args:
        root_folder: Root folder to scan.
        extensions: List of video extensions to look for (e.g., ["mp4", "mkv"]).

    Returns:
        AnalysisSummary with folder/file structure (pending analysis).
    """
    root_path = Path(root_folder).resolve()

    # Find all video files
    all_files = list(_find_video_files(root_folder, extensions))

    if not all_files:
        return AnalysisSummary(root_folder=root_folder)

    # Group by folder
    folder_results: dict[str, FolderAnalysisResult] = {}

    for file_path in all_files:
        # Get file size (fast)
        try:
            stat = os.stat(file_path)
            file_size = stat.st_size
        except OSError:
            file_size = 0

        path_hash = compute_path_hash(file_path)

        # Create pending result
        result = FileAnalysisResult(
            path=file_path,
            path_hash=path_hash,
            status="pending",
            file_size_bytes=file_size,
            video_codec=None,
            resolution=None,
            duration_sec=None,
            estimated_reduction_percent=None,
            estimated_savings_bytes=None,
            status_detail="Not analyzed yet",
        )

        # Group by folder
        folder_path = os.path.dirname(file_path)
        if folder_path not in folder_results:
            try:
                relative = Path(folder_path).relative_to(root_path)
                relative_str = str(relative) if str(relative) != "." else "(root)"
            except ValueError:
                relative_str = folder_path
            folder_results[folder_path] = FolderAnalysisResult(folder_path=folder_path, relative_path=relative_str)

        fr = folder_results[folder_path]
        fr.total_files += 1
        fr.files.append(result)
        fr.total_size_bytes += file_size

    # Build summary - sort folders and files alphabetically (case-insensitive)
    folders = sorted(folder_results.values(), key=lambda f: f.relative_path.lower())
    for folder in folders:
        folder.files.sort(key=lambda f: os.path.basename(f.path).lower())

    return AnalysisSummary(
        root_folder=root_folder,
        total_folders=len(folders),
        total_files=sum(f.total_files for f in folders),
        folders=folders,
    )


def _find_video_files(root: str, extensions: list[str]) -> Generator[str, None, None]:
    """Find all video files in a directory tree.

    Uses single-pass os.walk() instead of multiple rglob() calls for performance.
    A directory tree is traversed once, and files are filtered by extension.

    Args:
        root: Root directory to search.
        extensions: List of extensions to match (without dots).

    Yields:
        Absolute paths to video files.
    """
    # Build set of lowercase extensions for fast lookup
    ext_set = {ext.lower() for ext in extensions}

    # Single-pass traversal (much faster than multiple rglob calls)
    for dirpath, _dirnames, filenames in os.walk(root):
        for filename in filenames:
            # Check extension (case-insensitive)
            ext = filename.rsplit(".", 1)[-1].lower() if "." in filename else ""
            if ext in ext_set:
                yield os.path.join(dirpath, filename)


def _analyze_file(
    file_path: str, root_path: Path, output_path: Path, index: HistoryIndex, anonymize: bool
) -> FileAnalysisResult:
    """Analyze a single file, using cache where possible.

    Args:
        file_path: Path to the video file.
        root_path: Root folder of the scan.
        output_path: Output folder for conversions.
        index: The history index for cache lookups.
        anonymize: Whether to anonymize paths.

    Returns:
        FileAnalysisResult with analysis data.
    """
    path_hash = compute_path_hash(file_path)
    filename = os.path.basename(file_path)

    # Get file stats
    try:
        stat = os.stat(file_path)
        file_size = stat.st_size
        file_mtime = stat.st_mtime
    except OSError as e:
        logger.warning(f"Cannot stat file {filename}: {e}")
        return FileAnalysisResult(
            path=file_path,
            path_hash=path_hash,
            status="skipped_error",
            file_size_bytes=0,
            video_codec=None,
            resolution=None,
            duration_sec=None,
            estimated_reduction_percent=None,
            estimated_savings_bytes=None,
            status_detail=f"Cannot access file: {e}",
        )

    # Check if output already exists
    output_file = _get_output_path(file_path, root_path, output_path)
    if output_file.exists():
        return FileAnalysisResult(
            path=file_path,
            path_hash=path_hash,
            status="already_done",
            file_size_bytes=file_size,
            video_codec=None,
            resolution=None,
            duration_sec=None,
            estimated_reduction_percent=None,
            estimated_savings_bytes=None,
            status_detail="Output file exists",
        )

    # Check cache
    cached = index.get(path_hash)
    if cached and cached.file_size_bytes == file_size and mtimes_match(cached.file_mtime, file_mtime):
        # Cache hit - use cached data
        return _record_to_result(file_path, cached, index)

    # Cache miss or stale - run ffprobe
    video_info = get_video_info(file_path)

    # Check if we have an existing record with ANALYZED, CONVERTED, or NOT_WORTHWHILE status
    # that should be preserved (only update metadata, not overwrite analysis/conversion data)
    if cached and cached.status in (FileStatus.CONVERTED, FileStatus.NOT_WORTHWHILE, FileStatus.ANALYZED):
        # Update metadata while preserving conversion data
        record = _update_existing_record_metadata(cached, file_size, file_mtime, video_info, anonymize, file_path)
        # Save updated record and return result based on existing status
        index.upsert(record)
        return _record_to_result(file_path, record, index)

    # New file or previously SCANNED - create new SCANNED record
    record = _create_scanned_record(file_path, path_hash, file_size, file_mtime, video_info, anonymize)

    # Check for skip conditions
    skip_status, skip_detail = _check_skip_conditions(record, video_info)
    if skip_status:
        record.skip_reason = skip_detail
        index.upsert(record)
        return FileAnalysisResult(
            path=file_path,
            path_hash=path_hash,
            status=skip_status,
            file_size_bytes=file_size,
            video_codec=record.video_codec,
            resolution=f"{record.width}x{record.height}" if record.width and record.height else None,
            duration_sec=record.duration_sec,
            estimated_reduction_percent=None,
            estimated_savings_bytes=None,
            status_detail=skip_detail,
        )

    # Estimate reduction based on similar files
    est_reduction, similar_count = _estimate_reduction(record, index)
    record.estimated_reduction_percent = est_reduction
    record.estimated_from_similar = similar_count

    # Save to index
    index.upsert(record)

    # Calculate estimated savings (accounting for audio which is copied unchanged)
    est_savings = None
    if est_reduction and file_size:
        # Estimate audio size - not reduced, copied unchanged
        meta = extract_video_metadata(video_info)
        audio_size = 0
        if meta.duration_sec and meta.total_audio_bitrate_kbps:
            audio_size = int(meta.duration_sec * meta.total_audio_bitrate_kbps * 1000 / 8)

        # Apply reduction only to video portion
        video_size = max(0, file_size - audio_size)
        est_savings = int(video_size * est_reduction / 100)

    return FileAnalysisResult(
        path=file_path,
        path_hash=path_hash,
        status="needs_conversion",
        file_size_bytes=file_size,
        video_codec=record.video_codec,
        resolution=f"{record.width}x{record.height}" if record.width and record.height else None,
        duration_sec=record.duration_sec,
        estimated_reduction_percent=est_reduction,
        estimated_savings_bytes=est_savings,
        status_detail=f"Est. based on {similar_count} similar files" if similar_count else "Est. (no similar files)",
    )


def _get_output_path(input_path: str, root_path: Path, output_path: Path) -> Path:
    """Calculate the output path for a given input file.

    Preserves directory structure relative to root.

    Args:
        input_path: Path to input file.
        root_path: Root folder of the scan.
        output_path: Output folder for conversions.

    Returns:
        Path object for the expected output file.
    """
    input_file = Path(input_path)
    try:
        relative = input_file.parent.relative_to(root_path)
    except ValueError:
        relative = Path()

    output_dir = output_path / relative
    return output_dir / (input_file.stem + ".mkv")


def _create_scanned_record(
    file_path: str, path_hash: str, file_size: int, file_mtime: float, video_info: dict | None, anonymize: bool
) -> FileRecord:
    """Create a SCANNED status record from ffprobe output.

    Args:
        file_path: Path to the file.
        path_hash: Pre-computed path hash.
        file_size: File size in bytes.
        file_mtime: File modification time.
        video_info: Output from get_video_info(), or None if failed.
        anonymize: Whether to anonymize the path.

    Returns:
        FileRecord with SCANNED status.
    """
    now = datetime.datetime.now().isoformat(sep=" ", timespec="seconds")

    meta = extract_video_metadata(video_info)

    return FileRecord(
        path_hash=path_hash,
        original_path=file_path if not anonymize else None,
        status=FileStatus.SCANNED,
        file_size_bytes=file_size,
        file_mtime=file_mtime,
        duration_sec=meta.duration_sec,
        video_codec=meta.video_codec,
        audio_codec=meta.audio_codec,
        width=meta.width,
        height=meta.height,
        bitrate_kbps=meta.bitrate_kbps,
        first_seen=now,
        last_updated=now,
    )


def _update_existing_record_metadata(
    existing: FileRecord, file_size: int, file_mtime: float, video_info: dict | None, anonymize: bool, file_path: str
) -> FileRecord:
    """Update metadata fields on an existing record while preserving status and conversion data.

    This is used when we have a CONVERTED or NOT_WORTHWHILE record with stale metadata
    (e.g., file_mtime=0 from migration). We update the metadata from ffprobe but keep
    all the conversion-related fields intact.

    Args:
        existing: The existing FileRecord to update.
        file_size: Current file size in bytes.
        file_mtime: Current file modification time.
        video_info: Output from get_video_info(), or None if failed.
        anonymize: Whether to anonymize paths.
        file_path: Path to the file.

    Returns:
        Updated FileRecord with refreshed metadata.
    """
    now = datetime.datetime.now().isoformat(sep=" ", timespec="seconds")

    meta = extract_video_metadata(video_info)

    # Update metadata fields while preserving status and conversion data
    return dataclasses.replace(
        existing,
        file_size_bytes=file_size,
        file_mtime=file_mtime,
        duration_sec=meta.duration_sec if meta.duration_sec else existing.duration_sec,
        video_codec=meta.video_codec if meta.video_codec else existing.video_codec,
        audio_codec=meta.audio_codec if meta.audio_codec else existing.audio_codec,
        width=meta.width if meta.width else existing.width,
        height=meta.height if meta.height else existing.height,
        bitrate_kbps=meta.bitrate_kbps if meta.bitrate_kbps else existing.bitrate_kbps,
        original_path=existing.original_path or (file_path if not anonymize else None),
        last_updated=now,
        # Preserve first_seen if it exists
        first_seen=existing.first_seen or now,
    )


def _check_skip_conditions(record: FileRecord, video_info: dict | None) -> tuple[str | None, str | None]:
    """Check if a file should be skipped for conversion.

    Args:
        record: The FileRecord with video metadata.
        video_info: Raw ffprobe output.

    Returns:
        Tuple of (status, detail) if should skip, (None, None) otherwise.
    """
    # No video info - can't analyze
    if not video_info:
        return "skipped_error", "Cannot read video metadata"

    # No video stream (use metadata extraction for consistency)
    meta = extract_video_metadata(video_info)
    if not meta.has_video:
        return "skipped_no_video", "No video stream found"

    # Already AV1 in MKV
    if record.video_codec and record.video_codec.lower() == "av1":
        return "skipped_already_av1", "Already AV1 codec"

    return None, None


def _record_to_result(file_path: str, record: FileRecord, index: HistoryIndex) -> FileAnalysisResult:
    """Convert a cached FileRecord to an analysis result.

    Args:
        file_path: Original file path (for display).
        record: Cached FileRecord.
        index: History index for estimation updates.

    Returns:
        FileAnalysisResult based on cached data.
    """
    # Determine status
    if record.status == FileStatus.CONVERTED:
        status = "already_done"
        detail = "Previously converted"
    elif record.status == FileStatus.NOT_WORTHWHILE:
        status = "not_worthwhile"
        detail = record.skip_reason or f"VMAF analysis failed (tried down to {record.min_vmaf_attempted})"
    elif record.status == FileStatus.ANALYZED:
        # ANALYZED files need conversion but have accurate predictions (Layer 2 complete)
        status = "needs_conversion"
        detail = f"CRF {record.best_crf} → VMAF {record.best_vmaf_achieved:.1f}" if record.best_crf else None
    elif record.skip_reason:
        status = "skipped_other"
        detail = record.skip_reason
    else:
        status = "needs_conversion"
        detail = None

    resolution = f"{record.width}x{record.height}" if record.width and record.height else None

    # Get reduction estimate/prediction based on status
    reduction = None
    savings = None
    if status == "needs_conversion":
        if record.status == FileStatus.ANALYZED:
            # ANALYZED: Use accurate prediction from CRF search (no estimation needed)
            reduction = record.predicted_size_reduction
            if reduction and record.file_size_bytes:
                savings = int(record.file_size_bytes * reduction / 100)
            # detail already set above with CRF/VMAF info
        else:
            # SCANNED: Use or calculate estimate based on similar files
            reduction = record.estimated_reduction_percent
            if reduction is None:
                reduction, similar_count = _estimate_reduction(record, index)
                record.estimated_reduction_percent = reduction
                record.estimated_from_similar = similar_count
                index.upsert(record)
            if reduction and record.file_size_bytes:
                savings = int(record.file_size_bytes * reduction / 100)
            detail = f"Est. based on {record.estimated_from_similar or 0} similar files"

    return FileAnalysisResult(
        path=file_path,
        path_hash=record.path_hash,
        status=status,
        file_size_bytes=record.file_size_bytes,
        video_codec=record.video_codec,
        resolution=resolution,
        duration_sec=record.duration_sec,
        estimated_reduction_percent=reduction if status == "needs_conversion" else record.reduction_percent,
        estimated_savings_bytes=savings,
        status_detail=detail,
    )


def _estimate_reduction(record: FileRecord, index: HistoryIndex) -> tuple[float | None, int]:
    """Estimate reduction percentage based on similar converted files.

    Args:
        record: The FileRecord to estimate for.
        index: History index to find similar files.

    Returns:
        Tuple of (estimated_reduction_percent, similar_files_count).
    """
    if not record.video_codec or not record.width:
        # Fall back to global average
        return _global_average_reduction(index), 0

    # Find similar files
    similar = index.find_similar(record.video_codec, record.width)

    if similar:
        reductions = [r.reduction_percent for r in similar if r.reduction_percent is not None]
        if reductions:
            return mean(reductions), len(reductions)

    # Fall back to global average
    return _global_average_reduction(index), 0


def _global_average_reduction(index: HistoryIndex) -> float:
    """Get the global average reduction from all converted files.

    Args:
        index: History index.

    Returns:
        Average reduction percent, or DEFAULT_REDUCTION_ESTIMATE_PERCENT as default.
    """
    converted = index.get_converted_records()
    if converted:
        reductions = [r.reduction_percent for r in converted if r.reduction_percent is not None]
        if reductions:
            return mean(reductions)
    return DEFAULT_REDUCTION_ESTIMATE_PERCENT
