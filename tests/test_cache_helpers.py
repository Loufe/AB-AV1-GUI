# tests/test_cache_helpers.py
"""Characterization tests for src/cache_helpers.py cache validation logic."""

import os

from src.cache_helpers import converted_verdict_applies, is_file_unchanged
from src.history_index import compute_path_hash
from src.models import FileRecord, FileStatus


def make_record(file_path: str, *, status: FileStatus = FileStatus.CONVERTED, **overrides) -> FileRecord:
    """Build a FileRecord whose stamps match the file currently on disk."""
    stat_info = os.stat(file_path)
    fields = {
        "path_hash": compute_path_hash(file_path),
        "original_path": file_path,
        "status": status,
        "file_size_bytes": stat_info.st_size,
        "file_mtime": stat_info.st_mtime,
    }
    fields.update(overrides)
    return FileRecord(**fields)


def write_file(path, content: bytes = b"video-bytes") -> str:
    path.write_bytes(content)
    return str(path)


# ---------------------------------------------------------------------------
# is_file_unchanged
# ---------------------------------------------------------------------------


def test_unchanged_file_matches(tmp_path):
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path)

    assert is_file_unchanged(record, file_path) is True


def test_mtime_within_tolerance_still_matches(tmp_path):
    # JSON round-trips lose float precision; anything under 1s counts as equal.
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path)
    record.file_mtime = os.stat(file_path).st_mtime + 0.5

    assert is_file_unchanged(record, file_path) is True


def test_mtime_beyond_tolerance_does_not_match(tmp_path):
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path)
    record.file_mtime = os.stat(file_path).st_mtime + 1.5

    assert is_file_unchanged(record, file_path) is False


def test_size_mismatch_does_not_match(tmp_path):
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path, file_size_bytes=os.stat(file_path).st_size + 1)

    assert is_file_unchanged(record, file_path) is False


def test_path_hash_mismatch_does_not_match(tmp_path):
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path, path_hash=compute_path_hash("/somewhere/else.mp4"))

    assert is_file_unchanged(record, file_path) is False


def test_missing_file_does_not_match(tmp_path):
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path)
    missing = str(tmp_path / "gone.mp4")
    record.path_hash = compute_path_hash(missing)

    assert is_file_unchanged(record, missing) is False


# ---------------------------------------------------------------------------
# converted_verdict_applies
# ---------------------------------------------------------------------------


def test_verdict_applies_when_file_unchanged(tmp_path):
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path, output_size_bytes=5)

    assert converted_verdict_applies(record, file_path) is True


def test_legacy_record_without_output_size_keeps_verdict(tmp_path):
    # Legacy path: no Layer-3 output size means we can't tell our own output
    # apart from genuinely changed content, so the verdict is kept.
    file_path = write_file(tmp_path / "movie.mkv")
    record = make_record(file_path, output_size_bytes=None)
    (tmp_path / "movie.mkv").write_bytes(b"completely different content!!")

    assert converted_verdict_applies(record, file_path) is True


def test_changed_non_mkv_file_invalidates_verdict(tmp_path):
    file_path = write_file(tmp_path / "movie.mp4")
    record = make_record(file_path, output_size_bytes=999)
    (tmp_path / "movie.mp4").write_bytes(b"new content of a different size")

    assert converted_verdict_applies(record, file_path) is False


def test_replace_mode_output_recognized_by_size(tmp_path):
    # Replace-mode steady state: the .mkv at the input path IS our output.
    # Stamps differ from the recorded input, but the size equals the recorded
    # output size, so the verdict still applies.
    output_content = b"x" * 100
    file_path = write_file(tmp_path / "movie.mkv", b"y" * 50)
    record = make_record(file_path, output_size_bytes=100)
    (tmp_path / "movie.mkv").write_bytes(output_content)

    assert converted_verdict_applies(record, file_path) is True


def test_changed_mkv_with_size_mismatch_invalidates_verdict(tmp_path):
    file_path = write_file(tmp_path / "movie.mkv", b"y" * 50)
    record = make_record(file_path, output_size_bytes=100)
    (tmp_path / "movie.mkv").write_bytes(b"z" * 77)

    assert converted_verdict_applies(record, file_path) is False


def test_missing_mkv_keeps_verdict_conservatively(tmp_path):
    # getsize() fails on a vanished file: current behavior keeps the verdict
    # rather than re-queueing.
    file_path = write_file(tmp_path / "movie.mkv")
    record = make_record(file_path, output_size_bytes=100)
    missing = str(tmp_path / "gone.mkv")
    record.path_hash = compute_path_hash(missing)

    assert converted_verdict_applies(record, missing) is True
