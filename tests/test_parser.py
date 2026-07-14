# tests/test_parser.py
"""Characterization tests for AbAv1Parser.parse_line (src/ab_av1/parser.py).

Lines are modeled on real ab-av1/ffmpeg output as documented in
docs/AB_AV1_PARSING.md. The stats dict is seeded the same way
AbAv1Wrapper.auto_encode() seeds it (src/ab_av1/wrapper.py).
"""

from src.ab_av1.parser import AbAv1Parser


class CallbackRecorder:
    """Stub file_info_callback that records every invocation."""

    def __init__(self):
        self.calls: list[tuple[str, str, object]] = []

    def __call__(self, filename: str, status: str, info: object) -> None:
        self.calls.append((filename, status, info))

    @property
    def events(self) -> list[object]:
        return [info for _, _, info in self.calls]


def make_stats(input_path: str = "/videos/movie.mp4", total_duration_seconds: float | None = 600.0) -> dict:
    """Seed a stats dict the way AbAv1Wrapper.auto_encode() does."""
    return {
        "phase": "crf-search",
        "progress_quality": 0,
        "progress_encoding": 0,
        "vmaf": None,
        "crf": None,
        "size_reduction": None,
        "input_path": input_path,
        "output_path": "/videos/movie.mkv",
        "command": "",
        "vmaf_target_used": 95,
        "last_ffmpeg_fps": None,
        "eta_text": None,
        "total_duration_seconds": total_duration_seconds,
        "last_reported_encoding_progress": -1.0,
        "estimated_output_size": None,
        "estimated_size_reduction": None,
    }


def make_encoding_stats(**kwargs) -> dict:
    """Stats dict as it looks after the parser's phase transition to encoding."""
    stats = make_stats(**kwargs)
    stats.update({"phase": "encoding", "progress_quality": 100.0, "progress_encoding": 0.0, "crf": 30, "vmaf": 95.5})
    return stats


def make_parser() -> tuple[AbAv1Parser, CallbackRecorder]:
    recorder = CallbackRecorder()
    return AbAv1Parser(file_info_callback=recorder), recorder


# ---------------------------------------------------------------------------
# CRF search phase
# ---------------------------------------------------------------------------


def test_crf_vmaf_line_updates_stats_and_fires_progress_callback():
    parser, recorder = make_parser()
    stats = make_stats()

    parser.parse_line("crf 30 VMAF 96.50 (23%)", stats)

    assert stats["crf"] == 30
    assert stats["vmaf"] == 96.5
    assert stats["progress_quality"] == 10.0
    assert len(recorder.calls) == 1
    filename, status, event = recorder.calls[0]
    assert filename == "movie.mp4"  # basename of input_path, not anonymized
    assert status == "progress"
    assert event.phase == "crf-search"
    assert event.progress_quality == 10.0
    assert event.progress_encoding == 0
    assert event.message == "Detecting Quality (CRF:30, VMAF:96.5)"


def test_repeated_crf_vmaf_lines_cap_quality_progress_at_90():
    parser, recorder = make_parser()
    stats = make_stats()

    for i in range(12):
        parser.parse_line(f"crf {30 + i} VMAF 9{i % 10}.00", stats)

    assert stats["progress_quality"] == 90.0
    # Callbacks only fire while progress increases: 10 -> 90 in 10% steps.
    assert len(recorder.calls) == 9


def test_best_crf_line_sets_crf_and_95_percent_quality():
    parser, recorder = make_parser()
    stats = make_stats()

    parser.parse_line("Best CRF: 31", stats)

    assert stats["crf"] == 31
    assert stats["progress_quality"] == 95.0
    assert len(recorder.calls) == 1
    assert recorder.events[0].crf == 31


