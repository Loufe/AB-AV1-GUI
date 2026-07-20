# tests/test_history_index.py
"""Tests for src/history_index.py: persistence, duplicate resolution, and locking."""

import json

import pytest
from src.config import HISTORY_SCHEMA_VERSION
from src.folder_analysis import _analyze_file
from src.history_index import HistoryIndex, compute_filename_hash, compute_path_hash
from src.models import FileRecord, FileStatus

DURATION = 120.0
SIZE_BYTES = 11  # len(b"video-bytes")


@pytest.fixture
def history_file(tmp_path, monkeypatch):
    """Point the history file at a per-test temp path (never the real one)."""
    path = tmp_path / "history.json"
    monkeypatch.setattr("src.history_index.get_history_path", lambda: str(path))
    return path


@pytest.fixture
def index(history_file):
    """A fresh, isolated HistoryIndex instance (bypasses the singleton)."""
    return HistoryIndex()


def make_record(file_path: str, *, status: FileStatus = FileStatus.SCANNED, **overrides) -> FileRecord:
    fields = {
        "path_hash": compute_path_hash(file_path),
        "original_path": file_path,
        "status": status,
        "filename_hash": compute_filename_hash(file_path),
        "file_size_bytes": SIZE_BYTES,
        "file_mtime": 1000.0,
        "duration_sec": DURATION,
        "video_codec": "h264",
        "width": 1920,
        "height": 1080,
        "last_updated": "2026-01-01 00:00:00",
    }
    fields.update(overrides)
    return FileRecord(**fields)


def make_ffprobe_info() -> dict:
    """Fake ffprobe output accepted by extract_video_metadata()."""
    return {
        "streams": [
            {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080, "r_frame_rate": "24/1"}
        ],
        "format": {"duration": str(DURATION), "bit_rate": "5000000"},
    }


# ---------------------------------------------------------------------------
# Load/save round-trip
# ---------------------------------------------------------------------------


def test_save_load_roundtrip(history_file, index):
    record = make_record(
        "/videos/movie.mkv",
        status=FileStatus.ANALYZED,
        best_crf=28,
        best_vmaf_achieved=95.5,
        predicted_size_reduction=40.0,
    )
    index.upsert(record)
    index.save()

    assert history_file.exists()

    reloaded = HistoryIndex()
    loaded = reloaded.get(record.path_hash)
    assert loaded == record
    # Size index is rebuilt on load (duplicate detection depends on it)
    assert reloaded.find_by_size(SIZE_BYTES) == [record]


def test_save_load_roundtrip_preserves_fractional_crf(history_file, index):
    # ab-av1 0.11+ finds fractional CRFs (0.25 steps) up to 70 for libsvtav1
    record = make_record(
        "/videos/movie.mkv",
        status=FileStatus.ANALYZED,
        best_crf=68.75,
        best_vmaf_achieved=95.5,
        predicted_size_reduction=40.0,
    )
    index.upsert(record)
    index.save()

    reloaded = HistoryIndex()
    loaded = reloaded.get(record.path_hash)
    assert loaded is not None
    assert loaded.best_crf == 68.75


def test_save_is_noop_when_not_dirty(history_file, index):
    index.get("0" * 16)  # Force load without mutating
    index.save()
    assert not history_file.exists()


def test_save_writes_versioned_container(history_file, index):
    index.upsert(make_record("/videos/movie.mkv"))
    index.save()

    data = json.loads(history_file.read_text(encoding="utf-8"))
    assert data["schema_version"] == HISTORY_SCHEMA_VERSION
    assert len(data["records"]) == 1


def test_load_rejects_legacy_array_with_migration_instructions(history_file):
    history_file.write_text('[{"path_hash": "abc", "status": "scanned"}]', encoding="utf-8")

    with pytest.raises(RuntimeError, match="migrate_history_v2"):
        HistoryIndex().get("abc")


def test_load_rejects_unknown_schema_version(history_file):
    history_file.write_text('{"schema_version": 99, "records": []}', encoding="utf-8")

    with pytest.raises(RuntimeError, match="unsupported schema_version"):
        HistoryIndex().get("abc")


# ---------------------------------------------------------------------------
# delete
# ---------------------------------------------------------------------------


def test_delete_removes_record_and_size_index_entry(index):
    record = make_record("/videos/movie.mkv")
    index.upsert(record)

    assert index.delete(record.path_hash) is True
    assert index.get(record.path_hash) is None
    assert index.find_by_size(SIZE_BYTES) == []
    # Second delete is a no-op
    assert index.delete(record.path_hash) is False


# ---------------------------------------------------------------------------
# Read-time duplicate resolution (ADR-002): nothing persisted for the copy
# ---------------------------------------------------------------------------


