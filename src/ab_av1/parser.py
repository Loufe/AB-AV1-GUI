# src/ab_av1/parser.py
"""
Parses the output stream from the ab-av1 executable.
"""

import re
import os
import logging

from src.utils import anonymize_filename, format_file_size # Need these for parsing/logging

logger = logging.getLogger(__name__)

class AbAv1Parser:
    """Parses output lines from ab-av1/ffmpeg to extract progress and stats."""

    def __init__(self, file_info_callback: callable = None):
        """
        Args:
            file_info_callback: Optional callback function to send progress updates.
                                Signature: callback(filename_basename, status, info_dict)
        """
        self.file_info_callback = file_info_callback
        # Pre-compile regex patterns for efficiency
        # Phase transition
        self._re_phase_encode = re.compile(r'ab_av1::command::encode\].*encoding', re.IGNORECASE)
        # CRF Search phase
        self._re_crf_vmaf = re.compile(r'crf\s+(\d+)\s+VMAF\s+(\d+\.?\d*)', re.IGNORECASE)
        self._re_best_crf = re.compile(r'Best\s+CRF:\s+(\d+)', re.IGNORECASE)
        # Encoding phase - time based
        self._re_time_progress = re.compile(r'\stime=(\d{2,}):(\d{2}):(\d{2})\.(\d+)')
        # Encoding phase - percentage based (often from summary lines)
        self._re_percent_progress = re.compile(r'^\s*(\d{1,3}(?:\.\d+)?)\s*%\s*,?\s*')
        # Encoding phase - size reduction percentage
        self._re_size_reduction_percent = re.compile(r'Output\s+size:.*?\((\d+\.?\d*)\s*%\s+of\s+source\)')
        # Encoding phase - fps
        self._re_fps = re.compile(r'(\d+\.?\d*)\s+fps')
        # Encoding phase - eta variations
        self._re_eta_sec = re.compile(r'eta\s+(\d+)\s*s(?:ec(?:onds?)?)?\b', re.IGNORECASE)
        self._re_eta_min = re.compile(r'eta\s+(\d+)\s*min(?:ute)?s?\b', re.IGNORECASE)
        self._re_eta_time = re.compile(r'eta\s+(\d+:\d{2}:\d{2})\b', re.IGNORECASE)
        self._re_eta_min_sec = re.compile(r'eta\s+(\d+:\d{2})\b', re.IGNORECASE)
        # Error patterns
        self._re_error_generic = re.compile(r'error|failed|invalid', re.IGNORECASE)

    def parse_line(self, line: str, stats: dict) -> dict:
        """
        Parses a single line of output, updates the stats dictionary,
        and potentially triggers the file_info_callback.

        Args:
            line: The line of text from stdout/stderr.
            stats: The current statistics dictionary for the ongoing process.

        Returns:
            The updated statistics dictionary.
        """
        line = line.strip()
        if not line:
            return stats # Skip empty lines

        try:
            anonymized_input_basename = os.path.basename(stats.get("input_path", "unknown_file"))
            current_phase = stats.get("phase", "crf-search")
            processed_line = False # Flag if line yielded useful info

            # --- Phase Transition Detection ---
            if current_phase == "crf-search" and self._re_phase_encode.search(line):
                logger.info(f"Phase transition to Encoding for {anonymize_filename(stats.get('input_path', ''))}")
                stats["phase"] = "encoding"
                stats["progress_quality"] = 100.0
                stats["progress_encoding"] = 0.0
                stats["last_reported_encoding_progress"] = 0.0 # Reset reported progress
                processed_line = True
                if self.file_info_callback:
                    callback_info = {
                        "progress_quality": 100.0, "progress_encoding": 0.0,
                        "message": "Encoding started", "phase": stats["phase"],
                        "vmaf": stats.get("vmaf"), "crf": stats.get("crf"),
                        "size_reduction": stats.get("size_reduction"),
                        "original_size": stats.get("original_size")
                    }
                    self.file_info_callback(anonymized_input_basename, "progress", callback_info)
                return stats # Don't process other rules on phase transition line

            # --- CRF Search Phase Parsing ---
            if current_phase == "crf-search":
                new_quality_progress = stats.get("progress_quality", 0)
                crf_vmaf_match = self._re_crf_vmaf.search(line)
                if crf_vmaf_match:
                    try:
                        crf_val = int(crf_vmaf_match.group(1))
                        vmaf_val = float(crf_vmaf_match.group(2))
                        stats["crf"] = crf_val
                        stats["vmaf"] = vmaf_val
                        processed_line = True
                        logger.info(f"CRF search update: CRF={stats['crf']}, VMAF={stats['vmaf']:.2f}")
                        new_quality_progress = min(90.0, stats.get("progress_quality", 0) + 10.0)
                    except (ValueError, IndexError) as e:
                        logger.warning(f"Error parsing CRF/VMAF values from line '{line[:80]}...': {e}")

                best_crf_match = self._re_best_crf.search(line)
                if best_crf_match:
                    try:
                        crf_val = int(best_crf_match.group(1))
                        stats["crf"] = crf_val
                        processed_line = True
                        logger.info(f"Best CRF determined: {stats['crf']}")
                        new_quality_progress = 95.0
                    except (ValueError, IndexError) as e:
                        logger.warning(f"Error parsing Best CRF value from line '{line[:80]}...': {e}")

                if new_quality_progress > stats.get("progress_quality", 0):
                    stats["progress_quality"] = new_quality_progress
                    if self.file_info_callback:
                        # Prepare the VMAF part of the message separately to avoid syntax error
                        vmaf_part = "?"
                        current_vmaf = stats.get("vmaf")
                        if current_vmaf is not None:
                            try:
                                vmaf_part = f"{float(current_vmaf):.1f}" # Format here
                            except (ValueError, TypeError):
                                logger.warning(f"Could not format VMAF value '{current_vmaf}' for message.")
                                vmaf_part = str(current_vmaf) # Use raw value if formatting fails

                        # Build the callback info dictionary
                        callback_info = {
                            "progress_quality": stats["progress_quality"], "progress_encoding": 0,
                            "message": f"Detecting Quality (CRF:{stats.get('crf', '?')}, VMAF:{vmaf_part})", # Use the pre-formatted part
                            "phase": current_phase, "vmaf": stats.get("vmaf"), "crf": stats.get("crf"),
                            "size_reduction": stats.get("size_reduction"),
                            "original_size": stats.get("original_size")
                        }
                        self.file_info_callback(anonymized_input_basename, "progress", callback_info)

            # --- Encoding Phase Parsing ---
            elif current_phase == "encoding":
                logger.debug(f"Raw Enc Line: {line}") # Log all lines in encoding phase for debug
                newly_calculated_progress = None # Store progress calculated from this line

                # --- Time-based progress ---
                time_match = self._re_time_progress.search(line)
                if time_match:
                    try:
                        h, m, s, ms_str = time_match.groups()
                        ms = float(f"0.{ms_str}")
                        current_seconds = (int(h) * 3600) + (int(m) * 60) + int(s) + ms
                        total_duration = stats.get("total_duration_seconds", 0.0)
                        if total_duration > 0:
                            time_based_progress = min(100.0, max(0.0, (current_seconds / total_duration) * 100.0))
                            newly_calculated_progress = time_based_progress
                            processed_line = True
                        else: logger.warning("Total duration is 0, cannot calculate time-based progress.")
                    except (ValueError, TypeError, IndexError) as e:
                        logger.warning(f"Cannot parse time-based progress values from line '{line[:80]}...': {e}")

                # --- Percentage-based progress ---
                progress_match = self._re_percent_progress.match(line)
                if progress_match:
                    try:
                        encoding_percent = float(progress_match.group(1))
                        clamped_encoding_percent = max(0.0, min(100.0, encoding_percent))
                        # Percentage is usually more direct, prioritize it
                        newly_calculated_progress = clamped_encoding_percent
                        processed_line = True
                        logger.info(f"[FFMPEG %] {line.strip()}") # Log summary line
                    except (ValueError, TypeError) as e:
                        logger.warning(f"Cannot parse encoding progress % from line '{line[:80]}...': {e}")

                # --- Apply Throttling and Send Callback ---
                if newly_calculated_progress is not None:
                    last_reported = stats.get("last_reported_encoding_progress", -1.0)
                    # Update if progress changed significantly or reached near 100%
                    if abs(newly_calculated_progress - last_reported) >= 0.5 or (newly_calculated_progress >= 99.9 and last_reported < 99.9):
                        stats["progress_encoding"] = newly_calculated_progress
                        stats["last_reported_encoding_progress"] = newly_calculated_progress

                        if self.file_info_callback:
                            callback_data = {
                                "progress_quality": 100.0,
                                "progress_encoding": stats["progress_encoding"],
                                "message": f"Encoding: {stats['progress_encoding']:.1f}%",
                                "phase": current_phase,
                                "original_size": stats.get("original_size"),
                                "vmaf": stats.get("vmaf"),
                                "crf": stats.get("crf"),
                                "size_reduction": stats.get("size_reduction") or stats.get("estimated_size_reduction"),
                                "output_size": stats.get("estimated_output_size"),
                                "is_estimate": True if stats.get("estimated_output_size") else False # Mark if size is estimated
                            }
                            logger.debug(f"Sending progress callback: {callback_data['progress_encoding']:.1f}%")
                            self.file_info_callback(anonymized_input_basename, "progress", callback_data)

                # --- Size reduction parsing ---
                size_match = self._re_size_reduction_percent.search(line)
                if size_match:
                    try:
                        size_percentage = float(size_match.group(1))
                        new_size_reduction = 100.0 - size_percentage
                        # Update if significantly different
                        if abs(stats.get("size_reduction", -100.0) - new_size_reduction) > 0.1: # Use -100 as default to force first update
                            stats["size_reduction"] = new_size_reduction
                            processed_line = True
                            logger.info(f"Parsed size reduction update: {stats['size_reduction']:.1f}%")
                    except (ValueError, IndexError) as e:
                        logger.warning(f"Cannot parse size reduction % from line '{line[:80]}...': {e}")

                # --- FPS/ETA parsing (update internal stats, not necessarily triggering callback here) ---
                fps_match = self._re_fps.search(line)
                if fps_match:
                    try: stats["last_ffmpeg_fps"] = fps_match.group(1); processed_line = True
                    except IndexError: pass # Ignore if group doesn't exist

                eta_text = None
                eta_match_time = self._re_eta_time.search(line)
                eta_match_min_sec = self._re_eta_min_sec.search(line)
                eta_match_min = self._re_eta_min.search(line)
                eta_match_sec = self._re_eta_sec.search(line)
                if eta_match_time: eta_text = f"{eta_match_time.group(1)}"
                elif eta_match_min_sec: eta_text = f"0:{eta_match_min_sec.group(1)}"
                elif eta_match_min: eta_text = f"{eta_match_min.group(1)} min"
                elif eta_match_sec: eta_text = f"{eta_match_sec.group(1)} sec"

                if eta_text is not None:
                    stats["eta_text"] = eta_text
                    processed_line = True
                # else: stats["eta_text"] = None # Keep last known ETA if not found on this line? Or clear? Let's clear.
                elif not any([eta_match_time, eta_match_min_sec, eta_match_min, eta_match_sec]):
                    stats["eta_text"] = None # Clear if no ETA found


            # --- General Error Detection ---
            if self._re_error_generic.search(line):
                logger.warning(f"Possible error detected in output line: {line}")
                # Note: Error *handling* (like stopping) is done in the wrapper based on return code/patterns

            # Debug log after parsing attempt if useful info was found
            if processed_line:
                logger.debug(f"Post-Parse Stats: Phase={stats.get('phase')}, Qual={stats.get('progress_quality'):.1f}, Enc={stats.get('progress_encoding'):.1f}, LastReported={stats.get('last_reported_encoding_progress'):.1f}, VMAF={stats.get('vmaf')}, CRF={stats.get('crf')}")

        except Exception as e:
            # Catch-all for unexpected errors during line processing
            logger.error(f"General error processing output line: '{line[:80]}...' - {e}", exc_info=True)

        return stats # Always return the potentially modified stats dictionary

    def parse_final_output(self, output_text: str, stats: dict) -> dict:
        """
        Extract final statistics from the complete output text as a fallback
        or verification step, updating the provided stats dictionary.

        Args:
            output_text: The complete console output text from ab-av1.
            stats: The statistics dictionary to update.

        Returns:
            The updated statistics dictionary.
        """
        logger.debug("Running final output parsing as fallback/verification.")

        # --- Final VMAF ---
        try:
            # Find all VMAF scores, use the last one reported
            vmaf_matches = re.findall(r'VMAF:\s+(\d+\.\d+)', output_text, re.IGNORECASE)
            if vmaf_matches:
                final_vmaf = float(vmaf_matches[-1])
                # Only overwrite if current value is None or significantly different
                if stats.get("vmaf") is None or abs(stats.get("vmaf", -1.0) - final_vmaf) > 0.01:
                    logger.info(f"Final VMAF extracted/verified: {final_vmaf:.2f} (overwriting {stats.get('vmaf')})")
                    stats["vmaf"] = final_vmaf
            elif stats.get("vmaf") is None:
                logger.warning("Could not find VMAF score in final output.")
        except (ValueError, IndexError, TypeError) as e:
            logger.warning(f"Error parsing final VMAF score: {e}")

        # --- Final CRF ---
        try:
            # Find all 'Best CRF' mentions, use the last one
            crf_matches = re.findall(r'Best\s+CRF:\s+(\d+)', output_text, re.IGNORECASE)
            if crf_matches:
                final_crf = int(crf_matches[-1])
                if stats.get("crf") != final_crf:
                    logger.info(f"Final CRF extracted/verified: {final_crf} (overwriting {stats.get('crf')})")
                    stats["crf"] = final_crf
            elif stats.get("crf") is None:
                logger.warning("Could not find Best CRF in final output.")
        except (ValueError, IndexError, TypeError) as e:
            logger.warning(f"Error parsing final CRF score: {e}")

        # --- Final Size Reduction ---
        try:
            # Find all 'Output size... (%)' mentions, use the last one
            size_percent_matches = self._re_size_reduction_percent.findall(output_text) # Use precompiled regex
            if size_percent_matches:
                # Note: findall returns only the capturing group content
                final_size_percent = float(size_percent_matches[-1])
                final_reduction = 100.0 - final_size_percent
                # Compare with possible float precision issues
                if stats.get("size_reduction") is None or abs(stats.get("size_reduction", -100.0) - final_reduction) > 0.01:
                    logger.info(f"Final size reduction extracted/verified: {final_reduction:.2f}% (overwriting {stats.get('size_reduction')})")
                    stats["size_reduction"] = final_reduction
            elif stats.get("size_reduction") is None:
                # Fallback: try parsing absolute sizes if percentage not found
                logger.debug("Size reduction percentage not found in final output, trying absolute sizes...")
                input_size_match = re.search(r'Input\s+size:\s+(\d+\.?\d*)\s+(\w+)', output_text, re.IGNORECASE)
                output_size_match = re.search(r'Output\s+size:\s+(\d+\.?\d*)\s+(\w+)', output_text, re.IGNORECASE)
                if input_size_match and output_size_match:
                    try:
                        input_size = float(input_size_match.group(1))
                        input_unit = input_size_match.group(2).upper()
                        output_size = float(output_size_match.group(1))
                        output_unit = output_size_match.group(2).upper()
                        unit_multipliers = {'B': 1, 'KB': 1024, 'MB': 1024**2, 'GB': 1024**3, 'TB': 1024**4}
                        input_bytes = input_size * unit_multipliers.get(input_unit, 1)
                        output_bytes = output_size * unit_multipliers.get(output_unit, 1)
                        if input_bytes > 0:
                            calculated_reduction = 100.0 * (1.0 - (output_bytes / input_bytes))
                            logger.info(f"Final size reduction calculated from sizes: {calculated_reduction:.2f}%")
                            stats["size_reduction"] = calculated_reduction
                        else:
                            logger.warning("Input size is zero, cannot calculate reduction from absolute sizes.")
                    except (ValueError, KeyError, TypeError, IndexError, ZeroDivisionError) as calc_e:
                        logger.warning(f"Could not calculate size reduction from final absolute sizes: {calc_e}")
                else:
                    logger.warning("Could not find size reduction percentage or absolute sizes in final output.")
        except (ValueError, IndexError, TypeError) as e:
            logger.warning(f"Error parsing final size reduction: {e}")

        return stats