# src/ab_av1/wrapper.py
"""
Wrapper class for the ab-av1 tool in the AV1 Video Converter application.

Handles executing ab-av1, managing the process, VMAF fallback,
and coordinating with the parser for output analysis.
Uses a simple blocking read loop for stdout/stderr.
Uses RUST_LOG for detailed ffmpeg progress output.
"""

import logging
import os
import re
import subprocess
import time
from typing import Any, Callable

# Project imports
from src.config import DEFAULT_ENCODING_PRESET, DEFAULT_VMAF_TARGET, MIN_VMAF_FALLBACK_TARGET, VMAF_FALLBACK_STEP
from src.utils import anonymize_filename, format_file_size, get_video_info, get_windows_subprocess_startupinfo

from .cleaner import clean_ab_av1_temp_folders

# Import exceptions, cleaner, parser from this package
from .exceptions import (
    AbAv1Error,
    ConversionNotWorthwhileError,
    EncodingError,
    InputFileError,
    OutputFileError,
    VMAFError,
)
from .parser import AbAv1Parser

logger = logging.getLogger(__name__)


class AbAv1Wrapper:
    """Wrapper for the ab-av1 tool providing high-level encoding interface.

    This class handles execution of ab-av1.exe, monitors progress via a parser,
    and manages VMAF-based encoding with automatic fallback.
    """

    def __init__(self):
        """Initialize the wrapper, find executable, prepare parser."""
        script_dir = os.path.dirname(os.path.abspath(__file__))  # src/ab_av1/
        src_dir = os.path.dirname(script_dir)  # src/
        self.executable_path = os.path.abspath(os.path.join(src_dir, "ab-av1.exe"))
        logger.debug(f"AbAv1Wrapper init - expecting executable at: {self.executable_path}")
        self._verify_executable()
        self.parser = AbAv1Parser()
        self.file_info_callback = None

    def _verify_executable(self) -> bool:
        """Verify that the ab-av1 executable exists at the expected location."""
        if not os.path.exists(self.executable_path):
            error_msg = f"ab-av1.exe not found. Place inside 'src' dir.\nExpected: {self.executable_path}"
            logger.error(error_msg)
            raise FileNotFoundError(error_msg)
        logger.debug(f"AbAv1Wrapper init - verified: {self.executable_path}")
        return True

    def auto_encode(
        self,
        input_path: str,
        output_path: str,
        file_info_callback: Callable[..., Any] | None = None,
        pid_callback: Callable[..., Any] | None = None,
        total_duration_seconds: float = 0.0,
    ) -> dict[str, Any]:
        """Run ab-av1 auto-encode with VMAF fallback loop using simple blocking reads.

        Args:
            input_path: Path to the input video file.
            output_path: Path where the output file should be saved.
            file_info_callback: Optional callback for reporting file status changes.
            pid_callback: Optional callback to receive the process ID.
            total_duration_seconds: Total duration of the input video in seconds.

        Returns:
            Dictionary containing encoding statistics and results.

        Raises:
            InputFileError, OutputFileError, VMAFError, EncodingError, AbAv1Error
        """
        self.file_info_callback = file_info_callback
        self.parser.file_info_callback = file_info_callback  # Ensure parser has callback
        preset = DEFAULT_ENCODING_PRESET
        initial_min_vmaf = DEFAULT_VMAF_TARGET

        anonymized_input_path = anonymize_filename(input_path)

        # --- Input Validation ---
        if not os.path.exists(input_path):
            error_msg = f"Input not found: {anonymized_input_path}"
            logger.error(error_msg)
            if self.file_info_callback:
                self.file_info_callback(
                    os.path.basename(input_path), "failed", {"message": error_msg, "type": "missing_input"}
                )
            raise InputFileError(error_msg, error_type="missing_input")

        try:
            video_info = get_video_info(input_path)
            if not video_info or "streams" not in video_info:
                raise InputFileError("Invalid video file", error_type="invalid_video")

            if not any(s.get("codec_type") == "video" for s in video_info.get("streams", [])):
                raise InputFileError("No video stream", error_type="no_video_stream")

            try:
                original_size = os.path.getsize(input_path)
                stats = {"original_size": original_size}
                logger.info(f"Original file size: {original_size} bytes ({format_file_size(original_size)})")
            except Exception as size_e:
                logger.warning(f"Couldn't get original file size: {size_e}")
                stats = {}
        except AbAv1Error:
            raise
        except Exception as e:
            error_msg = f"Error analyzing {anonymized_input_path}"
            logger.exception(error_msg)
            if self.file_info_callback:
                self.file_info_callback(
                    os.path.basename(input_path), "failed", {"message": error_msg, "type": "analysis_failed"}
                )
            raise InputFileError(error_msg, error_type="analysis_failed") from e

        # --- Output Path Setup ---
        if not output_path.lower().endswith(".mkv"):
            output_path = os.path.splitext(output_path)[0] + ".mkv"

        output_dir = os.path.dirname(output_path)
        try:
            os.makedirs(output_dir, exist_ok=True)
        except Exception as e:
            error_msg = "Cannot create output dir"
            logger.exception(error_msg)
            if self.file_info_callback:
                self.file_info_callback(
                    os.path.basename(input_path), "failed", {"message": error_msg, "type": "output_dir_creation_failed"}
                )
            raise OutputFileError(error_msg, error_type="output_dir_creation_failed") from e

        anonymized_output_path = anonymize_filename(output_path)

        # --- VMAF Fallback Loop ---
        current_vmaf_target = initial_min_vmaf
        last_error_info = None
        success = False

        if not stats:
            stats = {}

        stats.update(
            {
                "phase": "crf-search",
                "progress_quality": 0,
                "progress_encoding": 0,
                "vmaf": None,
                "crf": None,
                "size_reduction": None,
                "input_path": input_path,
                "output_path": output_path,
                "command": "",
                "vmaf_target_used": current_vmaf_target,
                "last_ffmpeg_fps": None,
                "eta_text": None,
                "total_duration_seconds": total_duration_seconds,
                "last_reported_encoding_progress": -1.0,
                "estimated_output_size": None,
                "estimated_size_reduction": None,
            }
        )

        while current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET:
            # Reset stats for this attempt
            stats["phase"] = "crf-search"
            stats["progress_quality"] = 0
            stats["progress_encoding"] = 0
            stats["vmaf"] = None
            stats["crf"] = None
            stats["last_reported_encoding_progress"] = -1.0
            stats["vmaf_target_used"] = current_vmaf_target
            stats["estimated_output_size"] = None
            stats["estimated_size_reduction"] = None
            stats["eta_text"] = None
            stats["last_ffmpeg_fps"] = None

            full_output_lines = []  # Reset collected output lines

            logger.info(f"[Attempt VMAF {current_vmaf_target}] Starting for {anonymized_input_path}")

            # --- Command Preparation ---
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
                str(current_vmaf_target),
            ]
            cmd_str = " ".join(cmd)
            stats["command"] = cmd_str

            cmd_for_log = [
                os.path.basename(self.executable_path),
                "auto-encode",
                "-i",
                os.path.basename(anonymized_input_path),
                "-o",
                os.path.basename(anonymized_output_path),
                "--preset",
                str(preset),
                "--min-vmaf",
                str(current_vmaf_target),
            ]
            cmd_str_log = " ".join(cmd_for_log)
            logger.debug(f"Running: {cmd_str_log}")

            # --- Environment Setup with maximum verbosity and pass-through flags ---
            process_env = os.environ.copy()
            # Critical environment variables for ffmpeg output
            process_env["RUST_LOG"] = "debug,ab_av1=trace,ffmpeg=trace"  # Filter out trace from other components
            process_env["AV1_PRINT_FFMPEG"] = "1"  # Force printing of ffmpeg output
            process_env["AV1_RAW_OUTPUT"] = "1"  # Pass through raw output
            process_env["FFMPEG_PROGRESS"] = "1"  # Try to enable any ffmpeg progress features
            process_env["SVT_VERBOSE"] = "1"  # Try to enable SVT-AV1 verbosity
            process_env["AB_AV1_VERBOSE"] = "1"  # Enable any ab-av1 verbosity
            process_env["AB_AV1_LOG_PROGRESS"] = "1"  # Try to enable any progress logging
            logger.info("Set targeted environment variable verbosity for ab-av1 and ffmpeg tools")

            # --- Starting/Retrying Callback ---
            if self.file_info_callback:
                callback_info = {
                    "message": "",
                    "original_vmaf": initial_min_vmaf,
                    "fallback_vmaf": current_vmaf_target,
                    "used_fallback": current_vmaf_target != initial_min_vmaf,
                    "vmaf_target_used": current_vmaf_target,
                    "original_size": stats.get("original_size"),
                }

                if current_vmaf_target != initial_min_vmaf:
                    callback_info["message"] = f"Retrying with VMAF target: {current_vmaf_target}"
                    self.file_info_callback(os.path.basename(input_path), "retrying", callback_info)
                else:
                    status = "starting" if stats.get("original_size") is not None else "starting_no_size"
                    self.file_info_callback(os.path.basename(input_path), status, callback_info)

            # --- Process Execution & Simple Blocking Read ---
            process = None
            return_code = -1
            full_output_text = ""
            read_loop_exception = None
            pipe_closed_prematurely = False  # Flag for pipe closure check

            try:
                startupinfo, creationflags = get_windows_subprocess_startupinfo()

                # Start the process, redirect stderr to stdout
                process = subprocess.Popen(
                    cmd,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    universal_newlines=True,
                    bufsize=1,
                    cwd=output_dir,
                    startupinfo=startupinfo,
                    creationflags=creationflags,
                    encoding="utf-8",
                    errors="replace",
                    env=process_env,
                )

                # Send PID to callback if provided
                if pid_callback:
                    pid_callback(process.pid)

                # Simple blocking readline loop
                logger.info(f"ab-av1 process {process.pid} started. Reading output...")
                current_output_lines = []

                assert process.stdout is not None  # Guaranteed by stdout=PIPE  # noqa: S101
                try:
                    for raw_line in iter(process.stdout.readline, ""):
                        line = raw_line.strip()
                        if not line:
                            continue

                        # Filter out sled::pagecache trace messages (internal Rust crate noise)
                        if "sled::pagecache" in line:
                            continue

                        current_output_lines.append(line + "\n")
                        stats = self.parser.parse_line(line, stats)  # Process every line
                except Exception as loop_err:
                    read_loop_exception = loop_err
                    logger.error(f"Exception in read loop: {loop_err}", exc_info=True)

                # --- Check process status after pipe reading completes ---
                final_poll_code = process.poll()
                logger.info(f"Status check after pipe reading: process.poll() returned {final_poll_code}")

                # Close file handles (stderr is None when using STDOUT redirect)
                try:
                    if process.stdout:
                        process.stdout.close()
                except Exception as e:
                    logger.warning(f"Error closing process stdout pipe: {e}")

                # Get the final return code
                if final_poll_code is None:
                    # Process is still running, we need to wait for it to finish
                    logger.warning("Process is still running after pipe reading completed")
                    pipe_closed_prematurely = True
                    try:
                        return_code = process.wait(timeout=30)  # Wait up to 30 seconds
                        logger.info(f"Process completed with return code {return_code}")
                    except subprocess.TimeoutExpired:
                        logger.exception("Process did not complete within timeout period, forcing termination")
                        process.terminate()
                        time.sleep(1)
                        if process.poll() is None:
                            process.kill()
                        return_code = -1
                else:
                    # Process already finished
                    return_code = final_poll_code

                logger.info(f"Process final return code: {return_code}")
                full_output_text = "".join(current_output_lines)

                if read_loop_exception:
                    # Raise errors after the process has exited
                    raise AbAv1Error(
                        f"Exception during pipe read: {read_loop_exception}",
                        command=cmd_str,
                        output=full_output_text,
                        error_type="pipe_read_error",
                    ) from read_loop_exception

            except FileNotFoundError as e:
                error_msg = f"Executable not found: {self.executable_path}"
                logger.exception(error_msg)
                if self.file_info_callback:
                    self.file_info_callback(
                        os.path.basename(input_path), "failed", {"message": error_msg, "type": "executable_not_found"}
                    )
                raise FileNotFoundError(error_msg) from e

            except Exception as e:
                # Catch exceptions from Popen or the loop exception re-raised above
                error_msg = f"Failed to start or manage process during VMAF {current_vmaf_target}"
                logger.exception(error_msg)

                if self.file_info_callback:
                    self.file_info_callback(
                        os.path.basename(input_path),
                        "failed",
                        {"message": error_msg, "type": "process_management_failed"},
                    )

                # Try to clean up any runaway process
                if process and process.poll() is None:
                    try:
                        logger.warning("Terminating/Killing runaway process due to error.")
                        process.terminate()
                        time.sleep(0.2)
                        process.kill()
                    except Exception as e:
                        logger.debug(f"Error terminating process: {e}")

                # Prepare output for error reporting
                full_output_text = "".join(current_output_lines if "current_output_lines" in locals() else [])
                return_code = process.poll() if process else -1

                # Re-raise appropriate exception
                if not isinstance(e, (AbAv1Error, FileNotFoundError)):
                    # Avoid re-wrapping known types
                    raise AbAv1Error(
                        error_msg, command=cmd_str, output=full_output_text, error_type="process_management_failed"
                    ) from e
                raise  # Re-raise the original known exception

            # --- Check Result ---
            logger.debug(f"Checking result for VMAF {current_vmaf_target}. RC={return_code}")
            if return_code == 0:
                if not os.path.exists(output_path):
                    error_msg = f"ab-av1 reported success (rc=0) but output file is missing: {anonymized_output_path}"
                    logger.error(error_msg)
                    last_error_info = {
                        "return_code": return_code,
                        "error_type": "missing_output_on_success",
                        "error_details": error_msg,
                        "command": cmd_str_log,
                        "output_tail": full_output_text.splitlines()[-20:],
                    }
                else:
                    success = True
                    logger.info(
                        f"Encode succeeded (rc=0) for {anonymized_input_path} with VMAF target {current_vmaf_target}"
                    )
                    break
            else:
                # --- Error Handling and Fallback Logic ---
                error_type = "unknown"
                error_details = "Unknown error"
                logger.debug("Analyzing failed run output...")
                logger.debug(f"Output Text for Analysis (last 1000 chars):\n'''\n{full_output_text[-1000:]}\n'''")

                # Analyze error by looking for common patterns in the output
                if re.search(r"Failed\s+to\s+find\s+a\s+suitable\s+crf", full_output_text, re.IGNORECASE):
                    logger.info("Detected 'Failed to find a suitable crf' error.")
                    error_type = "crf_search_failed"
                    error_details = f"Could not find suitable CRF for VMAF {current_vmaf_target}"

                    # Check if this is the last attempt (minimum VMAF reached)
                    if current_vmaf_target <= MIN_VMAF_FALLBACK_TARGET:
                        # This means conversion isn't worthwhile
                        error_msg = (
                            f"No efficient conversion possible - CRF search failed even at "
                            f"VMAF {MIN_VMAF_FALLBACK_TARGET}"
                        )
                        logger.info(f"File not worth converting: {anonymized_input_path}")

                        if self.file_info_callback:
                            self.file_info_callback(
                                os.path.basename(input_path),
                                "skipped_not_worth",
                                {
                                    "message": error_msg,
                                    "original_size": stats.get("original_size"),
                                    "min_vmaf_attempted": MIN_VMAF_FALLBACK_TARGET,
                                },
                            )

                        # Raise specific exception for this case
                        raise ConversionNotWorthwhileError(
                            error_msg,
                            command=cmd_str_log,
                            output=full_output_text,
                            original_size=stats.get("original_size"),
                        )
                elif re.search(r"ffmpeg.*?:\s*Invalid\s+data\s+found", full_output_text, re.IGNORECASE):
                    error_type = "invalid_input_data"
                    error_details = "Invalid data in input"
                elif re.search(r"No\s+such\s+file\s+or\s+directory", full_output_text, re.IGNORECASE):
                    error_type = "file_not_found"
                    error_details = "Input not found/inaccessible"
                elif re.search(r"failed\s+to\s+open\s+file", full_output_text, re.IGNORECASE):
                    error_type = "file_open_failed"
                    error_details = "Failed to open input"
                elif re.search(r"permission\s+denied", full_output_text, re.IGNORECASE):
                    error_type = "permission_denied"
                    error_details = "Permission denied"
                elif re.search(r"vmaf\s+.*?error", full_output_text, re.IGNORECASE):
                    error_type = "vmaf_calculation_failed"
                    error_details = "VMAF calculation failed"
                elif re.search(r"encode\s+.*?error", full_output_text, re.IGNORECASE):
                    error_type = "encoding_failed"
                    error_details = "Encoding failed"
                elif re.search(r"out\s+of\s+memory", full_output_text, re.IGNORECASE):
                    error_type = "memory_error"
                    error_details = "Out of memory"
                elif "ab-av1.exe" in full_output_text and "not recognized" in full_output_text:
                    error_type = "executable_not_found"
                    error_details = "ab-av1.exe command failed (not found or path issue?)"
                elif return_code != 0:
                    error_details = f"ab-av1 exited with code {return_code}"
                    logger.info(f"No specific error pattern matched, using generic exit code message: {error_details}")

                last_error_info = {
                    "return_code": return_code,
                    "error_type": error_type,
                    "error_details": error_details,
                    "command": cmd_str_log,
                    "output_tail": full_output_text.splitlines()[-20:]
                    if full_output_text
                    else ["<No output captured>"],
                }
                logger.warning(
                    f"[Attempt VMAF {current_vmaf_target}] Failed (rc={return_code}): "
                    f"{error_details} (Type: {error_type})"
                )

                # Handle fallback for CRF search failures
                logger.debug(f"Checking if error type '{error_type}' triggers VMAF fallback...")
                if error_type == "crf_search_failed":
                    current_vmaf_target -= VMAF_FALLBACK_STEP
                    if current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET:
                        logger.info(f"--> Retrying {anonymized_input_path} with VMAF target: {current_vmaf_target}")
                        clean_ab_av1_temp_folders(output_dir)
                        continue
                    logger.error(f"CRF search failed down to minimum VMAF {MIN_VMAF_FALLBACK_TARGET}. Stopping.")
                    last_error_info["error_details"] = f"CRF search failed down to VMAF {MIN_VMAF_FALLBACK_TARGET}"
                    break
                logger.error(f"Non-recoverable error type '{error_type}'. Stopping attempts for this file.")
                clean_ab_av1_temp_folders(output_dir)
                break

        # --- End of VMAF Fallback Loop ---

        # --- Failure Reporting ---
        if not success:
            clean_ab_av1_temp_folders(output_dir)
            if last_error_info:
                error_msg = f"ab-av1 failed (rc={last_error_info['return_code']}): {last_error_info['error_details']}"
                logger.error(error_msg)
                logger.error(f"Last Cmd: {last_error_info['command']}")
                logger.error(
                    f"Last Output tail ({len(last_error_info['output_tail'])} lines):\n"
                    f"{''.join(last_error_info['output_tail'])}"
                )

                if self.file_info_callback:
                    self.file_info_callback(
                        os.path.basename(input_path),
                        "failed",
                        {
                            "message": error_msg,
                            "type": last_error_info["error_type"],
                            "details": last_error_info["error_details"],
                            "command": last_error_info["command"],
                        },
                    )

                # Map error types to exception classes
                error_type = last_error_info["error_type"]
                exc_map = {
                    "invalid_input_data": InputFileError,
                    "file_not_found": InputFileError,
                    "file_open_failed": InputFileError,
                    "no_video_stream": InputFileError,
                    "analysis_failed": InputFileError,
                    "vmaf_calculation_failed": VMAFError,
                    "encoding_failed": EncodingError,
                    "memory_error": EncodingError,
                    "crf_search_failed": EncodingError,
                    "executable_not_found": EncodingError,
                    "permission_denied": EncodingError,
                    "output_dir_creation_failed": OutputFileError,
                    "rename_failed": OutputFileError,
                    "missing_output_on_success": OutputFileError,
                    "pipe_read_error": AbAv1Error,
                }
                exception_class = exc_map.get(error_type, AbAv1Error)
                raise exception_class(
                    error_msg, command=last_error_info["command"], output=full_output_text, error_type=error_type
                )
            generic_error_msg = f"Encode failed for {anonymized_input_path} for unknown reasons after loop."
            logger.error(generic_error_msg)
            if self.file_info_callback:
                self.file_info_callback(
                    os.path.basename(input_path), "failed", {"message": generic_error_msg, "type": "unknown_loop_error"}
                )
            raise AbAv1Error(generic_error_msg, error_type="unknown_loop_error")

        # --- Success Path ---
        logger.info(
            f"ab-av1 completed successfully for {anonymized_input_path} "
            f"(used VMAF target {stats.get('vmaf_target_used', '?')})"
        )
        stats = self.parser.parse_final_output(full_output_text, stats)

        # --- Logging Final Stats ---
        if stats.get("crf") is not None:
            logger.info(f"Final CRF: {stats['crf']}")
        if stats.get("vmaf") is not None:
            logger.info(f"Final VMAF: {stats['vmaf']:.2f}")
        if stats.get("size_reduction") is not None:
            logger.info(f"Final Size reduction: {stats['size_reduction']:.2f}%")
        else:
            logger.warning("Final size reduction could not be determined from parsing.")

        # --- Post-Success Sanity Checks ---
        if not os.path.exists(output_path):
            error_msg = f"CRITICAL: Output file missing after reported success: {anonymized_output_path}"
            logger.error(error_msg)
            if self.file_info_callback:
                self.file_info_callback(
                    os.path.basename(input_path),
                    "failed",
                    {"message": error_msg, "type": "missing_output_post_success"},
                )
            raise OutputFileError(error_msg, command=cmd_str_log, error_type="missing_output_post_success")

        # --- Completion Callback ---
        if self.file_info_callback:
            final_stats_for_callback = {
                "message": (
                    f"Complete (VMAF {stats.get('vmaf', 'N/A'):.2f} @ Target {stats.get('vmaf_target_used', '?')})"
                ),
                "vmaf": stats.get("vmaf"),
                "crf": stats.get("crf"),
                "vmaf_target_used": stats.get("vmaf_target_used"),
                "size_reduction": stats.get("size_reduction"),
                "output_path": output_path,
            }
            try:
                final_size = os.path.getsize(output_path)
                final_stats_for_callback["output_size"] = final_size
                logger.info(f"Final output size: {final_size} bytes ({format_file_size(final_size)})")
            except Exception as size_e:
                logger.warning(f"Could not get final output size for callback: {size_e}")

            self.file_info_callback(os.path.basename(input_path), "completed", final_stats_for_callback)

        # --- Temp Folder Cleanup (Final Check) ---
        cleanup_dir = os.path.dirname(output_path)
        cleaned_count = clean_ab_av1_temp_folders(cleanup_dir)
        if cleaned_count > 0:
            logger.info(f"Cleaned {cleaned_count} leftover temporary folder(s) in {cleanup_dir}.")
        else:
            logger.debug(f"No leftover temporary folders found to clean in {cleanup_dir}.")

        # --- Clear Callbacks ---
        self.file_info_callback = None
        self.parser.file_info_callback = None
        return stats
