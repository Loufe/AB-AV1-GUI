# src/ab_av1/wrapper.py
"""
Wrapper class for the ab-av1 tool in the AV1 Video Converter application.

Provides the high-level operations (auto-encode, crf-search, encode) on top of
runner.run_ab_av1, sharing input validation, environment setup, the VMAF
fallback loop, and failure detection across all three.

Failure detection relies on ab-av1's contract: it exits 0/1 only, and prints
"Error: {err}" to stderr as its final line on failure (merged into stdout here).
"""

import logging
import os
import time
from collections.abc import Callable
from typing import Any, NoReturn

from src.config import (
    AB_AV1_NO_SUITABLE_CRF_MESSAGE,
    DEFAULT_ENCODING_PRESET,
    DEFAULT_VMAF_TARGET,
    MIN_VMAF_FALLBACK_TARGET,
    VMAF_FALLBACK_STEP,
)
from src.privacy import anonymize_filename
from src.utils import format_crf, format_file_size, get_video_info
from src.vendor_manager import AB_AV1_EXE, AB_AV1_EXE_NAME, FFMPEG_DIR, get_ab_av1_path
from src.video_metadata import extract_video_metadata

from .checker import get_log_interval_for_duration
from .cleaner import clean_ab_av1_temp_folders
from .exceptions import AbAv1CancelledError, AbAv1Error, ConversionNotWorthwhileError, InputFileError, OutputFileError
from .parser import AbAv1Parser
from .runner import ProcessResult, run_ab_av1
from .stats import CrfSearchResult, EncodeStats

logger = logging.getLogger(__name__)


def _is_no_suitable_crf(result: ProcessResult) -> bool:
    """True if a failed run means "no CRF meets the VMAF target" (fallback trigger).

    Matched case-insensitively as a substring of the error line - falling back to
    the full output if no "Error: " line was found - so wording drift in future
    ab-av1 releases doesn't silently disable the VMAF fallback ladder.
    """
    haystack = result.error_line if result.error_line is not None else result.output
    return AB_AV1_NO_SUITABLE_CRF_MESSAGE.lower() in haystack.lower()


def _format_cmd_for_log(cmd: list[str], replacements: dict[str, str]) -> str:
    """Build the anonymized log form of a command by mapping known tokens.

    Deriving the log string from the real command (instead of maintaining a
    parallel list) keeps the two from drifting apart.
    """
    return " ".join(replacements.get(token, token) for token in cmd)


