# tests/test_tree_display.py
"""Tests for tree display helpers in src/gui/tree_display.py.

Covers queue file status formatting (including skip reason persistence) and
compute_analysis_display_values. The latter is exercised with explicit
FileRecord instances and pre-computed percentiles so the history index is
never touched. Key contract (issue #6): the time estimate depends only on
codec/duration/resolution and must not be suppressed when the savings
estimate is unavailable.
"""

from src.gui.tree_display import compute_analysis_display_values, format_queue_file_status
from src.models import FileRecord, FileStatus, QueueFileItem, QueueItem, QueueItemStatus
from src.utils import format_file_size

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


# ---------------------------------------------------------------------------
# compute_analysis_display_values
# ---------------------------------------------------------------------------

# duration 600s * p50 2.0 = 1200s = 20m; count 10 => high confidence (no prefix)
PERCENTILES = {("h264", "1080p"): {"p25": 1.0, "p50": 2.0, "p75": 3.0, "count": 10}}

ONE_GB = 1_073_741_824


def make_record(**overrides) -> FileRecord:
    defaults = {
        "path_hash": "a" * 32,
        "original_path": None,
        "status": FileStatus.SCANNED,
        "file_size_bytes": ONE_GB,
        "file_mtime": 0.0,
        "duration_sec": 600.0,
        "video_codec": "h264",
        "width": 1920,
        "height": 1080,
        "estimated_reduction_percent": 50.0,
    }
    defaults.update(overrides)
    return FileRecord(**defaults)


def test_scanned_record_shows_savings_time_and_efficiency():
    record = make_record()
    _, size_str, savings_str, time_str, eff_str, tag = compute_analysis_display_values(
        record, grouped_percentiles=PERCENTILES
    )
    assert size_str == format_file_size(ONE_GB)
    assert savings_str == f"~{format_file_size(ONE_GB // 2)}"  # Layer 1 => "~" prefix
    assert time_str == "20m"
    assert eff_str == "1.5 GB/h"  # 0.5 GB saved in 20 minutes
    assert tag == ""


def test_time_shown_without_reduction_estimate():
    # Issue #6 regression: a record without a savings estimate (e.g., an alias
    # record) must still show a time estimate - time needs only duration/codec.
    record = make_record(estimated_reduction_percent=None)
    _, _, savings_str, time_str, eff_str, _ = compute_analysis_display_values(record, grouped_percentiles=PERCENTILES)
    assert savings_str == "—"
    assert time_str == "20m"
    assert eff_str == "—"


def test_time_shown_without_file_size():
    record = make_record(file_size_bytes=0)
    _, size_str, savings_str, time_str, _, _ = compute_analysis_display_values(record, grouped_percentiles=PERCENTILES)
    assert size_str == "—"
    assert savings_str == "—"
    assert time_str == "20m"


def test_analyzed_record_uses_precise_prediction():
    record = make_record(status=FileStatus.ANALYZED, predicted_size_reduction=25.0, estimated_reduction_percent=50.0)
    _, _, savings_str, time_str, _, _ = compute_analysis_display_values(record, grouped_percentiles=PERCENTILES)
    # Layer 2 prediction wins over the Layer 1 estimate and drops the "~" prefix
    assert savings_str == format_file_size(ONE_GB // 4)
    assert time_str == "20m"


def test_no_duration_shows_no_time_but_keeps_savings():
    record = make_record(duration_sec=None)
    _, _, savings_str, time_str, eff_str, _ = compute_analysis_display_values(record, grouped_percentiles=PERCENTILES)
    assert savings_str == f"~{format_file_size(ONE_GB // 2)}"
    assert time_str == "—"
    assert eff_str == "—"


def test_no_percentile_data_shows_no_time():
    record = make_record()
    _, _, savings_str, time_str, eff_str, _ = compute_analysis_display_values(record, grouped_percentiles={})
    assert savings_str == f"~{format_file_size(ONE_GB // 2)}"
    assert time_str == "—"
    assert eff_str == "—"


def test_terminal_statuses_show_verdict_not_estimates():
    converted = make_record(status=FileStatus.CONVERTED)
    _, _, savings_str, time_str, _, tag = compute_analysis_display_values(converted, grouped_percentiles=PERCENTILES)
    assert (savings_str, time_str, tag) == ("Done", "—", "done")

    not_worthwhile = make_record(status=FileStatus.NOT_WORTHWHILE)
    _, _, savings_str, time_str, _, tag = compute_analysis_display_values(
        not_worthwhile, grouped_percentiles=PERCENTILES
    )
    assert (savings_str, time_str, tag) == ("Skip", "—", "skip")


def test_av1_file_shows_av1_indicator():
    record = make_record(video_codec="av1")
    _, _, savings_str, time_str, _, tag = compute_analysis_display_values(record, grouped_percentiles=PERCENTILES)
    assert (savings_str, time_str, tag) == ("AV1", "—", "av1")
