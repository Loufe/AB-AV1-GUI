# tools/test_export_history_v3.py
"""Tests for the V2 history export (docs/HISTORY_IMPORT.md schema).

Run with: uvx pytest tools/test_export_history_v3.py
"""

import datetime

from export_history_v3 import export_records


def identity_resolve(path: str) -> str:
    return path


def make_record(path: str, *, status: str = "scanned", **overrides) -> dict:
    record = {
        "path_hash": f"hash-of:{path}",
        "original_path": path,
        "status": status,
        "file_size_bytes": 1000,
        "file_mtime": 1000.0,
    }
    record.update(overrides)
    return record


def export_one(record: dict) -> dict:
    exported, _stats = export_records([record], resolve_path=identity_resolve)
    assert len(exported) == 1
    return exported[0]


def local_epoch_ms(iso: str) -> int:
    return round(datetime.datetime.fromisoformat(iso).timestamp() * 1000)


def test_scanned_record_exports_stamp_and_metadata_only():
    record = export_one(
        make_record(
            "/videos/movie.mkv",
            video_codec="h264",
            width=1920,
            height=1080,
            duration_sec=61.5,
            best_crf=30.0,  # stray Layer-2 data on a scanned record is not trusted
        )
    )

    assert record == {
        "path": "/videos/movie.mkv",
        "status": "scanned",
        "size": 1000,
        "modified_ns": "1000000000000",
        "video_codec": "h264",
        "width": 1920,
        "height": 1080,
        "duration_ms": 61500,
    }


def test_converted_record_prefers_final_values_and_scales_them():
    record = export_one(
        make_record(
            "/videos/movie.mkv",
            status="converted",
            output_size_bytes=400,
            encoding_time_sec=88.25,
            best_crf=28.0,
            best_vmaf_achieved=94.0,
            vmaf_target_when_analyzed=94,
            final_crf=30.5,
            final_vmaf=95.55,
            vmaf_target_used=95,
            last_updated="2024-01-02 03:04:05",
        )
    )

    assert record["output_size"] == 400
    assert record["encoding_time_ms"] == 88250
    assert record["crf_thousandths"] == 30500
    assert record["vmaf_hundredths"] == 9555
    assert record["target"] == 95
    assert record["decided_at_ms"] == local_epoch_ms("2024-01-02 03:04:05")


def test_converted_record_falls_back_to_analysis_values():
    record = export_one(
        make_record(
            "/videos/movie.mkv",
            status="converted",
            best_crf=28.0,
            best_vmaf_achieved=94.2,
            vmaf_target_when_analyzed=94,
        )
    )

    assert record["crf_thousandths"] == 28000
    assert record["vmaf_hundredths"] == 9420
    assert record["target"] == 94


def test_analyzed_record_exports_search_results():
    record = export_one(
        make_record(
            "/videos/movie.mkv",
            status="analyzed",
            best_crf=27.25,
            best_vmaf_achieved=95.0,
            vmaf_target_when_analyzed=95,
        )
    )

    assert record["status"] == "analyzed"
    assert record["crf_thousandths"] == 27250
    assert record["vmaf_hundredths"] == 9500
    assert record["target"] == 95
    assert "output_size" not in record


def test_not_worthwhile_record_exports_both_targets():
    record = export_one(
        make_record("/videos/movie.mkv", status="not_worthwhile", vmaf_target_attempted=95, min_vmaf_attempted=90)
    )

    assert record["status"] == "not_worthwhile"
    assert record["requested_target"] == 95
    assert record["floor_target"] == 90
    assert "crf_thousandths" not in record


def test_anonymized_and_unknown_status_records_are_dropped():
    anonymized = make_record("/videos/movie.mkv")
    anonymized["original_path"] = None
    weird = make_record("/videos/other.mkv", status="mystery")

    exported, stats = export_records([anonymized, weird], resolve_path=identity_resolve)

    assert exported == []
    assert stats["dropped_anonymized"] == 1
    assert stats["dropped_unknown_status"] == 1
    assert stats["total_out"] == 0


def test_out_of_range_and_nonfinite_values_are_omitted_not_guessed():
    record = export_one(
        make_record(
            "/videos/movie.mkv",
            status="analyzed",
            file_size_bytes=-5,
            file_mtime=float("nan"),
            duration_sec=float("inf"),
            width=None,
            best_crf=28.0,
            best_vmaf_achieved=101.0,  # would exceed the 10000-hundredths cap
            vmaf_target_when_analyzed=150,  # outside 0-100
        )
    )

    assert "size" not in record
    assert "modified_ns" not in record
    assert "duration_ms" not in record
    assert "width" not in record
    assert record["crf_thousandths"] == 28000
    assert "vmaf_hundredths" not in record
    assert "target" not in record


def test_decided_at_falls_back_to_first_seen_then_is_omitted():
    fallback = export_one(
        make_record("/videos/a.mkv", last_updated="not a timestamp", first_seen="2023-06-01 12:00:00")
    )
    assert fallback["decided_at_ms"] == local_epoch_ms("2023-06-01 12:00:00")

    missing = make_record("/videos/b.mkv")
    exported, stats = export_records([missing], resolve_path=identity_resolve)
    assert "decided_at_ms" not in exported[0]
    assert stats["missing_decided_at"] == 1


def test_paths_are_resolved_and_output_is_sorted_by_path():
    def unc_resolve(path: str) -> str:
        return path.replace("B:", "\\\\server\\share")

    records = [make_record("B:\\zebra.mkv"), make_record("B:\\alpha.mkv")]

    exported, stats = export_records(records, resolve_path=unc_resolve)

    assert [record["path"] for record in exported] == ["\\\\server\\share\\alpha.mkv", "\\\\server\\share\\zebra.mkv"]
    assert stats["total_out"] == 2