class AbAv1Wrapper:
    """Wrapper for the ab-av1 tool providing high-level encoding interface.

    This class handles execution of ab-av1, monitors progress via a parser,
    and manages VMAF-based encoding with automatic fallback.
    """

    def __init__(self):
        """Initialize the wrapper, find executable, prepare parser."""
        ab_av1_path = get_ab_av1_path()
        if ab_av1_path is None:
            error_msg = (
                f"{AB_AV1_EXE_NAME} not found.\nExpected: {AB_AV1_EXE}\n"
                "Install via 'cargo install ab-av1' or click 'Download' in Settings."
            )
            logger.error(error_msg)
            raise FileNotFoundError(error_msg)
        self.executable_path = str(ab_av1_path)
        logger.debug(f"AbAv1Wrapper init - using executable at: {self.executable_path}")
        self.parser = AbAv1Parser()
        self.file_info_callback = None

    # --- Shared helpers ---

    def _fail(self, input_path: str, message: str, error_type: str, **extra: Any) -> None:
        """Emit a "failed" file_info_callback if one is set."""
        if self.file_info_callback:
            info = {"message": message, "type": error_type}
            info.update(extra)
            self.file_info_callback(os.path.basename(input_path), "failed", info)

    def _validate_input(self, input_path: str) -> tuple[dict, int | None]:
        """Check the input exists and is a video; return (video_info, size or None).

        Raises:
            InputFileError: If the input is missing, unreadable, or has no video stream.
        """
        anonymized = anonymize_filename(input_path)
        if not os.path.exists(input_path):
            error_msg = f"Input not found: {anonymized}"
            logger.error(error_msg)
            self._fail(input_path, error_msg, "missing_input")
            raise InputFileError(error_msg, error_type="missing_input")

        try:
            video_info = get_video_info(input_path)
            if not video_info or "streams" not in video_info:
                raise InputFileError("Invalid video file", error_type="invalid_video")
            if not extract_video_metadata(video_info).has_video:
                raise InputFileError("No video stream", error_type="no_video_stream")
        except InputFileError as e:
            logger.error(f"{e.message} ({anonymized})")  # noqa: TRY400 - no traceback needed for expected validation failure
            self._fail(input_path, e.message, e.error_type or "invalid_video")
            raise
        except Exception as e:
            error_msg = f"Error analyzing {anonymized}"
            logger.exception(error_msg)
            self._fail(input_path, error_msg, "analysis_failed")
            raise InputFileError(error_msg, error_type="analysis_failed") from e

        try:
            original_size = os.path.getsize(input_path)
            logger.info(f"Original file size: {original_size} bytes ({format_file_size(original_size)})")
        except Exception as size_e:
            logger.warning(f"Couldn't get original file size: {size_e}")
            original_size = None
        return video_info, original_size

    def _prepare_output(self, input_path: str, output_path: str) -> tuple[str, str]:
        """Coerce the output to .mkv and create its directory; return (path, dir).

        Raises:
            OutputFileError: If the output directory cannot be created.
        """
        if not output_path.lower().endswith(".mkv"):
            output_path = os.path.splitext(output_path)[0] + ".mkv"
        output_dir = os.path.dirname(output_path)
        try:
            os.makedirs(output_dir, exist_ok=True)
        except Exception as e:
            error_msg = "Cannot create output dir"
            logger.exception(error_msg)
            self._fail(input_path, error_msg, "output_dir_creation_failed")
            raise OutputFileError(error_msg, error_type="output_dir_creation_failed") from e
        return output_path, output_dir

    @staticmethod
    def _process_env(verbose_ffmpeg: bool) -> dict[str, str]:
        """Build the subprocess environment: vendor FFmpeg on PATH, verbose RUST_LOG.

        verbose_ffmpeg adds per-frame ffmpeg trace output - needed to parse
        encoding progress, but pure noise for crf-search sample runs.
        """
        env = os.environ.copy()
        # If vendor FFmpeg exists, prepend it to PATH so ab-av1 finds it.
        # This only affects this subprocess - user's system PATH is unchanged.
        if FFMPEG_DIR.exists():
            env["PATH"] = str(FFMPEG_DIR) + os.pathsep + env.get("PATH", "")
            logger.debug(f"Using vendor FFmpeg from {FFMPEG_DIR}")
        env["RUST_LOG"] = "debug,ab_av1=trace,ffmpeg=trace" if verbose_ffmpeg else "debug,ab_av1=trace"
        return env

    def _run_once(
        self,
        cmd: list[str],
        *,
        input_path: str,
        cwd: str,
        env: dict[str, str],
        on_line: Callable[[str], None] | None,
        cancel_event: Any | None,
        pid_callback: Callable[..., Any] | None,
    ) -> ProcessResult:
        """Run one ab-av1 process, reporting spawn failures via callback.

        Raises:
            FileNotFoundError: If the executable is missing.
            AbAv1Error: If the process cannot be spawned for any other OS reason
                (e.g. corrupt or non-executable binary).
        """
        try:
            return run_ab_av1(
                cmd, cwd=cwd, env=env, on_line=on_line, cancel_event=cancel_event, pid_callback=pid_callback
            )
        except FileNotFoundError:
            error_msg = f"Executable not found: {self.executable_path}"
            logger.exception(error_msg)
            self._fail(input_path, error_msg, "executable_not_found")
            raise
        except OSError as e:
            error_msg = f"Failed to start ab-av1 process: {e}"
            logger.exception(error_msg)
            self._fail(input_path, error_msg, "process_spawn_failed")
            raise AbAv1Error(error_msg, error_type="process_spawn_failed") from e

    def _raise_for_failed_result(
        self, result: ProcessResult, *, input_path: str, cmd_str_log: str, cleanup_dir: str, failure_error_type: str
    ) -> NoReturn:
        """Clean temp folders and raise for a cancelled / hung / failed run.

        Raises:
            AbAv1CancelledError: The run was cancelled via cancel_event.
            AbAv1Error: Silence timeout, or any other failure (failure_error_type).
        """
        clean_ab_av1_temp_folders(cleanup_dir)

        if result.cancelled:
            logger.info(f"ab-av1 run cancelled for {anonymize_filename(input_path)}")
            raise AbAv1CancelledError("Cancelled by user", command=cmd_str_log, error_type="cancelled")

        if result.silence_timeout:
            error_msg = f"ab-av1 produced no output for too long and was terminated (rc={result.return_code})"
            logger.error(error_msg)
            self._fail(input_path, error_msg, "process_silent_timeout")
            raise AbAv1Error(error_msg, command=cmd_str_log, output=result.output, error_type="process_silent_timeout")

        detail = result.error_line or f"exit code {result.return_code}"
        error_msg = f"ab-av1 failed (rc={result.return_code}): {detail}"
        logger.error(error_msg)
        logger.error(f"Last Cmd: {cmd_str_log}")
        tail = result.output.splitlines()[-20:] if result.output else ["<No output captured>"]
        logger.error(f"Last Output tail ({len(tail)} lines):\n" + "\n".join(tail))
        self._fail(input_path, error_msg, failure_error_type, details=detail, command=cmd_str_log)
        raise AbAv1Error(error_msg, command=cmd_str_log, output=result.output, error_type=failure_error_type)

    def _verify_output_exists(self, output_path: str, *, input_path: str, cmd_str_log: str) -> None:
        """Raise OutputFileError if a run reported success but produced no file."""
        if os.path.exists(output_path):
            return
        error_msg = f"ab-av1 reported success (rc=0) but output file is missing: {anonymize_filename(output_path)}"
        logger.error(error_msg)
        self._fail(input_path, error_msg, "missing_output_on_success")
        raise OutputFileError(error_msg, command=cmd_str_log, error_type="missing_output_on_success")

    def _run_with_vmaf_fallback(
        self,
        *,
        input_path: str,
        initial_target: int,
        make_cmd: Callable[[int], tuple[list[str], str]],
        cwd: str,
        on_attempt_start: Callable[[int, str], None],
        on_line: Callable[[str], None] | None,
        cancel_event: Any | None,
        pid_callback: Callable[..., Any] | None,
        original_size: int | None,
        verbose_ffmpeg: bool,
    ) -> tuple[ProcessResult, int, str]:
        """Run ab-av1, decrementing the VMAF target on "no suitable crf" failures.

        Args:
            make_cmd: Builds (real command, anonymized command string) for a target.
            cwd: Working directory for the process; ab-av1 temp folders appear
                (and are cleaned) there.
            on_attempt_start: Called with (target, anonymized command) before each attempt.
            verbose_ffmpeg: See _process_env.

        Returns:
            (successful ProcessResult, VMAF target used, anonymized command string).

        Raises:
            AbAv1CancelledError: The run was cancelled via cancel_event.
            ConversionNotWorthwhileError: No suitable CRF even at the minimum target.
            AbAv1Error: Silence timeout or any other non-recoverable failure.
        """
        env = self._process_env(verbose_ffmpeg)
        anonymized_input = anonymize_filename(input_path)
        target = initial_target

        while True:
            # Don't spawn a doomed process for the next fallback step after the
            # user already requested cancellation.
            if cancel_event is not None and cancel_event.is_set():
                clean_ab_av1_temp_folders(cwd)
                logger.info(f"ab-av1 run cancelled before VMAF {target} attempt for {anonymized_input}")
                raise AbAv1CancelledError("Cancelled by user", error_type="cancelled")

            cmd, cmd_str_log = make_cmd(target)
            logger.info(f"[Attempt VMAF {target}] Running: {cmd_str_log}")
            on_attempt_start(target, cmd_str_log)

            result = self._run_once(
                cmd,
                input_path=input_path,
                cwd=cwd,
                env=env,
                on_line=on_line,
                cancel_event=cancel_event,
                pid_callback=pid_callback,
            )

            if result.cancelled or result.silence_timeout:
                self._raise_for_failed_result(
                    result,
                    input_path=input_path,
                    cmd_str_log=cmd_str_log,
                    cleanup_dir=cwd,
                    failure_error_type="ab_av1_failed",
                )

            if result.return_code == 0:
                logger.info(f"ab-av1 succeeded for {anonymized_input} at VMAF target {target}")
                return result, target, cmd_str_log

            if _is_no_suitable_crf(result):
                clean_ab_av1_temp_folders(cwd)
                next_target = target - VMAF_FALLBACK_STEP
                if next_target >= MIN_VMAF_FALLBACK_TARGET:
                    logger.info(f"No suitable CRF at VMAF {target}; retrying {anonymized_input} at {next_target}")
                    target = next_target
                    continue
                error_msg = (
                    f"No efficient conversion possible - CRF search failed even at VMAF {MIN_VMAF_FALLBACK_TARGET}"
                )
                logger.info(f"File not worth converting: {anonymized_input}")
                if self.file_info_callback:
                    self.file_info_callback(
                        os.path.basename(input_path),
                        "skipped_not_worth",
                        {
                            "message": error_msg,
                            "original_size": original_size,
                            "min_vmaf_attempted": MIN_VMAF_FALLBACK_TARGET,
                        },
                    )
                raise ConversionNotWorthwhileError(
                    error_msg, command=cmd_str_log, output=result.output, original_size=original_size
                )

            # Non-recoverable failure - no retry
            self._raise_for_failed_result(
                result,
                input_path=input_path,
                cmd_str_log=cmd_str_log,
                cleanup_dir=cwd,
                failure_error_type="ab_av1_failed",
            )

    # --- Public operations ---

    def auto_encode(
        self,
        input_path: str,
        output_path: str,
        file_info_callback: Callable[..., Any] | None = None,
        pid_callback: Callable[..., Any] | None = None,
        total_duration_seconds: float = 0.0,
        hw_decoder: str | None = None,
        cancel_event: Any | None = None,
    ) -> EncodeStats:
        """Run ab-av1 auto-encode (CRF search + encode) with VMAF fallback.

        Args:
            input_path: Path to the input video file.
            output_path: Path where the output file should be saved.
            file_info_callback: Optional callback for reporting file status changes.
            pid_callback: Optional callback to receive the process ID.
            total_duration_seconds: Total duration of the input video in seconds.
            hw_decoder: Optional hardware decoder name (e.g., "h264_cuvid", "hevc_qsv").
            cancel_event: Optional threading.Event; when set, the run is aborted
                mid-process and AbAv1CancelledError is raised.

        Returns:
            EncodeStats with final statistics and timing breakdown.

        Raises:
            InputFileError, OutputFileError, AbAv1CancelledError,
            ConversionNotWorthwhileError, AbAv1Error
        """
        self.file_info_callback = file_info_callback
        self.parser.file_info_callback = file_info_callback
        preset = DEFAULT_ENCODING_PRESET
        initial_target = DEFAULT_VMAF_TARGET

        process_start_time = time.time()
        anonymized_input_path = anonymize_filename(input_path)

        _video_info, original_size = self._validate_input(input_path)
        output_path, output_dir = self._prepare_output(input_path, output_path)
        anonymized_output_path = anonymize_filename(output_path)

        log_interval = get_log_interval_for_duration(total_duration_seconds)
        if log_interval:
            logger.info(f"Using log interval: {log_interval}")

        stats = EncodeStats(
            input_path=input_path,
            output_path=output_path,
            original_size=original_size,
            total_duration_seconds=total_duration_seconds,
            vmaf_target_used=initial_target,
        )

        log_replacements = {
            self.executable_path: os.path.basename(self.executable_path),
            input_path: os.path.basename(anonymized_input_path),
            output_path: os.path.basename(anonymized_output_path),
        }

        def make_cmd(target: int) -> tuple[list[str], str]:
            cmd = [
                self.executable_path,
                "auto-encode",
                "-i",
                input_path,
                "-o",
                output_path,
                "--preset",
                str(preset),
                "--min-vmaf",
                str(target),
            ]
            if hw_decoder:
                cmd.extend(["--enc-input", f"c:v={hw_decoder}"])
            if log_interval:
                cmd.extend(["--log-interval", log_interval])
            return cmd, _format_cmd_for_log(cmd, log_replacements)

        def on_attempt_start(target: int, cmd_str_log: str) -> None:
            stats.reset_for_attempt(target)
            stats.command = cmd_str_log
            if self.file_info_callback:
                callback_info = {
                    "message": "",
                    "original_vmaf": initial_target,
                    "fallback_vmaf": target,
                    "used_fallback": target != initial_target,
                    "vmaf_target_used": target,
                    "original_size": original_size,
                }
                if target != initial_target:
                    callback_info["message"] = f"Retrying with VMAF target: {target}"
                    self.file_info_callback(os.path.basename(input_path), "retrying", callback_info)
                else:
                    status = "starting" if original_size is not None else "starting_no_size"
                    self.file_info_callback(os.path.basename(input_path), status, callback_info)

        encoding_phase_start: list[float | None] = [None]

        def on_line(line: str) -> None:
            self.parser.parse_line(line, stats)
            if encoding_phase_start[0] is None and stats.phase == "encoding":
                encoding_phase_start[0] = time.time()

        result, target_used, cmd_str_log = self._run_with_vmaf_fallback(
            input_path=input_path,
            initial_target=initial_target,
            make_cmd=make_cmd,
            cwd=output_dir,
            on_attempt_start=on_attempt_start,
            on_line=on_line,
            cancel_event=cancel_event,
            pid_callback=pid_callback,
            original_size=original_size,
            verbose_ffmpeg=True,
        )

        # --- Success Path ---
        self._verify_output_exists(output_path, input_path=input_path, cmd_str_log=cmd_str_log)

        logger.info(f"ab-av1 completed successfully for {anonymized_input_path} (used VMAF target {target_used})")
        self.parser.parse_final_output(result.output, stats)

        # --- Calculate timing breakdown ---
        now = time.time()
        if encoding_phase_start[0] is not None:
            stats.crf_search_time_sec = encoding_phase_start[0] - process_start_time
            stats.encoding_time_sec = now - encoding_phase_start[0]
        else:
            # Fallback: couldn't detect phase transition (shouldn't happen normally)
            stats.crf_search_time_sec = now - process_start_time
            stats.encoding_time_sec = 0.0

        # --- Logging Final Stats ---
        if stats.crf is not None:
            logger.info(f"Final CRF: {format_crf(stats.crf)}")
        if stats.vmaf is not None:
            logger.info(f"Final VMAF: {stats.vmaf:.2f}")
        if stats.size_reduction is not None:
            logger.info(f"Final Size reduction: {stats.size_reduction:.2f}%")
        else:
            logger.warning("Final size reduction could not be determined from parsing.")

        # --- Completion Callback ---
        if self.file_info_callback:
            vmaf_text = f"{stats.vmaf:.2f}" if stats.vmaf is not None else "N/A"
            final_stats_for_callback = {
                "message": f"Complete (VMAF {vmaf_text} @ Target {target_used})",
                "vmaf": stats.vmaf,
                "crf": stats.crf,
                "vmaf_target_used": stats.vmaf_target_used,
                "size_reduction": stats.size_reduction,
                "output_path": output_path,
            }
            try:
                final_size = os.path.getsize(output_path)
                stats.output_size = final_size
                final_stats_for_callback["output_size"] = final_size
                logger.info(f"Final output size: {final_size} bytes ({format_file_size(final_size)})")
            except Exception as size_e:
                logger.warning(f"Could not get final output size for callback: {size_e}")

            self.file_info_callback(os.path.basename(input_path), "completed", final_stats_for_callback)

        # --- Temp Folder Cleanup (Final Check) ---
        cleaned_count = clean_ab_av1_temp_folders(output_dir)
        if cleaned_count > 0:
            logger.info(f"Cleaned {cleaned_count} leftover temporary folder(s) in {output_dir}.")

        # --- Clear Callbacks ---
        self.file_info_callback = None
        self.parser.file_info_callback = None
        return stats

    def crf_search(
        self,
        input_path: str,
        vmaf_target: int | None = None,
        preset: int | None = None,
        progress_callback: Callable[..., Any] | None = None,
        stop_event: Any | None = None,
        hw_decoder: str | None = None,
        pid_callback: Callable[..., Any] | None = None,
    ) -> CrfSearchResult:
        """Run ab-av1 crf-search with VMAF fallback (no full encoding).

        This performs VMAF-targeted CRF search by sampling the video at multiple
        CRF values to find the optimal quality setting. Does NOT encode the full video.

        Args:
            input_path: Path to the input video file.
            vmaf_target: Target VMAF score (default: DEFAULT_VMAF_TARGET).
            preset: SVT-AV1 encoding preset (default: DEFAULT_ENCODING_PRESET).
            progress_callback: Optional callback for progress updates.
                Signature: callback(progress_percent, message)
            stop_event: Optional threading.Event to signal cancellation (aborts mid-run).
            hw_decoder: Optional hardware decoder name (e.g., "h264_cuvid", "hevc_qsv").
            pid_callback: Optional callback to receive the process ID (for force-stop).

        Returns:
            CrfSearchResult with the optimal CRF, achieved VMAF, and predictions.

        Raises:
            InputFileError: If input file is missing or invalid
            AbAv1CancelledError: If the search was cancelled via stop_event
            ConversionNotWorthwhileError: If CRF search fails at all VMAF targets down to minimum
            AbAv1Error: For other ab-av1 execution errors
        """
        # No file_info_callback for analysis runs - progress flows through progress_callback
        self.file_info_callback = None
        self.parser.file_info_callback = None

        if vmaf_target is None:
            vmaf_target = DEFAULT_VMAF_TARGET
        if preset is None:
            preset = DEFAULT_ENCODING_PRESET

        crf_search_start_time = time.time()
        initial_target = vmaf_target
        anonymized_input_path = anonymize_filename(input_path)

        video_info, original_size = self._validate_input(input_path)

        # Use input file's directory as cwd so temp folders are created (and cleaned) there
        input_dir = os.path.dirname(input_path) or os.getcwd()

        stats = EncodeStats(input_path=input_path, original_size=original_size, vmaf_target_used=initial_target)

        log_replacements = {
            self.executable_path: os.path.basename(self.executable_path),
            input_path: os.path.basename(anonymized_input_path),
        }

        def make_cmd(target: int) -> tuple[list[str], str]:
            cmd = [
                self.executable_path,
                "crf-search",
                "-i",
                input_path,
                "--preset",
                str(preset),
                "--min-vmaf",
                str(target),
            ]
            if hw_decoder:
                cmd.extend(["--enc-input", f"c:v={hw_decoder}"])
            return cmd, _format_cmd_for_log(cmd, log_replacements)

        def on_attempt_start(target: int, cmd_str_log: str) -> None:
            stats.reset_for_attempt(target)
            stats.command = cmd_str_log

        # Only forward changed values: on_line fires for every output line, and each
        # forwarded update ends up as a Tkinter root.after post on the main thread.
        last_sent: list[tuple[float, float | None, float | None] | None] = [None]

        def on_line(line: str) -> None:
            self.parser.parse_line(line, stats)
            if not (progress_callback and stats.progress_quality):
                return
            snapshot = (stats.progress_quality, stats.crf, stats.vmaf)
            if snapshot == last_sent[0]:
                return
            last_sent[0] = snapshot
            target = stats.vmaf_target_used
            suffix = f" (target: {target})" if target != initial_target else ""
            vmaf_text = stats.vmaf if stats.vmaf is not None else "?"
            message = f"CRF:{format_crf(stats.crf)}, VMAF:{vmaf_text}{suffix}"
            progress_callback(stats.progress_quality, message)

        result, target_used, cmd_str_log = self._run_with_vmaf_fallback(
            input_path=input_path,
            initial_target=initial_target,
            make_cmd=make_cmd,
            cwd=input_dir,
            on_attempt_start=on_attempt_start,
            on_line=on_line,
            cancel_event=stop_event,
            pid_callback=pid_callback,
            original_size=original_size,
            verbose_ffmpeg=False,
        )

        # --- Parse Final Results ---
        self.parser.parse_final_output(result.output, stats)

        if stats.crf is None or stats.vmaf is None:
            error_msg = "CRF search completed but could not parse results"
            logger.error(error_msg)
            logger.error(f"Output:\n{result.output[-1000:]}")
            raise AbAv1Error(error_msg, command=cmd_str_log, output=result.output, error_type="parse_error")

        # Calculate predicted output size (accounting for audio which is copied unchanged)
        predicted_output_size = None
        if original_size and stats.size_reduction:
            # ab-av1's size_reduction is video-only; audio is copied unchanged
            meta = extract_video_metadata(video_info)
            audio_size_bytes = 0
            if meta.duration_sec and meta.total_audio_bitrate_kbps:
                audio_size_bytes = int(meta.duration_sec * meta.total_audio_bitrate_kbps * 1000 / 8)

            # Apply reduction only to video portion
            video_size = max(0, original_size - audio_size_bytes)
            predicted_video = int(video_size * (1 - stats.size_reduction / 100))
            predicted_output_size = predicted_video + audio_size_bytes

        search_result = CrfSearchResult(
            best_crf=stats.crf,
            best_vmaf=stats.vmaf,
            predicted_size_reduction=stats.size_reduction,
            predicted_output_size=predicted_output_size,
            vmaf_target_used=target_used,
            original_size=original_size,
            used_fallback=target_used != initial_target,
            preset_used=preset,
            crf_search_time_sec=time.time() - crf_search_start_time,
        )

        reduction_text = (
            f"{search_result.predicted_size_reduction}" if search_result.predicted_size_reduction is not None else "N/A"
        )
        logger.info(
            f"CRF search complete: CRF={format_crf(search_result.best_crf)}, "
            f"VMAF={search_result.best_vmaf:.2f}, "
            f"Reduction={reduction_text}%, "
            f"Target={target_used}"
            f"{' (fallback)' if search_result.used_fallback else ''}"
        )

        clean_ab_av1_temp_folders(input_dir)
        return search_result

    def encode_with_crf(
        self,
        input_path: str,
        output_path: str,
        crf: float,
        preset: int | None = None,
        file_info_callback: Callable[..., Any] | None = None,
        pid_callback: Callable[..., Any] | None = None,
        total_duration_seconds: float = 0.0,
        hw_decoder: str | None = None,
        cancel_event: Any | None = None,
    ) -> EncodeStats:
        """Run ab-av1 encode with explicit CRF (skip CRF search phase).

        Use this when you already know the optimal CRF from previous quality analysis.

        Args:
            input_path: Path to the input video file.
            output_path: Path where the output file should be saved.
            crf: Explicit CRF value to use for encoding.
            preset: SVT-AV1 encoding preset (default: DEFAULT_ENCODING_PRESET).
            file_info_callback: Optional callback for reporting file status changes.
            pid_callback: Optional callback to receive the process ID.
            total_duration_seconds: Total duration of the input video in seconds.
            hw_decoder: Optional hardware decoder name (e.g., "h264_cuvid", "hevc_qsv").
            cancel_event: Optional threading.Event; when set, the run is aborted
                mid-process and AbAv1CancelledError is raised.

        Returns:
            EncodeStats with final statistics and timing breakdown.

        Raises:
            InputFileError, OutputFileError, AbAv1CancelledError, AbAv1Error
        """
        self.file_info_callback = file_info_callback
        self.parser.file_info_callback = file_info_callback

        if preset is None:
            preset = DEFAULT_ENCODING_PRESET

        encoding_start_time = time.time()
        anonymized_input_path = anonymize_filename(input_path)

        _video_info, original_size = self._validate_input(input_path)
        output_path, output_dir = self._prepare_output(input_path, output_path)
        anonymized_output_path = anonymize_filename(output_path)

        # --- Command Preparation ---
        cmd = [
            self.executable_path,
            "encode",
            "-i",
            input_path,
            "-o",
            output_path,
            "--preset",
            str(preset),
            "--crf",
            format_crf(crf),
        ]
        if hw_decoder:
            cmd.extend(["--enc-input", f"c:v={hw_decoder}"])
        log_interval = get_log_interval_for_duration(total_duration_seconds)
        if log_interval:
            cmd.extend(["--log-interval", log_interval])
            logger.info(f"Using log interval: {log_interval}")

        cmd_str_log = _format_cmd_for_log(
            cmd,
            {
                self.executable_path: os.path.basename(self.executable_path),
                input_path: os.path.basename(anonymized_input_path),
                output_path: os.path.basename(anonymized_output_path),
            },
        )
        logger.info(f"Running encode with cached CRF: {cmd_str_log}")

        stats = EncodeStats(
            input_path=input_path,
            output_path=output_path,
            command=cmd_str_log,
            phase="encoding",
            progress_quality=100.0,  # CRF search already done
            crf=crf,
            original_size=original_size,
            total_duration_seconds=total_duration_seconds,
            used_cached_crf=True,
        )

        # --- Starting Callback ---
        if self.file_info_callback:
            callback_info = {
                "message": f"Encoding with cached CRF {format_crf(crf)}",
                "crf": crf,
                "original_size": original_size,
                "used_cached_crf": True,
            }
            self.file_info_callback(os.path.basename(input_path), "starting", callback_info)

        result = self._run_once(
            cmd,
            input_path=input_path,
            cwd=output_dir,
            env=self._process_env(verbose_ffmpeg=True),
            on_line=lambda line: self.parser.parse_line(line, stats),
            cancel_event=cancel_event,
            pid_callback=pid_callback,
        )

        # --- Check Result ---
        if result.cancelled or result.silence_timeout or result.return_code != 0:
            self._raise_for_failed_result(
                result,
                input_path=input_path,
                cmd_str_log=cmd_str_log,
                cleanup_dir=output_dir,
                failure_error_type="encoding_failed",
            )

        # --- Verify Output ---
        self._verify_output_exists(output_path, input_path=input_path, cmd_str_log=cmd_str_log)

        # --- Success ---
        logger.info(f"Encode completed successfully for {anonymized_input_path} (CRF {format_crf(crf)})")
        self.parser.parse_final_output(result.output, stats)
        stats.crf = crf  # Ensure CRF is set

        # Add timing (no CRF search - using cached CRF)
        stats.crf_search_time_sec = 0.0
        stats.encoding_time_sec = time.time() - encoding_start_time

        # Record output size and actual size reduction
        try:
            stats.output_size = os.path.getsize(output_path)
            if original_size:
                stats.size_reduction = ((original_size - stats.output_size) / original_size) * 100
                logger.info(f"Actual size reduction: {stats.size_reduction:.2f}%")
        except Exception as e:
            logger.warning(f"Could not calculate size reduction: {e}")

        # --- Completion Callback ---
        if self.file_info_callback:
            final_stats_for_callback = {
                "message": f"Complete (CRF {format_crf(crf)}, cached)",
                "crf": crf,
                "size_reduction": stats.size_reduction,
                "output_path": output_path,
                "used_cached_crf": True,
            }
            if stats.output_size is not None:
                final_stats_for_callback["output_size"] = stats.output_size

            self.file_info_callback(os.path.basename(input_path), "completed", final_stats_for_callback)

        # --- Cleanup ---
        cleaned_count = clean_ab_av1_temp_folders(output_dir)
        if cleaned_count > 0:
            logger.info(f"Cleaned {cleaned_count} leftover temporary folder(s)")

        self.file_info_callback = None
        self.parser.file_info_callback = None
        return stats
