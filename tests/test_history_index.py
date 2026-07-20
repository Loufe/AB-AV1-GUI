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
# No duplicate detection (ADR-001): every path is canonical
# ---------------------------------------------------------------------------


def test_scan_of_content_copy_creates_independent_record(tmp_path, index, monkeypatch):
    """A content copy of a decided file gets its own SCANNED record - no alias,
    no verdict mirroring (ADR-001; content identity waits on the #28 tier)."""
    root = tmp_path / "videos"
    (root / "dir1").mkdir(parents=True)
    out = tmp_path / "out"  # deliberately nonexistent: no output-exists short-circuit

    copy_path = str(root / "dir1" / "movie.mkv")
    (root / "dir1" / "movie.mkv").write_bytes(b"video-bytes")

    # Decided record under another path (same filename, size, duration)
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

    # The copy is analyzed on its own merits: fresh SCANNED record, no mirrored verdict
    assert result.status == "needs_conversion"
    copy_record = index.get(compute_path_hash(copy_path))
    assert copy_record is not None
    assert copy_record.status == FileStatus.SCANNED
    assert copy_record.best_crf is None
    assert index.get_by_status(FileStatus.ANALYZED) == [source]


# ---------------------------------------------------------------------------
# converted_revision: staleness signal for the estimation percentile cache
# ---------------------------------------------------------------------------


def test_converted_revision_bumps_only_on_converted_changes(index):
    revision = index.converted_revision

    index.upsert(make_record("/videos/a.mkv"))  # SCANNED: no bump
    assert index.converted_revision == revision

    index.upsert(make_record("/videos/b.mkv", status=FileStatus.CONVERTED))
    assert index.converted_revision == revision + 1

    # Overwriting a CONVERTED record bumps again (its data may have changed)
    index.upsert(make_record("/videos/b.mkv", status=FileStatus.CONVERTED))
    assert index.converted_revision == revision + 2


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
