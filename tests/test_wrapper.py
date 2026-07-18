# tests/test_wrapper.py
"""Tests for the pure helpers in src/ab_av1/wrapper.py.

_is_no_suitable_crf pins the VMAF-fallback trigger contract: it must tolerate
wording drift (case, suffixes) in ab-av1's NoGoodCrf message, because ab-av1 is
auto-updated and a silent mismatch would disable the entire fallback ladder.
"""

from src.ab_av1.runner import ProcessResult
from src.ab_av1.wrapper import _format_cmd_for_log, _is_no_suitable_crf


def _failed(error_line, output=""):
    return ProcessResult(return_code=1, output=output, error_line=error_line)


class TestIsNoSuitableCrf:
    def test_exact_message_matches(self):
        assert _is_no_suitable_crf(_failed("Failed to find a suitable crf")) is True

    def test_suffixed_message_matches(self):
        assert _is_no_suitable_crf(_failed("Failed to find a suitable crf, last crf 17 vmaf 94.2")) is True

    def test_case_insensitive(self):
        assert _is_no_suitable_crf(_failed("failed to find a suitable CRF")) is True

    def test_falls_back_to_output_when_no_error_line(self):
        result = _failed(None, output="[DEBUG ab_av1] ...\nFailed to find a suitable crf\n[TRACE] tail")
        assert _is_no_suitable_crf(result) is True

    def test_unrelated_failure_does_not_match(self):
        assert _is_no_suitable_crf(_failed("Invalid data found when processing input")) is False

    def test_no_error_line_and_unrelated_output_does_not_match(self):
        assert _is_no_suitable_crf(_failed(None, output="ffmpeg exited with code 1")) is False


class TestFormatCmdForLog:
    def test_replaces_known_tokens_and_keeps_the_rest(self):
        cmd = ["/vendor/ab-av1", "auto-encode", "-i", "/videos/movie.mp4", "-o", "/out/movie.mkv", "--preset", "6"]
        replacements = {
            "/vendor/ab-av1": "ab-av1",
            "/videos/movie.mp4": "file_8a4b.mp4",
            "/out/movie.mkv": "file_1a2b.mkv",
        }
        assert (
            _format_cmd_for_log(cmd, replacements) == "ab-av1 auto-encode -i file_8a4b.mp4 -o file_1a2b.mkv --preset 6"
        )

    def test_no_replacements_returns_joined_cmd(self):
        cmd = ["ab-av1", "crf-search", "--min-vmaf", "95"]
        assert _format_cmd_for_log(cmd, {}) == "ab-av1 crf-search --min-vmaf 95"