def test_predicted_size_reduction_line_stores_reduction_without_callback():
    parser, recorder = make_parser()
    stats = make_stats()

    parser.parse_line("predicted video stream size 450MB (65%)", stats)

    # Line reports predicted size as 65% of original => 35% reduction.
    assert stats["size_reduction"] == 35.0
    # Quality progress did not increase, so no callback fires.
    assert recorder.calls == []


def test_unrelated_line_in_crf_search_changes_nothing():
    parser, recorder = make_parser()
    stats = make_stats()
    before = dict(stats)

    result = parser.parse_line("Svt[info]: SVT [version]: SVT-AV1 Encoder Lib v2.1.0", stats)

    assert result == before
    assert recorder.calls == []


def test_empty_line_returns_stats_unchanged():
    parser, recorder = make_parser()
    stats = make_stats()
    before = dict(stats)

    result = parser.parse_line("   ", stats)

    assert result is stats
    assert result == before
    assert recorder.calls == []


# ---------------------------------------------------------------------------
# Phase transition
# ---------------------------------------------------------------------------


def test_encode_log_line_triggers_phase_transition():
    parser, recorder = make_parser()
    stats = make_stats()
    stats.update({"progress_quality": 95.0, "crf": 31, "vmaf": 95.1})

    parser.parse_line("[2024-01-01T00:00:00Z INFO ab_av1::command::encode] encoding video.mkv", stats)

    assert stats["phase"] == "encoding"
    assert stats["progress_quality"] == 100.0
    assert stats["progress_encoding"] == 0.0
    assert len(recorder.calls) == 1
    event = recorder.events[0]
    assert event.message == "Encoding started"
    assert event.phase == "encoding"
    assert event.progress_quality == 100.0
    assert event.crf == 31


def test_starting_encoding_text_also_triggers_transition():
    parser, _ = make_parser()
    stats = make_stats()

    parser.parse_line("Starting encoding", stats)

    assert stats["phase"] == "encoding"


def test_encode_log_line_ignored_when_already_encoding():
    parser, recorder = make_parser()
    stats = make_encoding_stats()

    parser.parse_line("[2024-01-01T00:00:00Z INFO ab_av1::command::encode] encoding video.mkv", stats)

    assert stats["phase"] == "encoding"
    assert recorder.calls == []


# ---------------------------------------------------------------------------
# Encoding phase: ab-av1 structured progress
# ---------------------------------------------------------------------------


def test_sample_encode_progress_line():
    parser, recorder = make_parser()
    stats = make_encoding_stats()

    parser.parse_line("[2024-01-01T00:00:00Z INFO ab_av1::command::sample_encode] 45.2%, 30 fps, eta 5m 30s", stats)

    assert stats["progress_encoding"] == 45.2
    assert stats["last_ffmpeg_fps"] == 30
    assert stats["eta_text"] == "5m 30s"
    assert len(recorder.calls) == 1
    event = recorder.events[0]
    assert event.message == "Encoding: 45.2% (FPS: 30, ETA: 5m 30s)"
    assert event.progress_encoding == 45.2
    assert event.eta_text == "5m 30s"


def test_main_encode_progress_line():
    parser, recorder = make_parser()
    stats = make_encoding_stats()

    parser.parse_line("[2024-01-01T00:00:00Z INFO ab_av1::command::encode] 45%, 30 fps, eta 5m 30s", stats)

    assert stats["progress_encoding"] == 45.0
    assert stats["last_ffmpeg_fps"] == 30
    assert stats["eta_text"] == "5m 30s"
    assert len(recorder.calls) == 1
    assert recorder.events[0].message == "Encoding: 45.0% (FPS: 30, ETA: 5m 30s)"


# ---------------------------------------------------------------------------
# Encoding phase: raw ffmpeg progress
# ---------------------------------------------------------------------------

FFMPEG_LINE = "frame= 7500 fps= 25.0 q=30.0 size=   10240kB time=00:05:00.00 bitrate=4500.0kbits/s speed=1.25x"


