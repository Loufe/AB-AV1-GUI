# tests/test_estimation.py
"""Characterization tests for the pure math in src/estimation.py.

Functions needing get_history_index() or a gui object (compute_grouped_*,
estimate_pending_files_eta, estimate_remaining_time) are not covered;
estimate_file_time is exercised only with explicit metadata and pre-computed
percentiles so the history index is never touched.
"""

import time

from src.estimation import compute_percentiles, estimate_current_file_eta, estimate_file_time, get_resolution_bucket

# ---------------------------------------------------------------------------
# get_resolution_bucket
# ---------------------------------------------------------------------------


def test_resolution_buckets_standard_sizes():
    assert get_resolution_bucket(3840, 2160) == "4k"
    assert get_resolution_bucket(2560, 1440) == "1440p"
    assert get_resolution_bucket(1920, 1080) == "1080p"
    assert get_resolution_bucket(1280, 720) == "720p"
    assert get_resolution_bucket(720, 480) == "sd"


def test_resolution_bucket_uses_pixel_count_not_orientation():
    # Portrait video with the same pixel count lands in the same bucket.
    assert get_resolution_bucket(1080, 1920) == "1080p"


def test_resolution_bucket_boundary_just_below_720p_is_sd():
    assert get_resolution_bucket(1279, 720) == "sd"


def test_resolution_bucket_missing_dimensions():
    assert get_resolution_bucket(None, 1080) == "unknown"
    assert get_resolution_bucket(1920, None) == "unknown"
    assert get_resolution_bucket(0, 0) == "unknown"


# ---------------------------------------------------------------------------
# compute_percentiles
# ---------------------------------------------------------------------------


def test_compute_percentiles_requires_minimum_samples():
    assert compute_percentiles([]) is None
    assert compute_percentiles([1.0, 2.0, 3.0, 4.0]) is None  # MIN_SAMPLES_FOR_ESTIMATE = 5


def test_compute_percentiles_quartiles():
    result = compute_percentiles([1.0, 2.0, 3.0, 4.0, 5.0])
    assert result == {"p25": 1.5, "p50": 3.0, "p75": 4.5, "count": 5}


# ---------------------------------------------------------------------------
# estimate_file_time (tiered lookup with pre-computed percentiles)
# ---------------------------------------------------------------------------

STATS_10 = {"p25": 1.0, "p50": 2.0, "p75": 3.0, "count": 10}
STATS_5 = {"p25": 1.0, "p50": 2.0, "p75": 3.0, "count": 5}


def test_tier1_codec_and_resolution_high_confidence():
    estimate = estimate_file_time(
        codec="h264", duration=600.0, width=1920, height=1080, grouped_percentiles={("h264", "1080p"): STATS_10}
    )
    assert estimate.confidence == "high"
    assert estimate.source == "h264:1080p"
    assert estimate.min_seconds == 600.0
    assert estimate.best_seconds == 1200.0
    assert estimate.max_seconds == 1800.0


def test_tier1_medium_confidence_below_10_samples():
    estimate = estimate_file_time(
        codec="h264", duration=600.0, width=1920, height=1080, grouped_percentiles={("h264", "1080p"): STATS_5}
    )
    assert estimate.confidence == "medium"
    assert estimate.source == "h264:1080p"


def test_tier2_codec_only_fallback():
    estimate = estimate_file_time(
        codec="h264", duration=600.0, width=1920, height=1080, grouped_percentiles={("h264", None): STATS_10}
    )
    assert estimate.confidence == "medium"
    assert estimate.source == "codec:h264"
    assert estimate.best_seconds == 1200.0


def test_tier3_global_fallback():
    estimate = estimate_file_time(
        codec="h264", duration=600.0, width=1920, height=1080, grouped_percentiles={(None, None): STATS_10}
    )
    assert estimate.confidence == "low"
    assert estimate.source == "global"


def test_tier4_no_matching_group():
    estimate = estimate_file_time(codec="h264", duration=600.0, width=1920, height=1080, grouped_percentiles={})
    assert estimate.confidence == "none"
    assert estimate.source == "insufficient_data"
    assert estimate.best_seconds == 0


def test_missing_duration_short_circuits():
    estimate = estimate_file_time(
        codec="h264", width=1920, height=1080, grouped_percentiles={("h264", "1080p"): STATS_10}
    )
    assert estimate.confidence == "none"
    assert estimate.source == "no_duration"


def test_unknown_resolution_skips_tier1():
    estimate = estimate_file_time(
        codec="h264", duration=600.0, grouped_percentiles={("h264", "unknown"): STATS_10, ("h264", None): STATS_5}
    )
    assert estimate.source == "codec:h264"


# ---------------------------------------------------------------------------
# estimate_current_file_eta
# ---------------------------------------------------------------------------


def test_eta_zero_when_not_running():
    assert estimate_current_file_eta(False, 100.0, time.time(), 50.0, time.time()) == 0


def test_eta_from_stored_ab_av1_value(monkeypatch):
    monkeypatch.setattr(time, "time", lambda: 1000.0)
    eta = estimate_current_file_eta(
        running=True,
        last_eta_seconds=100.0,
        last_eta_timestamp=990.0,
        encoding_progress=50.0,
        encoding_start_time=900.0,
    )
    assert eta == 90.0  # stored ETA minus the 10s elapsed since it was reported


def test_eta_from_stored_value_clamps_at_zero(monkeypatch):
    monkeypatch.setattr(time, "time", lambda: 1000.0)
    eta = estimate_current_file_eta(
        running=True,
        last_eta_seconds=100.0,
        last_eta_timestamp=850.0,
        encoding_progress=50.0,
        encoding_start_time=900.0,
    )
    assert eta == 0


def test_eta_falls_back_to_progress_extrapolation(monkeypatch):
    monkeypatch.setattr(time, "time", lambda: 1000.0)
    eta = estimate_current_file_eta(
        running=True, last_eta_seconds=None, last_eta_timestamp=None, encoding_progress=50.0, encoding_start_time=900.0
    )
    # 100s elapsed at 50% => 200s total => 100s remaining
    assert eta == 100.0


def test_eta_zero_without_progress_or_stored_value():
    eta = estimate_current_file_eta(
        running=True, last_eta_seconds=None, last_eta_timestamp=None, encoding_progress=0.0, encoding_start_time=None
    )
    assert eta == 0