def test_scan_of_duplicate_resolves_verdict_without_persisting_it(tmp_path, index, monkeypatch):
    """Scanning a copy of a decided file surfaces the verdict but persists only
    a plain SCANNED record for the copy's own path (ADR-002)."""
    root = tmp_path / "videos"
    (root / "dir1").mkdir(parents=True)
    out = tmp_path / "out"  # deliberately nonexistent: no output-exists short-circuit

    copy_path = str(root / "dir1" / "movie.mkv")
    (root / "dir1" / "movie.mkv").write_bytes(b"video-bytes")

    # Decided source record under another path (same filename, size, duration)
    source = make_record(
        "/videos/original/movie.mkv",
        status=FileStatus.ANALYZED,
        best_crf=28,
        best_vmaf_achieved=95.5,
        predicted_size_reduction=40.0,
    )
    index.upsert(source)

    monkeypatch.setattr("src.folder_analysis.get_video_info", lambda _path: make_ffprobe_info())

    result = _analyze_file(copy_path, root, out, index, anonymize=False)

    # The displayed result carries the source's Layer-2 verdict
    assert result.status == "needs_conversion"
    assert result.estimated_reduction_percent == 40.0
    assert "CRF" in (result.status_detail or "")

    # The copy's own record is a canonical SCANNED record, not a mirrored verdict
    copy_record = index.get(compute_path_hash(copy_path))
    assert copy_record is not None
    assert copy_record.status == FileStatus.SCANNED
    assert copy_record.best_crf is None

    # The source remains the only decided record
    assert index.get_by_status(FileStatus.ANALYZED) == [source]


def test_cache_hit_on_duplicate_path_still_resolves_verdict(tmp_path, index, monkeypatch):
    """A second scan (cache hit on the copy's SCANNED record) resolves the same
    verdict at read time - deleting the source takes effect on the next read."""
    root = tmp_path / "videos"
    (root / "dir1").mkdir(parents=True)
    out = tmp_path / "out"

    copy_path = str(root / "dir1" / "movie.mkv")
    (root / "dir1" / "movie.mkv").write_bytes(b"video-bytes")

    source = make_record(
        "/videos/original/movie.mkv", status=FileStatus.NOT_WORTHWHILE, skip_reason="VMAF target not achievable"
    )
    index.upsert(source)
    monkeypatch.setattr("src.folder_analysis.get_video_info", lambda _path: make_ffprobe_info())

    first = _analyze_file(copy_path, root, out, index, anonymize=False)
    assert first.status == "not_worthwhile"

    # Second scan is a cache hit on the copy's own SCANNED record
    second = _analyze_file(copy_path, root, out, index, anonymize=False)
    assert second.status == "not_worthwhile"

    # Removing the source verdict changes the resolution immediately (no stale alias)
    index.delete(source.path_hash)
    third = _analyze_file(copy_path, root, out, index, anonymize=False)
    assert third.status == "needs_conversion"


# ---------------------------------------------------------------------------
# save_if_stale debouncing (issue #22)
# ---------------------------------------------------------------------------


@pytest.fixture
def clock(monkeypatch):
    """Controllable replacement for time.monotonic inside history_index."""
    now = [1000.0]
    monkeypatch.setattr("src.history_index.time.monotonic", lambda: now[0])
    return now


def records_on_disk(history_file) -> int:
    return len(json.loads(history_file.read_text(encoding="utf-8"))["records"])


def test_save_if_stale_suppresses_saves_within_interval(index, clock, history_file):
    index.upsert(make_record("/videos/a.mkv"))
    index.save_if_stale(30)  # No prior save -> writes immediately
    assert records_on_disk(history_file) == 1

    index.upsert(make_record("/videos/b.mkv"))
    clock[0] += 10
    index.save_if_stale(30)  # Dirty, but only 10s since last write -> suppressed
    assert records_on_disk(history_file) == 1

    clock[0] += 25  # 35s since last write
    index.save_if_stale(30)  # Stale -> writes
    assert records_on_disk(history_file) == 2


def test_forced_flush_saves_regardless_of_interval(index, clock, history_file):
    index.upsert(make_record("/videos/a.mkv"))
    index.save_if_stale(30)
    assert records_on_disk(history_file) == 1

    index.upsert(make_record("/videos/b.mkv"))
    clock[0] += 1
    index.save()  # Mandatory flush ignores the debounce interval
    assert records_on_disk(history_file) == 2


def test_save_if_stale_respects_dirty_flag(index, clock, history_file):
    index.upsert(make_record("/videos/a.mkv"))
    index.save()
    history_file.unlink()

    clock[0] += 100  # Interval elapsed, but nothing changed since the last save
    index.save_if_stale(30)
    assert not history_file.exists()
