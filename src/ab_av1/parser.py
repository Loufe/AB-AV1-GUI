# src/ab_av1/parser.py
"""
Parses the output stream from the ab-av1 executable.
Focuses on lines expected directly from ab-av1 via stdout/stderr.
"""

import logging
import os
import re
from typing import Any, Callable

from src.config import SIZE_REDUCTION_CHANGE_THRESHOLD, VMAF_CHANGE_THRESHOLD
from src.models import ProgressEvent
from src.utils import anonymize_filename

logger = logging.getLogger(__name__)


class AbAv1Parser:
    """Parses output lines from ab-av1 to extract progress and stats."""

    def __init__(self, file_info_callback: Callable[..., Any] | None = None):
        """
        Args:
            file_info_callback: Optional callback function to send progress updates.
                                Signature: callback(filename_basename, status, info_dict)
        """
        self.file_info_callback = file_info_callback

        # Pre-compile regex patterns for efficiency
        # Detect ffmpeg progress accurately with multiple patterns
        self._re_ffmpeg_progress = re.compile(r"(?:frame|fps|q|size|time|bitrate|speed)", re.IGNORECASE)
        self._re_ffmpeg_time = re.compile(r"time=\s*(\d+):(\d+):(\d+\.\d+)", re.IGNORECASE)
        self._re_ffmpeg_time_seconds = re.compile(r"time=\s*(\d+\.\d+)", re.IGNORECASE)
        self._re_ffmpeg_frame = re.compile(r"frame=\s*(\d+)", re.IGNORECASE)
        self._re_ffmpeg_fps = re.compile(r"fps=\s*(\d+\.?\d*)", re.IGNORECASE)
        self._re_ffmpeg_speed = re.compile(r"speed=\s*(\d+\.?\d*)x", re.IGNORECASE)
        self._re_ffmpeg_size = re.compile(r"size=\s*(\d+)([kKmMgG]?[bB])", re.IGNORECASE)

        # Refined regex patterns based on actual formats in logs
        self._re_phase_encode_start = re.compile(
            r"ab_av1::command::encode\]\s*encoding(?:\s+video|\s+\S+\.mkv|\s|$)|Starting encoding", re.IGNORECASE
        )
        self._re_sample_progress = re.compile(
            r"\[.*?sample_encode\].*?(\d+(\.\d+)?)%,\s*(\d+)\s*fps,\s*eta\s*(.*?)(?=$|\))", re.IGNORECASE
        )
        self._re_main_encoding = re.compile(
            r"command::encode\]\s*(\d+)%,\s*(\d+)\s*fps,\s*eta\s+([\w\s]+)(?:$|\)|\])", re.IGNORECASE
        )
        self._re_crf_vmaf = re.compile(r"crf\s+(\d+)\s+VMAF\s+(\d+\.?\d*)", re.IGNORECASE)
        self._re_best_crf = re.compile(r"Best\s+CRF:\s+(\d+)", re.IGNORECASE)
        self._re_size_reduction_percent = re.compile(
            r"predicted video stream size.*?\((\d+\.?\d*)\s*%\)", re.IGNORECASE
        )

        # Encoding Phase Summary Line (ab-av1 direct output)
        # Example: ⠖ 00:00:37 Encoding -------- (encoding, eta 0s)
        self._re_ab_av1_encoding_summary = re.compile(
            r"\b(Encoding)\s*-*\s*\((\w+),\s*eta\s*([\w\s:]+)\)", re.IGNORECASE
        )

        # Error patterns
        self._re_error_generic = re.compile(r"error|failed|invalid", re.IGNORECASE)
        self._re_error_crf_fail = re.compile(r"Failed\s+to\s+find\s+a\s+suitable\s+crf", re.IGNORECASE)

    def _build_encoding_callback_data(
        self, stats: dict[str, Any], progress_encoding: float, message: str, eta_text: str | None = None
    ) -> ProgressEvent:
        """Build ProgressEvent for encoding progress updates.

        Args:
            stats: Current stats dictionary with vmaf, crf, size_reduction, etc.
            progress_encoding: Current encoding progress percentage (0-100).
            message: Human-readable progress message.
            eta_text: Optional ETA text to include.

        Returns:
            ProgressEvent suitable for passing to file_info_callback.
        """
        return ProgressEvent(
            progress_quality=100.0,
            progress_encoding=progress_encoding,
            message=message,
            phase="encoding",
            original_size=stats.get("original_size"),
            vmaf=stats.get("vmaf"),
            crf=stats.get("crf"),
            size_reduction=stats.get("size_reduction"),
            vmaf_target_used=stats.get("vmaf_target_used"),
            eta_text=eta_text,
            output_size=stats.get("estimated_output_size") if stats.get("estimated_output_size") else None,
            is_estimate=True if stats.get("estimated_output_size") else None,
        )

    def parse_line(self, line: str, stats: dict) -> dict:
        """
        Parses a single line of output known to come from ab-av1's stdout/stderr,
        updates the stats dictionary, and potentially triggers the file_info_callback.

        Args:
            line: The line of text from stdout/stderr.
            stats: The current statistics dictionary for the ongoing process.

        Returns:
            The updated statistics dictionary.
        """
        line = line.strip()
        if not line:
            return stats  # Skip empty lines

        try:
            anonymized_input_basename = os.path.basename(stats.get("input_path", "unknown_file"))
            current_phase = stats.get("phase", "crf-search")
            processed_line = False  # Flag if line yielded useful info

            # --- Phase Transition Detection ---
            if current_phase == "crf-search" and self._re_phase_encode_start.search(line):
                logger.info(
                    f"Phase transition to Encoding detected for {anonymize_filename(stats.get('input_path', ''))}"
                )
                stats["phase"] = "encoding"
                stats["progress_quality"] = 100.0
                stats["progress_encoding"] = 0.0
                stats["last_reported_encoding_progress"] = 0.0
                processed_line = True

                # Log extra information about this transition
                logger.info(
                    f"Starting encoding phase with CRF: {stats.get('crf', '?')}, "
                    f"VMAF Target: {stats.get('vmaf_target_used', '?')}"
                )
                logger.info(f"Duration in seconds: {stats.get('total_duration_seconds', '?')}")
                logger.info("Looking for ffmpeg progress output lines in the form 'frame=XXX fps=XXX time=XX:XX:XX'")

                if self.file_info_callback:
                    callback_info = ProgressEvent(
                        progress_quality=100.0,
                        progress_encoding=0.0,
                        message="Encoding started",
                        phase=stats["phase"],
                        vmaf=stats.get("vmaf"),
                        crf=stats.get("crf"),
                        size_reduction=stats.get("size_reduction"),  # Use predicted if available
                        original_size=stats.get("original_size"),
                        vmaf_target_used=stats.get("vmaf_target_used"),
                    )
                    self.file_info_callback(anonymized_input_basename, "progress", callback_info)
                return stats  # Return early

            # --- CRF Search Phase Parsing ---
            if current_phase == "crf-search":
                new_quality_progress = stats.get("progress_quality", 0)
                crf_vmaf_match = self._re_crf_vmaf.search(line)
                if crf_vmaf_match:
                    processed_line = True
                    try:
                        crf_val = int(crf_vmaf_match.group(1))
                        vmaf_val = float(crf_vmaf_match.group(2))
                        stats["crf"] = crf_val
                        stats["vmaf"] = vmaf_val
                        logger.info(f"CRF search update: CRF={stats['crf']}, VMAF={stats['vmaf']:.2f}")
                        new_quality_progress = min(90.0, stats.get("progress_quality", 0) + 10.0)
                    except (ValueError, IndexError) as e:
                        logger.warning(f"Error parsing CRF/VMAF values from line '{line[:80]}...': {e}")

                best_crf_match = self._re_best_crf.search(line)
                if best_crf_match:
                    processed_line = True
                    try:
                        crf_val = int(best_crf_match.group(1))
                        stats["crf"] = crf_val
                        logger.info(f"Best CRF determined: {stats['crf']}")
                        new_quality_progress = 95.0
                    except (ValueError, IndexError) as e:
                        logger.warning(f"Error parsing Best CRF value from line '{line[:80]}...': {e}")

                # Predicted size reduction parsing (often shown during CRF search)
                size_match = self._re_size_reduction_percent.search(line)
                if size_match:
                    processed_line = True
                    try:
                        size_percentage = float(size_match.group(1))
                        # This is percentage *of original*, so reduction is 100 - this
                        new_size_reduction = 100.0 - size_percentage
                        # Only update if changed significantly
                        # Using None check to prevent TypeError: unsupported operand type(s)
                        # for -: 'NoneType' and 'float'
                        current_reduction = stats.get("size_reduction")
                        reduction_changed = (
                            current_reduction is None
                            or abs(current_reduction - new_size_reduction) > SIZE_REDUCTION_CHANGE_THRESHOLD
                        )
                        if reduction_changed:
                            stats["size_reduction"] = new_size_reduction
                            logger.info(f"Parsed predicted size reduction: {stats['size_reduction']:.1f}%")
                    except (ValueError, IndexError) as e:
                        logger.warning(f"Cannot parse predicted size reduction % from line '{line[:80]}...': {e}")

                # Send callback if quality progress increased
                if new_quality_progress > stats.get("progress_quality", 0):
                    stats["progress_quality"] = new_quality_progress
                    if self.file_info_callback:
                        vmaf_part = "?"
                        current_vmaf = stats.get("vmaf")
                        if current_vmaf is not None:
                            try:
                                vmaf_part = f"{float(current_vmaf):.1f}"
                            except (ValueError, TypeError):
                                vmaf_part = str(current_vmaf)

                        callback_info = ProgressEvent(
                            progress_quality=stats["progress_quality"],
                            progress_encoding=0,
                            message=f"Detecting Quality (CRF:{stats.get('crf', '?')}, VMAF:{vmaf_part})",
                            phase=current_phase,
                            vmaf=stats.get("vmaf"),
                            crf=stats.get("crf"),
                            size_reduction=stats.get("size_reduction"),  # Include prediction
                            original_size=stats.get("original_size"),
                            vmaf_target_used=stats.get("vmaf_target_used"),
                        )
                        self.file_info_callback(anonymized_input_basename, "progress", callback_info)

            # --- Encoding Phase Parsing (ab-av1 Summary Line) ---
            elif current_phase == "encoding":
                # Look for sample encoding progress format
                sample_progress_match = self._re_sample_progress.search(line)
                if sample_progress_match:
                    progress_pct = float(sample_progress_match.group(1))
                    fps = int(sample_progress_match.group(3))
                    eta_text = sample_progress_match.group(4).strip()

                    logger.info(f"Sample encoding progress detected: {progress_pct}%, {fps} fps, ETA: {eta_text}")

                    # Update stats
                    stats["progress_encoding"] = progress_pct
                    stats["last_ffmpeg_fps"] = fps
                    stats["eta_text"] = eta_text

                    # Send progress update
                    if self.file_info_callback:
                        message = f"Encoding: {progress_pct:.1f}% (FPS: {fps}, ETA: {eta_text})"
                        callback_data = self._build_encoding_callback_data(stats, progress_pct, message, eta_text)
                        self.file_info_callback(anonymized_input_basename, "progress", callback_data)

                    processed_line = True
                    return stats

                # Look for any main encoding indicators
                main_encoding_match = self._re_main_encoding.search(line)
                if main_encoding_match:
                    logger.info(f"Main encoding phase detected: {line}")
                    # Parse the progress information from the match
                    progress_pct = float(main_encoding_match.group(1))
                    fps = int(main_encoding_match.group(2))
                    eta_text = main_encoding_match.group(3).strip()

                    logger.info(f"Main encoding progress: {progress_pct}%, {fps} fps, ETA: {eta_text}")

                    # Update stats
                    stats["progress_encoding"] = progress_pct
                    stats["last_ffmpeg_fps"] = fps
                    stats["eta_text"] = eta_text

                    # Send progress update
                    if self.file_info_callback:
                        message = f"Encoding: {progress_pct:.1f}% (FPS: {fps}, ETA: {eta_text})"
                        callback_data = self._build_encoding_callback_data(stats, progress_pct, message, eta_text)
                        self.file_info_callback(anonymized_input_basename, "progress", callback_data)

                    processed_line = True
                    return stats

                # Even simpler: look for anything with a percentage
                percentage_match = re.search(r"(\d+)\s*%", line)
                if percentage_match:
                    progress_pct = float(percentage_match.group(1))
                    logger.info(f"Percentage detected in line: {progress_pct}% in '{line}'")

                    # Basic progress update (no FPS or ETA)
                    stats["progress_encoding"] = progress_pct

                    # Send progress update
                    if self.file_info_callback:
                        message = f"Encoding: {progress_pct:.1f}%"
                        callback_data = self._build_encoding_callback_data(stats, progress_pct, message)
                        self.file_info_callback(anonymized_input_basename, "progress", callback_data)

                    processed_line = True

                # Fall back to ab-av1 summary line if no ffmpeg progress
                summary_match = self._re_ab_av1_encoding_summary.search(line)
                if summary_match:
                    processed_line = True
                    try:
                        phase_text = summary_match.group(2)
                        eta_text = summary_match.group(3).strip()
                        # Only update ETA if it changed to avoid spamming logs/UI
                        if stats.get("eta_text") != eta_text:
                            stats["eta_text"] = eta_text
                            logger.info(f"Parsed ab-av1 summary: Phase='{phase_text}', ETA='{eta_text}'")

                            # Send progress update using existing percentage, new ETA
                            if self.file_info_callback:
                                current_progress = stats.get("progress_encoding", 0.0)
                                message = f"Encoding: {current_progress:.1f}% (ETA: {eta_text})"
                                callback_data = self._build_encoding_callback_data(
                                    stats, current_progress, message, eta_text
                                )
                                logger.debug(f"Sending progress callback (summary): {current_progress:.1f}%")
                                self.file_info_callback(anonymized_input_basename, "progress", callback_data)

                    except IndexError:
                        logger.warning(f"Error parsing groups from ab-av1 summary line: '{line}'")

            # --- General Error Detection ---
            # Check for generic error keywords OR the specific CRF fail message
            if self._re_error_generic.search(line) or self._re_error_crf_fail.search(line):
                logger.warning(f"Possible error detected in output line: {line}")
                # Error *handling* (like triggering fallback) is done in the wrapper

            # Debug log after parsing attempt if useful info was found
            if processed_line:
                # Log key stats that are expected to be updated by this parser
                logger.debug(
                    f"Post-Parse Stats: Phase={stats.get('phase')}, "
                    f"Qual={stats.get('progress_quality', 0):.1f}%, VMAF={stats.get('vmaf')}, "
                    f"CRF={stats.get('crf')}, ETA={stats.get('eta_text')}, "
                    f"SizeReduc={stats.get('size_reduction')}"
                )

        except Exception:
            logger.exception(f"General error processing output line: '{line[:80]}...'")

        return stats  # Always return the potentially modified stats dictionary

    def parse_final_output(self, output_text: str, stats: dict) -> dict:
        """
        Extract final statistics from the complete output text (main pipe) as a fallback
        or verification step, updating the provided stats dictionary.

        Args:
            output_text: The complete console output text from ab-av1's stdout/stderr.
            stats: The statistics dictionary to update.

        Returns:
            The updated statistics dictionary.
        """
        logger.debug("Running final output parsing on main pipe text as fallback/verification.")

        # --- Final VMAF ---
        try:
            vmaf_matches = re.findall(r"VMAF\s+(\d+\.\d+)", output_text, re.IGNORECASE)
            if vmaf_matches:
                final_vmaf = float(vmaf_matches[-1])
                if stats.get("vmaf") is None or abs(stats.get("vmaf", -1.0) - final_vmaf) > VMAF_CHANGE_THRESHOLD:
                    logger.info(f"[Final Parse] VMAF verified/updated: {final_vmaf:.2f} (from {stats.get('vmaf')})")
                    stats["vmaf"] = final_vmaf
            elif stats.get("vmaf") is None:
                logger.warning("[Final Parse] Could not find VMAF score in main pipe output.")
        except (ValueError, IndexError, TypeError) as e:
            logger.warning(f"[Final Parse] Error parsing final VMAF score: {e}")

        # --- Final CRF ---
        try:
            crf_matches = re.findall(r"Best\s+CRF:\s+(\d+)", output_text, re.IGNORECASE)
            if crf_matches:
                final_crf = int(crf_matches[-1])
                if stats.get("crf") != final_crf:
                    logger.info(f"[Final Parse] CRF verified/updated: {final_crf} (from {stats.get('crf')})")
                    stats["crf"] = final_crf
            elif stats.get("crf") is None:
                logger.warning("[Final Parse] Could not find Best CRF in main pipe output.")
        except (ValueError, IndexError, TypeError) as e:
            logger.warning(f"[Final Parse] Error parsing final CRF score: {e}")

        # --- Final Size Reduction ---
        # Use the value potentially parsed earlier from predicted size line
        if stats.get("size_reduction") is not None:
            logger.info(f"[Final Parse] Using previously parsed size reduction: {stats['size_reduction']:.2f}%")
        else:
            # Try parsing the predicted size line again from the full text as a last resort
            size_match = self._re_size_reduction_percent.search(output_text)
            if size_match:
                try:
                    size_percentage = float(size_match.group(1))
                    final_size_reduction = 100.0 - size_percentage
                    stats["size_reduction"] = final_size_reduction
                    logger.info(
                        f"[Final Parse] Found predicted size reduction in final text: {stats['size_reduction']:.1f}%"
                    )
                except (ValueError, IndexError) as e:
                    logger.warning(f"[Final Parse] Cannot parse final predicted size reduction %: {e}")
            else:
                logger.warning(
                    "[Final Parse] Size reduction percentage not found in main pipe output "
                    "and wasn't parsed previously."
                )

        return stats
