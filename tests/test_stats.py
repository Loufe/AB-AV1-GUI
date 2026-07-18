# tests/test_stats.py
"""Tests for EncodeStats (src/ab_av1/stats.py)."""

from src.ab_av1.stats import EncodeStats


def test_reset_for_attempt_clears_per_attempt_state():
    stats = EncodeStats(
        phase="encoding",
        progress_quality=80.0,
        progress_encoding=45.0,
        vmaf=93.4,
        crf=27.0,
        eta_text="5m",
        last_ffmpeg_fps=120.0,
        vmaf_target_used=95,
    )

    stats.reset_for_attempt(94)

    assert stats.phase == "crf-search"
    assert stats.progress_quality == 0.0
    assert stats.progress_encoding == 0.0
    assert stats.vmaf is None
    assert stats.crf is None
    assert stats.eta_text is None
    assert stats.last_ffmpeg_fps is None
    assert stats.vmaf_target_used == 94


def test_reset_for_attempt_preserves_size_reduction():
    # The prediction from an earlier attempt remains the best available estimate
    # until the next attempt parses a new one.
    stats = EncodeStats(size_reduction=42.5)

    stats.reset_for_attempt(94)

    assert stats.size_reduction == 42.5
