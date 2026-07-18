# tests/test_tree_display.py
"""Tests for queue file status formatting and skip reason persistence."""

from src.gui.tree_display import format_queue_file_status
from src.models import QueueFileItem, QueueItem, QueueItemStatus

# ---------------------------------------------------------------------------
# format_queue_file_status
# ---------------------------------------------------------------------------


def test_completed_without_skip_reason_shows_done():
    assert format_queue_file_status(QueueItemStatus.COMPLETED) == ("Done", "file_done")


def test_completed_with_skip_reason_shows_skipped():
    text, tag = format_queue_file_status(
        QueueItemStatus.COMPLETED, skip_reason="Not worth converting (VMAF 95 unattainable)"
    )
    assert text == "Skipped: Not worth converting (VMAF 95 unattainable)"
    assert tag == "file_skipped"


def test_error_shows_error_message():
    assert format_queue_file_status(QueueItemStatus.ERROR, error_message="Disk full") == ("Disk full", "file_error")


def test_skip_reason_ignored_for_non_completed_statuses():
    assert format_queue_file_status(QueueItemStatus.CONVERTING, skip_reason="stale") == (
        "Converting...",
        "file_converting",
    )


def test_stopping_pending_shows_will_skip():
    assert format_queue_file_status(QueueItemStatus.PENDING, stopping=True) == ("Will skip", "file_skipped")


# ---------------------------------------------------------------------------
# QueueFileItem skip_reason serialization
# ---------------------------------------------------------------------------


def _folder_item(files: list[QueueFileItem]) -> QueueItem:
    return QueueItem(id="test-id", source_path="/videos", is_folder=True, files=files)


def test_skip_reason_round_trips():
    item = _folder_item(
        [
            QueueFileItem(
                path="/videos/a.mp4", status=QueueItemStatus.COMPLETED, size_bytes=100, skip_reason="Already converted"
            ),
            QueueFileItem(path="/videos/b.mp4", status=QueueItemStatus.COMPLETED, size_bytes=200),
        ]
    )

    restored = QueueItem.from_dict(item.to_dict())

    assert restored.files[0].skip_reason == "Already converted"
    assert restored.files[1].skip_reason is None


def test_missing_skip_reason_key_defaults_to_none():
    data = _folder_item([QueueFileItem(path="/videos/a.mp4")]).to_dict()
    del data["files"][0]["skip_reason"]

    restored = QueueItem.from_dict(data)

    assert restored.files[0].skip_reason is None
