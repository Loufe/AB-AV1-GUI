# tests/test_history_index.py
"""Tests for src/history_index.py: persistence, aliases, and locking."""

import json
import threading
import time

import pytest
from src.folder_analysis import _analyze_file
from src.history_index import HistoryIndex, compute_filename_hash, compute_path_hash, create_alias_record
from src.models import FileRecord, FileStatus, VideoMetadata

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


# ---------------------------------------------------------------------------
# create_alias_record
# ---------------------------------------------------------------------------


def test_create_alias_record_mirrors_verdict_and_rekeys_identity(index):
    source = make_record(
        "/videos/movie.mkv",
        status=FileStatus.ANALYZED,
        best_crf=28,
        best_vmaf_achieved=95.5,
        predicted_size_reduction=40.0,
        estimated_reduction_percent=33.0,
        estimated_from_similar=4,
    )
    alias_path = "/copies/movie.mkv"
    alias = create_alias_record(
        source,
        prior_first_seen="2025-06-01 12:00:00",
        file_path=alias_path,
        path_hash=compute_path_hash(alias_path),
        file_size=SIZE_BYTES,
        file_mtime=2000.0,
        meta=VideoMetadata(duration_sec=DURATION, video_codec="h264", width=1920, height=1080),
        anonymize=False,
    )

    # Identity is this path's
    assert alias.path_hash == compute_path_hash(alias_path)
    assert alias.original_path == alias_path
    assert alias.file_mtime == 2000.0
    assert alias.first_seen == "2025-06-01 12:00:00"
    # Verdict mirrors the source
    assert alias.duplicate_of == source.path_hash
    assert alias.status == FileStatus.ANALYZED
    assert alias.best_crf == 28
    # Estimates are cleared on a decided record
    assert alias.estimated_reduction_percent is None
    assert alias.estimated_from_similar is None

    # Aliases never enter the size index (must not become duplicate candidates)
    index.upsert(source)
    index.upsert(alias)
    assert index.find_by_size(SIZE_BYTES) == [source]


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
# Concurrency: duplicate resolution must be atomic (issue #22)
# ---------------------------------------------------------------------------


def test_concurrent_scan_of_duplicates_yields_one_canonical(tmp_path, index, monkeypatch):
    """Two parallel workers resolving duplicates must serialize on the index.

    Setup: a decided ANALYZED record (source) plus a stale SCANNED record at
    path1 sharing its size/duration. path1 (same filename as the source) and
    path2 (renamed copy) are scanned concurrently.

    Serialized via index.transaction(): worker A aliases path1 to the source,
    which retires the stale SCANNED record from the size index; worker B then
    sees the source as the unique size+duration match (ADR-001 step 3) and
    aliases too - one canonical, two aliases.

    Unfixed race: B runs find_better_duplicate before A upserts, still sees the
    stale SCANNED record, the step-3 uniqueness check fails, and B writes a
    second canonical record for the same physical file.
    """
    root = tmp_path / "videos"
    (root / "dir1").mkdir(parents=True)
    (root / "dir2").mkdir(parents=True)
    out = tmp_path / "out"  # deliberately nonexistent: no output-exists short-circuit

    path1 = str(root / "dir1" / "movie.mkv")
    path2 = str(root / "dir2" / "renamed.mkv")
    (root / "dir1" / "movie.mkv").write_bytes(b"video-bytes")
    (root / "dir2" / "renamed.mkv").write_bytes(b"video-bytes")

    # Decided source record under a third path (file itself no longer around)
    source = make_record(
        "/videos/original/movie.mkv",
        status=FileStatus.ANALYZED,
        best_crf=28,
        best_vmaf_achieved=95.5,
        predicted_size_reduction=40.0,
    )
    index.upsert(source)
    # Stale SCANNED record for path1 (content unchanged, mtime stamp outdated)
    index.upsert(make_record(path1, file_mtime=1.0))

    monkeypatch.setattr("src.folder_analysis.get_video_info", lambda _path: make_ffprobe_info())

    # Widen the race window: pause after find_better_duplicate computes its
    # result, before the caller upserts. With the fix the pause happens inside
    # the transaction, so the second worker cannot interleave.
    in_find = threading.Event()
    original_find = HistoryIndex.find_better_duplicate

    def slow_find(self, *args, **kwargs):
        result = original_find(self, *args, **kwargs)
        in_find.set()
        time.sleep(0.3)
        return result

    monkeypatch.setattr(HistoryIndex, "find_better_duplicate", slow_find)

    errors: list[Exception] = []

    def analyze(path: str) -> None:
        try:
            _analyze_file(path, root, out, index, anonymize=False)
        except Exception as e:  # pragma: no cover - failure reporting only
            errors.append(e)

    worker_a = threading.Thread(target=analyze, args=(path1,))
    worker_b = threading.Thread(target=analyze, args=(path2,))
    worker_a.start()
    assert in_find.wait(timeout=5.0), "worker A never reached find_better_duplicate"
    worker_b.start()
    worker_a.join(timeout=10.0)
    worker_b.join(timeout=10.0)

    assert not errors

    record1 = index.get(compute_path_hash(path1))
    record2 = index.get(compute_path_hash(path2))
    assert record1 is not None and record1.duplicate_of == source.path_hash
    assert record2 is not None and record2.duplicate_of == source.path_hash

    canonical = [r for r in index.get_all_records() if r.duplicate_of is None]
    assert canonical == [source]


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
    return len(json.loads(history_file.read_text(encoding="utf-8")))


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
