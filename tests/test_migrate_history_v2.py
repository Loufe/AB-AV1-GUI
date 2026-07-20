# tests/test_migrate_history_v2.py
"""Tests for the schema-v2 history migration (ADR-002)."""

import pytest
from tools.migrate_history_v2 import migrate_records


def identity_rekey(path: str) -> tuple[str, str]:
    return f"hash-of:{path}", path


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


def test_alias_records_are_dropped():
    records = [
        make_record("/videos/movie.mkv", status="converted"),
        make_record("/copies/movie.mkv", status="converted", duplicate_of="hash-of:/videos/movie.mkv"),
    ]

    migrated, stats = migrate_records(records, rekey=identity_rekey)

    assert stats["dropped_aliases"] == 1
    assert len(migrated) == 1
    assert migrated[0]["path_hash"] == "hash-of:/videos/movie.mkv"
    assert all("duplicate_of" not in r for r in migrated)


def test_conversion_time_folds_into_encoding_time():
    records = [
        make_record("/a.mkv", status="converted", conversion_time_sec=300.0),
        make_record("/b.mkv", status="converted", conversion_time_sec=300.0, encoding_time_sec=200.0),
    ]

    migrated, stats = migrate_records(records, rekey=identity_rekey)

    by_path = {r["original_path"]: r for r in migrated}
    # Folded where encoding_time_sec was absent; existing split timing wins otherwise
    assert by_path["/a.mkv"]["encoding_time_sec"] == 300.0
    assert by_path["/b.mkv"]["encoding_time_sec"] == 200.0
    assert stats["folded_times"] == 1
    assert all("conversion_time_sec" not in r for r in migrated)


def test_pre_analyzed_era_status_normalizes():
    records = [
        make_record("/a.mkv", status="scanned", best_crf=28.0, best_vmaf_achieved=95.5),
        make_record("/b.mkv", status="scanned"),  # No Layer-2 data: stays scanned
    ]

    migrated, stats = migrate_records(records, rekey=identity_rekey)

    by_path = {r["original_path"]: r for r in migrated}
    assert by_path["/a.mkv"]["status"] == "analyzed"
    assert by_path["/b.mkv"]["status"] == "scanned"
    assert stats["normalized_statuses"] == 1


def test_drive_letter_records_rekey_and_merge_with_unc_twin():
    """The same file recorded via mapped drive and UNC collapses to one record,
    keeping the better verdict and the earliest first_seen."""

    def unc_rekey(path: str) -> tuple[str, str]:
        resolved = path.replace("B:\\", "\\\\nas\\share\\")
        return f"hash-of:{resolved.lower()}", resolved

    records = [
        make_record(
            "B:\\videos\\movie.mkv",
            path_hash="old-drive-hash",
            status="converted",
            first_seen="2025-01-01 00:00:00",
            last_updated="2025-06-01 00:00:00",
        ),
        make_record(
            "\\\\nas\\share\\videos\\movie.mkv",
            path_hash="hash-of:\\\\nas\\share\\videos\\movie.mkv".lower(),
            status="scanned",
            first_seen="2024-01-01 00:00:00",
            last_updated="2026-01-01 00:00:00",
        ),
    ]
    # Make the UNC twin's stored hash match what unc_rekey computes for it
    records[1]["path_hash"] = unc_rekey(records[1]["original_path"])[0]

    migrated, stats = migrate_records(records, rekey=unc_rekey)

    assert stats["rekeyed"] >= 1
    assert stats["merged"] == 1
    assert len(migrated) == 1
    winner = migrated[0]
    # Higher status (converted) wins; UNC spelling is the surviving key
    assert winner["status"] == "converted"
    assert winner["original_path"] == "\\\\nas\\share\\videos\\movie.mkv"
    assert winner["path_hash"] == unc_rekey("B:\\videos\\movie.mkv")[0]
    # Earliest first_seen across both spellings survives
    assert winner["first_seen"] == "2024-01-01 00:00:00"


def test_merge_tie_broken_by_last_updated():
    records = [
        make_record("/same.mkv", status="analyzed", best_crf=28.0, last_updated="2025-01-01 00:00:00"),
        make_record("/same.mkv", status="analyzed", best_crf=30.0, last_updated="2026-01-01 00:00:00"),
    ]

    migrated, stats = migrate_records(records, rekey=identity_rekey)

    assert stats["merged"] == 1
    assert len(migrated) == 1
    assert migrated[0]["best_crf"] == 30.0  # Most recently updated wins the tie


def test_anonymized_records_keep_stored_hash():
    records = [make_record("/a.mkv", original_path=None, path_hash="anon-hash-1234")]

    migrated, stats = migrate_records(records, rekey=identity_rekey)

    assert stats["rekeyed"] == 0
    assert migrated[0]["path_hash"] == "anon-hash-1234"
    assert migrated[0]["original_path"] is None


def test_unknown_keys_are_dropped():
    records = [make_record("/a.mkv", some_removed_field=123, another_junk_key="x")]

    migrated, stats = migrate_records(records, rekey=identity_rekey)

    assert stats["dropped_unknown_keys"] == 2
    assert "some_removed_field" not in migrated[0]
    assert "another_junk_key" not in migrated[0]


def test_migration_is_idempotent():
    records = [
        make_record("/a.mkv", status="converted", conversion_time_sec=300.0),
        make_record("/b.mkv", status="scanned", best_crf=28.0, best_vmaf_achieved=95.5),
        make_record("/copy.mkv", duplicate_of="hash-of:/a.mkv"),
    ]

    once, _ = migrate_records(records, rekey=identity_rekey)
    twice, stats = migrate_records(once, rekey=identity_rekey)

    assert twice == once
    assert stats["dropped_aliases"] == 0
    assert stats["folded_times"] == 0
    assert stats["normalized_statuses"] == 0
    assert stats["merged"] == 0


def test_pre_audio_streams_format_is_rejected():
    records = [make_record("/a.mkv", audio_codec="aac")]

    with pytest.raises(ValueError, match="pre-audio_streams"):
        migrate_records(records, rekey=identity_rekey)
