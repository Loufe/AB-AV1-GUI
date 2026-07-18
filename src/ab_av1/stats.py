# src/ab_av1/stats.py
"""
Result and live-state containers for ab-av1 runs.

EncodeStats is the mutable parse state threaded through AbAv1Parser during a
run; CrfSearchResult is the immutable outcome of a crf-search.
"""

from dataclasses import dataclass


@dataclass
class EncodeStats:
    """Live parse state and final statistics for an encode or crf-search run."""

    input_path: str = ""
    output_path: str = ""
    command: str = ""
    phase: str = "crf-search"  # "crf-search" | "encoding"
    progress_quality: float = 0.0
    progress_encoding: float = 0.0
    vmaf: float | None = None
    crf: float | None = None
    size_reduction: float | None = None
    original_size: int | None = None
    output_size: int | None = None
    vmaf_target_used: int | None = None
    total_duration_seconds: float = 0.0
    last_ffmpeg_fps: float | None = None
    eta_text: str | None = None
    used_cached_crf: bool = False
    crf_search_time_sec: float = 0.0
    encoding_time_sec: float = 0.0

    def reset_for_attempt(self, vmaf_target: int) -> None:
        """Reset per-attempt parse state before a VMAF fallback retry.

        size_reduction is deliberately NOT reset: the prediction from an
        earlier attempt remains the best available estimate until the next
        attempt parses a new one.
        """
        self.phase = "crf-search"
        self.progress_quality = 0.0
        self.progress_encoding = 0.0
        self.vmaf = None
        self.crf = None
        self.eta_text = None
        self.last_ffmpeg_fps = None
        self.vmaf_target_used = vmaf_target


@dataclass
class CrfSearchResult:
    """Outcome of a successful crf-search (quality analysis without encoding)."""

    best_crf: float
    best_vmaf: float
    predicted_size_reduction: float | None
    predicted_output_size: int | None
    vmaf_target_used: int
    original_size: int | None
    used_fallback: bool
    preset_used: int
    crf_search_time_sec: float