def test_ffmpeg_time_line_computes_progress_from_duration():
    parser, recorder = make_parser()
    stats = make_encoding_stats(total_duration_seconds=600.0)

    parser.parse_line(FFMPEG_LINE, stats)

    # 300s of 600s => 50%
    assert stats["progress_encoding"] == 50.0
    assert stats["last_ffmpeg_fps"] == 25.0
    assert len(recorder.calls) == 1
    event = recorder.events[0]
    assert event.message == "Encoding: 50.0% (25 fps, 1.25x)"
    assert event.progress_quality == 100.0


def test_ffmpeg_time_line_throttles_repeat_updates():
    parser, recorder = make_parser()
    stats = make_encoding_stats(total_duration_seconds=600.0)

    parser.parse_line(FFMPEG_LINE, stats)
    parser.parse_line(FFMPEG_LINE, stats)  # same timestamp: < 0.1% increase

    assert stats["progress_encoding"] == 50.0
    assert len(recorder.calls) == 1


def test_ffmpeg_time_line_without_duration_is_ignored():
    parser, recorder = make_parser()
    stats = make_encoding_stats(total_duration_seconds=None)

    parser.parse_line(FFMPEG_LINE, stats)

    assert stats["progress_encoding"] == 0.0
    assert recorder.calls == []


def test_ffmpeg_progress_never_exceeds_99_9():
    parser, _ = make_parser()
    stats = make_encoding_stats(total_duration_seconds=100.0)

    parser.parse_line("frame= 100 fps= 25.0 time=00:02:00.00 speed=1.0x", stats)

    assert stats["progress_encoding"] == 99.9


# ---------------------------------------------------------------------------
# Encoding phase: generic percentage fallback and summary line
# ---------------------------------------------------------------------------


def test_generic_percentage_fallback():
    parser, recorder = make_parser()
    stats = make_encoding_stats()

    parser.parse_line("progress: 62%", stats)

    assert stats["progress_encoding"] == 62.0
    assert len(recorder.calls) == 1
    assert recorder.events[0].message == "Encoding: 62.0%"


def test_generic_percentage_fallback_never_regresses_progress():
    # Like the ffmpeg time= path, the generic percentage fallback requires a
    # 0.1% increase, so a stray low percentage in unrelated output cannot drag
    # the progress bar backwards.
    parser, recorder = make_parser()
    stats = make_encoding_stats()
    stats["progress_encoding"] = 80.0

    parser.parse_line("some tool output mentioning 10%", stats)

    assert stats["progress_encoding"] == 80.0
    assert recorder.calls == []


def test_ab_av1_summary_line_updates_eta_only():
    parser, recorder = make_parser()
    stats = make_encoding_stats()
    stats["progress_encoding"] = 42.0

    parser.parse_line("00:00:37 Encoding -------- (encoding, eta 36m)", stats)

    assert stats["eta_text"] == "36m"
    assert stats["progress_encoding"] == 42.0  # percentage untouched
    assert len(recorder.calls) == 1
    assert recorder.events[0].message == "Encoding: 42.0% (ETA: 36m)"

    # Same ETA again: no duplicate callback (throttled on ETA change).
    parser.parse_line("00:00:42 Encoding -------- (encoding, eta 36m)", stats)
    assert len(recorder.calls) == 1


def test_unrelated_line_in_encoding_phase_changes_nothing():
    parser, recorder = make_parser()
    stats = make_encoding_stats()
    before = dict(stats)

    parser.parse_line("Svt[info]: Number of logical cores available: 16", stats)

    assert stats == before
    assert recorder.calls == []


def test_parse_line_without_callback_does_not_raise():
    parser = AbAv1Parser(file_info_callback=None)
    stats = make_stats()

    parser.parse_line("crf 30 VMAF 96.50", stats)
    parser.parse_line("Best CRF: 30", stats)

    assert stats["crf"] == 30
    assert stats["progress_quality"] == 95.0
