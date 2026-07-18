# tests/test_tree_display.py
"""Tests for queue file status formatting and skip reason persistence."""

from src.gui.tree_display import format_queue_file_status
from src.models import QueueFileItem, QueueItem, QueueItemStatus


class TestFormatQueueFileStatus:
    def test_completed_without_skip_reason_shows_done(self):
        text, tag = format_queue_file_status(QueueItemStatus.COMPLETED)
        assert text == "Done"
        assert tag == "file_done"

    def test_completed_with_skip_reason_shows_skipped(self):
        text, tag = format_queue_file_status(
            QueueItemStatus.COMPLETED, skip_reason="Not worth converting (VMAF 95 unattainable)"
        )
        assert text == "Skipped: Not worth converting (VMAF 95 unattainable)"
        assert tag == "file_skipped"

    def test_error_shows_error_message(self):
        text, tag = format_queue_file_status(QueueItemStatus.ERROR, error_message="Disk full")
        assert text == "Disk full"
        assert tag == "file_error"

    def test_skip_reason_ignored_for_non_completed_statuses(self):
        text, tag = format_queue_file_status(QueueItemStatus.CONVERTING, skip_reason="stale")
        assert text == "Converting..."
        assert tag == "file_converting"

    def test_stopping_pending_shows_will_skip(self):
        text, tag = format_queue_file_status(QueueItemStatus.PENDING, stopping=True)
        assert text == "Will skip"
        assert tag == "file_skipped"


class TestQueueFileItemSerialization:
    def test_skip_reason_round_trips(self):
        item = QueueItem(
            id="test-id",
            source_path="/videos",
            is_folder=True,
            files=[
                QueueFileItem(
                    path="/videos/a.mp4",
                    status=QueueItemStatus.COMPLETED,
                    size_bytes=100,
                    skip_reason="Already converted",
                ),
                QueueFileItem(path="/videos/b.mp4", status=QueueItemStatus.COMPLETED, size_bytes=200),
            ],
        )

        restored = QueueItem.from_dict(item.to_dict())

        assert restored.files[0].skip_reason == "Already converted"
        assert restored.files[1].skip_reason is None

    def test_missing_skip_reason_key_defaults_to_none(self):
        data = QueueItem(
            id="test-id", source_path="/videos", is_folder=True, files=[QueueFileItem(path="/videos/a.mp4")]
        ).to_dict()
        del data["files"][0]["skip_reason"]

        restored = QueueItem.from_dict(data)

        assert restored.files[0].skip_reason is None
