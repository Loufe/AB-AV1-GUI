#src/ab_av1_wrapper.py
"""
Wrapper module for the ab-av1 tool in the AV1 Video Converter application.

This module provides an interface to the ab-av1 command line tool, handling
execution, progress monitoring, result parsing, and VMAF fallback.
"""
import os
import subprocess
import re
import logging
import json
import shutil
import glob
import tempfile
from pathlib import Path
# Import constants from config

from src.config import (
DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET,
MIN_VMAF_FALLBACK_TARGET, VMAF_FALLBACK_STEP
)
# Use constants from utils - Replace 'convert_app' with 'src'

from src.utils import get_video_info, anonymize_filename, format_file_size

logger = logging.getLogger(__name__)
#--- Constants for Fallback ---
# Moved to src/config.py: MIN_VMAF_FALLBACK_TARGET, VMAF_FALLBACK_STEP

class AbAv1Error(Exception):
    """Base exception for ab-av1 related errors"""
    def __init__(self, message, command=None, output=None, error_type=None):
        self.message = message
        self.command = command
        self.output = output
        self.error_type = error_type
        super().__init__(self.message)

class InputFileError(AbAv1Error): pass
class OutputFileError(AbAv1Error): pass
class VMAFError(AbAv1Error): pass
class EncodingError(AbAv1Error): pass

class AbAv1Wrapper:
    """Wrapper for the ab-av1 tool providing high-level encoding interface.


    This class handles execution of ab-av1.exe, monitors progress, parses output,
    and manages VMAF-based encoding with automatic fallback.
    """

    def _log_consolidated_progress(self, stats, current_temp_size=None, estimated_final_size=None):
        """Log a consolidated progress message to reduce the number of log lines.

        Args:
            stats: Dictionary containing encoding statistics
            current_temp_size: Current size of the output file in bytes
            estimated_final_size: Estimated final size of the output file in bytes
        """
        try:
            # Create consolidated progress message
            progress_parts = []

            # Add basic progress info
            encoding_progress = stats.get("progress_encoding", 0)
            progress_parts.append(f"{encoding_progress:.1f}%")

            # Add phase
            progress_parts.append(f"phase={stats.get('phase', 'encoding')}")

            # Add FPS if available
            if stats.get("last_ffmpeg_fps"):
                progress_parts.append(f"{stats['last_ffmpeg_fps']} fps")

            # Add ETA if available
            if stats.get("eta_text"):
                progress_parts.append(f"ETA: {stats['eta_text']}")

            # Add input filename if available (anonymized)
            if stats.get("input_path"):
                filename = os.path.basename(stats["input_path"])
                progress_parts.append(f"file={anonymize_filename(filename)}")

            # Add size info if available
            if current_temp_size and estimated_final_size:
                progress_parts.append(f"Size: {format_file_size(current_temp_size)}/{format_file_size(estimated_final_size)}")

            # Add size reduction if original size is known
            if stats.get("size_reduction") is not None:
                progress_parts.append(f"reduction={stats['size_reduction']:.1f}%")
            elif stats.get("original_size") and estimated_final_size and estimated_final_size > 0 and stats.get("original_size") > 0: # Check original size > 0
                reduction_percent = 100.0 - ((estimated_final_size / stats["original_size"]) * 100.0)
                progress_parts.append(f"reduction={reduction_percent:.1f}%")

            # Log consolidated message
            logger.info(f"PROGRESS: {' | '.join(progress_parts)}")
        except Exception as e:
            logger.error(f"Error in log_consolidated_progress: {e}")


    def __init__(self):
        """Initialize the wrapper and verify the executable exists."""
        app_dir = os.path.dirname(os.path.abspath(__file__))
        self.executable_path = os.path.join(app_dir, "ab-av1.exe")
        logger.debug(f"AbAv1Wrapper init - expecting executable at: {self.executable_path}")
        self._verify_executable()
        self.file_info_callback = None

    def _verify_executable(self) -> bool:
        """Verify that the ab-av1 executable exists at the expected location.

        Returns:
            True if the executable exists

        Raises:
            FileNotFoundError: If the executable is not found
        """
        if not os.path.exists(self.executable_path):
            # Adjusted error message to reflect that ab-av1.exe should be inside the src package now
            error_msg = (f"ab-av1.exe not found. Place inside 'src' dir.\nExpected: {self.executable_path}")
            logger.error(error_msg); raise FileNotFoundError(error_msg)
        logger.debug(f"AbAv1Wrapper init - verified: {self.executable_path}"); return True

    def _update_stats_from_line(self, line: str, stats: dict) -> None:
        """Update statistics based on a line of output from ab-av1.

        Parses output lines to extract progress updates, VMAF scores, CRF values,
        and other information, then updates the stats dictionary.

        Args:
            line: A line of output from the ab-av1 process
            stats: Dictionary to update with extracted information
        """
        line = line.strip()
        if not line: return # Skip empty lines

        try:
            anonymized_input_basename = os.path.basename(stats.get("input_path", "unknown_file"))
            current_phase = stats.get("phase", "crf-search")
            progress_quality = stats.get("progress_quality", 0)
            progress_encoding = stats.get("progress_encoding", 0)
            processed_line = False # Flag to track if any regex matched

            # --- PHASE TRANSITION DETECTION ---
            try:
                # Use \s+ for flexible spacing
                phase_transition_match = re.search(r'ab_av1::command::encode\].*encoding', line, re.IGNORECASE)
                if phase_transition_match and current_phase == "crf-search":
                    logger.info(f"Phase transition to Encoding for {anonymize_filename(stats.get('input_path', ''))}")
                    stats["phase"] = "encoding"; stats["progress_quality"] = 100.0; stats["progress_encoding"] = 0.0
                    processed_line = True
                    if self.file_info_callback:
                        # Safely get VMAF/CRF for callback
                        vmaf_val = stats.get("vmaf")
                        crf_val = stats.get("crf")
                        sr_val = stats.get("size_reduction")
                        os_val = stats.get("original_size")

                        self.file_info_callback(anonymized_input_basename, "progress", {
                            "progress_quality":100.0, "progress_encoding":0.0, "message":"Encoding started",
                            "phase":stats["phase"], "vmaf":vmaf_val, "crf":crf_val,
                            "size_reduction":sr_val, "original_size": os_val
                        })
                    return # Don't process other rules on phase transition line
            except (AttributeError, ValueError, IndexError) as e:
                logger.warning(f"Error parsing phase transition from line: '{line[:80]}...' - {e}")
            except Exception as e:
                logger.error(f"Unexpected error parsing phase transition: {e}", exc_info=True)

            # --- CRF SEARCH PHASE ---
            if current_phase == "crf-search":
                new_quality_progress = progress_quality
                try:
                    # Use \s+ for flexible spacing, capture digits and float
                    crf_vmaf_match = re.search(r'crf\s+(\d+)\s+VMAF\s+(\d+\.?\d*)', line, re.IGNORECASE)
                    if crf_vmaf_match:
                        crf_val = int(crf_vmaf_match.group(1))
                        vmaf_val = float(crf_vmaf_match.group(2))
                        stats["crf"] = crf_val; stats["vmaf"] = vmaf_val
                        processed_line = True
                        logger.info(f"CRF search update: CRF={stats['crf']}, VMAF={stats['vmaf']:.2f}")
                        new_quality_progress = min(90.0, progress_quality + 10.0)
                except (AttributeError, ValueError, IndexError) as e:
                    logger.warning(f"Error parsing CRF/VMAF update from line: '{line[:80]}...' - {e}")
                except Exception as e:
                    logger.error(f"Unexpected error parsing CRF/VMAF: {e}", exc_info=True)

                try:
                    # Use \s+ for flexible spacing
                    best_crf_match = re.search(r'Best\s+CRF:\s+(\d+)', line, re.IGNORECASE)
                    if best_crf_match:
                         crf_val = int(best_crf_match.group(1))
                         stats["crf"] = crf_val; processed_line = True
                         logger.info(f"Best CRF determined: {stats['crf']}")
                         new_quality_progress = 95.0
                except (AttributeError, ValueError, IndexError) as e:
                    logger.warning(f"Error parsing Best CRF from line: '{line[:80]}...' - {e}")
                except Exception as e:
                    logger.error(f"Unexpected error parsing Best CRF: {e}", exc_info=True)

                if new_quality_progress > progress_quality:
                     stats["progress_quality"] = new_quality_progress
                     if self.file_info_callback:
                         # Safely get VMAF/CRF for callback
                         vmaf_val = stats.get("vmaf")
                         crf_val = stats.get("crf")
                         sr_val = stats.get("size_reduction")
                         os_val = stats.get("original_size")

                         self.file_info_callback(anonymized_input_basename, "progress", {
                             "progress_quality":stats["progress_quality"], "progress_encoding":0,
                             "message":f"Detecting Quality (CRF:{crf_val or '?'}, VMAF:{f'{vmaf_val:.1f}' if vmaf_val else '?'})",
                             "phase":current_phase, "vmaf":vmaf_val, "crf":crf_val,
                             "size_reduction":sr_val, "original_size": os_val
                         })

            # --- ENCODING PHASE ---
            elif current_phase == "encoding":
                time_based_update_sent = False
                # --- Time-based progress parsing (Phase 3) ---
                try:
                    # Match HH:MM:SS.ms format
                    time_match = re.search(r'\stime=(\d{2,}):(\d{2}):(\d{2})\.(\d{2})', line)
                    if time_match:
                        h = int(time_match.group(1))
                        m = int(time_match.group(2))
                        s = int(time_match.group(3))
                        ms = int(time_match.group(4))
                        current_seconds = (h * 3600) + (m * 60) + s + (ms / 100.0)

                        total_duration = stats.get("total_duration_seconds", 0.0)
                        if total_duration > 0:
                            time_based_progress = min(100.0, max(0.0, (current_seconds / total_duration) * 100.0))

                            # Throttling logic
                            last_reported = stats.get("last_reported_encoding_progress", -1.0)
                            # Update if progress changed by at least ~0.5% or if it just reached 100% (and wasn't 100 before)
                            if abs(time_based_progress - last_reported) >= 0.5 or (time_based_progress >= 99.9 and last_reported < 99.9):
                                stats["progress_encoding"] = time_based_progress # Update internal state
                                stats["last_reported_encoding_progress"] = time_based_progress # Update last reported value

                                # Trigger callback
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
                                        "output_size": stats.get("estimated_output_size")
                                    }
                                    logger.debug(f"Sending time-based progress callback: {callback_data['progress_encoding']:.1f}%")
                                    self.file_info_callback(anonymized_input_basename, "progress", callback_data)
                                    time_based_update_sent = True # Mark that we sent an update based on time

                                processed_line = True # Mark line as processed
                except (AttributeError, ValueError, IndexError, TypeError) as e:
                    logger.warning(f"Cannot parse time-based progress from line: '{line[:80]}...' - {e}")
                except Exception as e:
                    logger.error(f"Unexpected error parsing time-based progress: {e}", exc_info=True)

                # --- Percentage-based progress parsing (existing) ---
                # Only parse this if a time-based update wasn't already sent for this line
                if not time_based_update_sent:
                    try:
                        # Match percentage at the start, be flexible with comma/spaces
                        progress_match = re.match(r'^\s*(\d{1,3}(?:\.\d+)?)\s*%\s*,?\s*', line)
                        if progress_match:
                            encoding_percent = float(progress_match.group(1))
                            clamped_encoding_percent = max(0.0, min(100.0, encoding_percent))

                            # Only update if it's significantly different from last report or if the % line is > time-based progress
                            last_reported = stats.get("last_reported_encoding_progress", -1.0)
                            if abs(clamped_encoding_percent - last_reported) >= 0.1 or clamped_encoding_percent > progress_encoding:
                                stats["progress_encoding"] = clamped_encoding_percent
                                stats["last_reported_encoding_progress"] = clamped_encoding_percent # Also update last reported here
                                stats["progress_quality"] = 100.0 # Quality phase is done
                                processed_line = True

                                # Construct callback data
                                callback_data = {
                                    "progress_quality": 100.0,
                                    "progress_encoding": stats["progress_encoding"],
                                    "message": f"Encoding: {stats['progress_encoding']:.1f}%",
                                    "phase": current_phase,
                                    "original_size": stats.get("original_size"),
                                    "vmaf": stats.get("vmaf"),
                                    "crf": stats.get("crf"),
                                    "size_reduction": stats.get("size_reduction") or stats.get("estimated_size_reduction"),
                                    "output_size": stats.get("estimated_output_size")
                                }

                                if self.file_info_callback:
                                    logger.debug(f"Sending percentage-based progress callback: {callback_data['progress_encoding']:.1f}%")
                                    self.file_info_callback(anonymized_input_basename, "progress", callback_data)

                                # Log the raw FFMPEG line only when sending update based on it
                                logger.info(f"[FFMPEG] {line.strip()}")

                    except (AttributeError, ValueError, IndexError) as e:
                        logger.warning(f"Cannot parse encoding progress from line: '{line[:80]}...' - {e}")
                    except Exception as e:
                        logger.error(f"Unexpected error parsing encoding progress: {e}", exc_info=True)

                # --- Size reduction parsing (existing) ---
                try:
                    # Use \s+ for flexible spacing
                    size_match = re.search(r'Output\s+size:.*?\((\d+\.?\d*)\s*%\s+of\s+source\)', line)
                    if size_match:
                        size_percentage = float(size_match.group(1))
                        new_size_reduction = 100.0 - size_percentage

                        # Update if significantly different
                        if abs(stats.get("size_reduction", -1.0) - new_size_reduction) > 0.1:
                            stats["size_reduction"] = new_size_reduction
                            processed_line = True
                            logger.info(f"Parsed size reduction update: {stats['size_reduction']:.1f}%")

                            # Send a dedicated update if callback exists and original size known
                            if "original_size" in stats and stats["original_size"] and self.file_info_callback:
                                original_size = stats["original_size"]
                                output_size_estimate = original_size * (size_percentage / 100.0)

                                self.file_info_callback(anonymized_input_basename, "progress", {
                                    "progress_quality": 100.0,
                                    "progress_encoding": stats["progress_encoding"], # Include current progress
                                    "message": f"Encoding: {stats.get('progress_encoding',0):.1f}%",
                                    "phase": current_phase,
                                    "vmaf": stats.get("vmaf"),
                                    "crf": stats.get("crf"),
                                    "size_reduction": stats["size_reduction"],
                                    "output_size": output_size_estimate,
                                    "original_size": original_size
                                })
                except (AttributeError, ValueError, IndexError) as e:
                    logger.warning(f"Cannot parse size reduction from line: '{line[:80]}...' - {e}")
                except Exception as e:
                    logger.error(f"Unexpected error parsing size reduction: {e}", exc_info=True)

            # Debug log if a line was processed by a regex
            if processed_line:
                logger.debug(f"Parsed line: '{line[:80]}...' -> Stats: Phase={stats.get('phase')}, Qual={stats.get('progress_quality'):.1f}, Enc={stats.get('progress_encoding'):.1f}, VMAF={stats.get('vmaf')}, CRF={stats.get('crf')}")

        except Exception as e:
            # Catch-all for unexpected errors during line processing
            logger.error(f"General error processing output line: '{line[:80]}...' - {e}", exc_info=True)


    def auto_encode(self, input_path: str, output_path: str,
                    file_info_callback: callable = None,
                    pid_callback: callable = None,
                    total_duration_seconds: float = 0.0) -> dict: # Added total_duration_seconds, removed progress_callback
        """Run ab-av1 auto-encode with VMAF fallback loop.

        This function performs the actual encoding process with automatic VMAF target
        fallback if the initial target cannot be achieved.

        Args:
            input_path: Path to the input video file
            output_path: Path where the output file should be saved
            file_info_callback: Optional callback for reporting file status changes
            pid_callback: Optional callback to receive the process ID
            total_duration_seconds: Total duration of the input video in seconds

        Returns:
            Dictionary containing encoding statistics and results

        Raises:
            InputFileError: If there is a problem with the input file
            OutputFileError: If there is a problem with the output path
            VMAFError: If the VMAF calculation fails
            EncodingError: If the encoding process fails
            AbAv1Error: For other errors
        """
        self.file_info_callback = file_info_callback
        # Use constants from src.config
        preset = DEFAULT_ENCODING_PRESET
        initial_min_vmaf = DEFAULT_VMAF_TARGET

        # --- Input Validation ---
        anonymized_input_path = anonymize_filename(input_path)
        if not os.path.exists(input_path):
            error_msg = f"Input not found: {anonymized_input_path}"; logger.error(error_msg)
            if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"missing_input"})
            raise InputFileError(error_msg, error_type="missing_input")
        try:
            video_info = get_video_info(input_path)
            if not video_info or "streams" not in video_info: raise InputFileError("Invalid video file", error_type="invalid_video")
            if not any(s.get("codec_type") == "video" for s in video_info.get("streams",[])): raise InputFileError("No video stream", error_type="no_video_stream")

            # Get original file size for size reduction calculations
            try:
                original_size = os.path.getsize(input_path)
                stats = {"original_size": original_size}
                logger.info(f"Original file size: {original_size} bytes")
            except Exception as size_e:
                logger.warning(f"Couldn't get original file size: {size_e}")
                stats = {}

        except Exception as e:
            if not isinstance(e, AbAv1Error):
                 error_msg=f"Error analyzing {anonymized_input_path}: {str(e)}"; logger.error(error_msg)
                 if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"analysis_failed"})
                 raise InputFileError(error_msg, error_type="analysis_failed")
            else: raise

        # --- Output Path Setup ---
        if not output_path.lower().endswith('.mkv'): output_path = os.path.splitext(output_path)[0] + '.mkv'
        output_dir = os.path.dirname(output_path)
        try: os.makedirs(output_dir, exist_ok=True)
        except Exception as e:
            error_msg = f"Cannot create output dir: {str(e)}"; logger.error(error_msg)
            if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"output_dir_creation_failed"})
            raise OutputFileError(error_msg, error_type="output_dir_creation_failed")
        temp_output = output_path + ".temp.mkv"
        anonymized_output_path = anonymize_filename(output_path)
        anonymized_temp_output = anonymize_filename(temp_output) # Will likely just be basename

        # --- VMAF Fallback Loop ---
        current_vmaf_target = initial_min_vmaf
        last_error_info = None
        success = False

        # Add more fields to stats
        stats.update({
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
            "last_ffmpeg_fps": None, # Initialize FPS field
            "eta_text": None,        # Initialize ETA field
            "total_duration_seconds": total_duration_seconds, # Store duration
            "last_reported_encoding_progress": -1.0, # Initialize for throttling
            "estimated_output_size": None, # For size estimation
            "estimated_size_reduction": None # For size estimation
        })

        while current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET:
            # Reset progress tracking for new attempt
            stats["phase"] = "crf-search"
            stats["progress_quality"] = 0
            stats["progress_encoding"] = 0
            stats["vmaf"] = None # Reset VMAF from previous failed attempt if any
            stats["last_reported_encoding_progress"] = -1.0
            stats["vmaf_target_used"] = current_vmaf_target # Update target for this attempt
            stats["estimated_output_size"] = None
            stats["estimated_size_reduction"] = None

            logger.info(f"Attempting encode for {anonymized_input_path} with VMAF target: {current_vmaf_target}")

            # --- Command Preparation ---
            cmd = [
                self.executable_path, "auto-encode",
                "-i", input_path, "-o", temp_output,
                "--preset", str(preset),
                "--min-vmaf", str(current_vmaf_target)
                # Add other parameters here if needed
            ]
            cmd_str = " ".join(cmd)
            stats["command"] = cmd_str

            cmd_for_log = [ # Anonymized log version
                os.path.basename(self.executable_path), "auto-encode",
                "-i", os.path.basename(anonymized_input_path),
                "-o", os.path.basename(anonymized_temp_output),
                "--preset", str(preset), "--min-vmaf", str(current_vmaf_target)
            ]
            cmd_str_log = " ".join(cmd_for_log)
            logger.debug(f"Running: {cmd_str_log}"); logger.debug(f"Full cmd: {cmd_str}")

            # Send 'retrying' or 'starting' callback
            if file_info_callback:
                if current_vmaf_target != initial_min_vmaf:
                     file_info_callback(os.path.basename(input_path), "retrying", {
                         "message": f"Retrying with VMAF target: {current_vmaf_target}",
                         "original_vmaf": initial_min_vmaf, "fallback_vmaf": current_vmaf_target,
                         "used_fallback": True
                     })
                elif 'original_size' in stats: # First attempt
                     file_info_callback(os.path.basename(input_path), "starting")


            # --- Process Execution ---
            process = None
            try:
                startupinfo = None;
                if os.name == 'nt': startupinfo = subprocess.STARTUPINFO(); startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW; startupinfo.wShowWindow = subprocess.SW_HIDE

                process = subprocess.Popen(cmd,
                                          stdout=subprocess.PIPE,
                                          stderr=subprocess.STDOUT, # Redirect stderr to stdout
                                          universal_newlines=True,
                                          bufsize=1, # Line buffered
                                          cwd=output_dir,
                                          startupinfo=startupinfo,
                                          encoding='utf-8',
                                          errors='replace')

                if hasattr(process, 'stdout'):
                    try:
                        if hasattr(process.stdout, 'reconfigure'): process.stdout.reconfigure(write_through=True)
                        else: logger.debug("stdout.reconfigure not available")
                    except Exception as e: logger.debug(f"Could not reconfigure stdout: {e}")
            except Exception as e:
                error_msg = f"Failed to start process: {str(e)}"; logger.error(error_msg)
                if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"process_start_failed"})
                raise EncodingError(error_msg, command=cmd_str, error_type="process_start_failed")
            if pid_callback: pid_callback(process.pid)

            # --- Statistics Tracking & Output Parsing ---
            # (State reset moved above before the loop starts for this attempt)

            full_output = []
            log_consolidation_counter = 0 # Counter for less frequent consolidated logging

            # Main output processing loop
            try:
                for line in iter(process.stdout.readline, ""):
                    full_output.append(line)
                    stripped_line = line.strip()

                    # --- FFMPEG Stat Extraction (moved before main parse) ---
                    if stats.get("phase") == "encoding" and stripped_line:
                        try: # Extract fps
                            fps_match = re.search(r'(\d+\.?\d*)\s+fps', stripped_line)
                            if fps_match: stats["last_ffmpeg_fps"] = fps_match.group(1)
                        except Exception as e: logger.warning(f"FPS parse error: {e}")

                        try: # Extract eta
                            eta_match_sec = re.search(r'eta\s+(\d+)\s*s(?:ec(?:onds?)?)?\b', stripped_line, re.IGNORECASE)
                            eta_match_min = re.search(r'eta\s+(\d+)\s*min(?:ute)?s?\b', stripped_line, re.IGNORECASE)
                            eta_match_time = re.search(r'eta\s+(\d+:\d{2}:\d{2})\b', stripped_line, re.IGNORECASE)
                            eta_match_min_sec = re.search(r'eta\s+(\d+:\d{2})\b', stripped_line, re.IGNORECASE)
                            if eta_match_time: stats["eta_text"] = f"{eta_match_time.group(1)}"
                            elif eta_match_min_sec: stats["eta_text"] = f"0:{eta_match_min_sec.group(1)}"
                            elif eta_match_min: stats["eta_text"] = f"{eta_match_min.group(1)} min"
                            elif eta_match_sec: stats["eta_text"] = f"{eta_match_sec.group(1)} sec"
                            else: stats["eta_text"] = None # Clear if not found
                        except Exception as e: logger.warning(f"ETA parse error: {e}")

                        # --- Estimate Size ---
                        temp_output_path = str(temp_output)
                        estimated_final_size = None
                        current_temp_size = None
                        if os.path.exists(temp_output_path):
                            try:
                                current_temp_size = os.path.getsize(temp_output_path)
                                current_enc_progress = stats.get("progress_encoding", 0)
                                if current_temp_size > 0 and current_enc_progress > 1: # Avoid division by zero/small values
                                    estimated_final_size = current_temp_size / (current_enc_progress / 100.0)
                                    stats["estimated_output_size"] = estimated_final_size

                                    if stats.get("original_size", 0) > 0:
                                        reduction = 100.0 - ((estimated_final_size / stats["original_size"]) * 100.0)
                                        stats["estimated_size_reduction"] = reduction

                                    # Send size estimate update via callback (throttled implicitly by progress update)
                                    # This info will be included in the progress callback triggered by _update_stats_from_line
                            except ZeroDivisionError: pass
                            except Exception as e: logger.error(f"Error estimating size: {e}")

                        # --- Consolidated Logging (less frequent) ---
                        log_consolidation_counter += 1
                        if log_consolidation_counter >= 10: # Log roughly every 10 relevant lines
                             self._log_consolidated_progress(stats, current_temp_size, estimated_final_size)
                             log_consolidation_counter = 0


                    # --- Main line parsing for stats and callbacks ---
                    self._update_stats_from_line(line, stats)

                    # --- Error Detection ---
                    if stripped_line and re.search(r'error|failed|invalid', stripped_line.lower()):
                        logger.warning(f"Possible error detected in output line: {stripped_line}")

            except Exception as e:
                logger.error(f"Error reading process output: {e}", exc_info=True)

            # Wait for process to finish
            return_code = process.wait()
            full_output_text = "".join(full_output)

            if return_code == 0:
                success = True; logger.info(f"Encode succeeded for {anonymized_input_path} with VMAF target {current_vmaf_target}"); break

            # --- Error Handling for this attempt ---
            else:
                error_type="unknown"; error_details="Unknown error"
                if re.search(r'ffmpeg.*?:\s*Invalid\s+data\s+found', full_output_text, re.IGNORECASE): error_type="invalid_input_data"; error_details="Invalid data in input"
                elif re.search(r'No\s+such\s+file\s+or\s+directory', full_output_text, re.IGNORECASE): error_type="file_not_found"; error_details="Input not found/inaccessible"
                elif re.search(r'failed\s+to\s+open\s+file', full_output_text, re.IGNORECASE): error_type="file_open_failed"; error_details="Failed to open input"
                elif re.search(r'permission\s+denied', full_output_text, re.IGNORECASE): error_type="permission_denied"; error_details="Permission denied"
                elif re.search(r'vmaf\s+.*?error', full_output_text, re.IGNORECASE): error_type="vmaf_calculation_failed"; error_details="VMAF calculation failed"
                elif re.search(r'encode\s+.*?error', full_output_text, re.IGNORECASE): error_type="encoding_failed"; error_details="Encoding failed"
                elif re.search(r'out\s+of\s+memory', full_output_text, re.IGNORECASE): error_type="memory_error"; error_details="Out of memory"
                elif re.search(r'Failed\s+to\s+find\s+a\s+suitable\s+crf', full_output_text, re.IGNORECASE): error_type = "crf_search_failed"; error_details = f"Could not find suitable CRF for VMAF {current_vmaf_target}"
                elif 'ab-av1.exe' in full_output_text and 'not recognized' in full_output_text: error_type = "executable_not_found"; error_details = "ab-av1.exe command failed (not found or path issue?)"

                last_error_info = {"return_code": return_code, "error_type": error_type, "error_details": error_details, "command": cmd_str_log, "output_tail": ''.join(full_output[-20:])}
                logger.warning(f"Attempt failed for VMAF {current_vmaf_target} (rc={return_code}): {error_details}")

                if error_type == "crf_search_failed":
                    current_vmaf_target -= VMAF_FALLBACK_STEP
                    if current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET: logger.info(f"Retrying {anonymized_input_path} with VMAF target: {current_vmaf_target}"); continue
                    else: logger.error(f"CRF search failed down to VMAF {MIN_VMAF_FALLBACK_TARGET}."); last_error_info["error_details"] = f"CRF search failed down to VMAF {MIN_VMAF_FALLBACK_TARGET}"; break
                else: logger.error(f"Non-recoverable error '{error_type}'. Stopping attempts."); break

        # --- End of Fallback Loop ---

        if not success:
            if last_error_info:
                error_msg = f"ab-av1 failed (rc={last_error_info['return_code']}): {last_error_info['error_details']}"
                logger.error(error_msg); logger.error(f"Last Cmd: {last_error_info['command']}"); logger.error(f"Last Output tail:\n{last_error_info['output_tail']}")
                if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg, "type":last_error_info['error_type'], "details":last_error_info['error_details'], "command":last_error_info['command']})
                error_type = last_error_info['error_type'] # Raise based on last error
                if error_type in ["invalid_input_data", "file_not_found", "file_open_failed", "no_video_stream", "analysis_failed"]: raise InputFileError(error_msg, command=last_error_info['command'], output=full_output_text, error_type=error_type)
                elif error_type == "vmaf_calculation_failed": raise VMAFError(error_msg, command=last_error_info['command'], output=full_output_text, error_type=error_type)
                elif error_type in ["encoding_failed", "memory_error", "crf_search_failed", "executable_not_found", "permission_denied"]: raise EncodingError(error_msg, command=last_error_info['command'], output=full_output_text, error_type=error_type)
                else: raise AbAv1Error(error_msg, command=last_error_info['command'], output=full_output_text, error_type=error_type)
            else:
                generic_error_msg = f"Encode failed for {anonymized_input_path} unknown reasons."; logger.error(generic_error_msg)
                if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":generic_error_msg, "type":"unknown_loop_error"})
                raise AbAv1Error(generic_error_msg)

        # --- Success Path ---
        logger.info(f"ab-av1 completed successfully for {anonymized_input_path} (used VMAF target {stats.get('vmaf_target_used', '?')})")
        self._parse_final_output(full_output_text, stats) # Ensure final stats parsed
        if stats["crf"] is not None: logger.info(f"Final CRF: {stats['crf']}")
        if stats["vmaf"] is not None: logger.info(f"Final VMAF: {stats['vmaf']:.2f}")
        if stats["size_reduction"] is not None: logger.info(f"Final Size reduction: {stats['size_reduction']:.2f}%")

        # Move temp file
        try:
            if os.path.exists(output_path): logger.warning(f"Overwriting: {anonymized_output_path}"); os.remove(output_path)
            shutil.move(temp_output, output_path); logger.info(f"Moved temp to final: {anonymized_output_path}")
        except Exception as e:
            error_msg=f"Failed move {os.path.basename(anonymized_temp_output)} to {os.path.basename(anonymized_output_path)}: {str(e)}"; logger.error(error_msg)
            if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg, "type":"rename_failed"})
            cleaned_count = clean_ab_av1_temp_folders(output_dir);
            if cleaned_count > 0: logger.info(f"Cleaned {cleaned_count} temp folders after rename failure.")
            raise OutputFileError(error_msg, command=cmd_str_log, error_type="rename_failed")

        # Send completion update
        if file_info_callback:
            final_stats_for_callback = {
                "message":f"Complete (VMAF {stats.get('vmaf','N/A'):.2f} @ Target {stats.get('vmaf_target_used','?')})",
                "vmaf":stats.get("vmaf"), "crf":stats.get("crf"),
                "vmaf_target_used": stats.get('vmaf_target_used'),
                "size_reduction": stats.get('size_reduction'),
                "output_path": output_path
                }
            try:
                 final_size = os.path.getsize(output_path)
                 final_stats_for_callback["output_size"] = final_size
            except Exception: pass # Ignore errors getting final size here

            file_info_callback(os.path.basename(input_path), "completed", final_stats_for_callback)

        # Clean temp folders on success
        cleaned_count = clean_ab_av1_temp_folders(output_dir);
        if cleaned_count > 0: logger.info(f"Cleaned {cleaned_count} temp folders.")
        self.file_info_callback = None # Clear callback reference
        return stats


    def _parse_final_output(self, output_text: str, stats: dict) -> None:
        """Extract final statistics from the complete output if not found earlier.

        Args:
            output_text: The complete console output text from ab-av1
            stats: Dictionary to update with extracted information
        """
        logger.debug("Running final output parsing as fallback/verification.")
        # Check for VMAF (find the LAST occurrence)
        try:
            vmaf_matches = re.findall(r'VMAF:\s+(\d+\.\d+)', output_text, re.IGNORECASE)
            if vmaf_matches:
                final_vmaf = float(vmaf_matches[-1])
                # Only overwrite if current value is None or different
                if stats.get("vmaf") is None or abs(stats.get("vmaf", 0.0) - final_vmaf) > 0.01:
                    logger.info(f"Final VMAF extracted/verified: {final_vmaf:.2f} (overwriting {stats.get('vmaf')})")
                    stats["vmaf"] = final_vmaf
            elif stats.get("vmaf") is None:
                 logger.warning("Could not find VMAF score in final output.")
        except (ValueError, IndexError, TypeError) as e:
             logger.warning(f"Error parsing final VMAF score: {e}")

        # Check for CRF (find the LAST occurrence of 'Best CRF')
        try:
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

        # Check for Size Reduction (find the LAST occurrence of percentage)
        try:
            size_percent_matches = re.findall(r'Output\s+size:.*?\((\d+\.?\d*)\s*%\s+of\s+source\)', output_text, re.IGNORECASE)
            if size_percent_matches:
                final_size_percent = float(size_percent_matches[-1])
                final_reduction = 100.0 - final_size_percent
                # Compare with possible float precision issues
                if stats.get("size_reduction") is None or abs(stats.get("size_reduction", -1) - final_reduction) > 0.01:
                     logger.info(f"Final size reduction extracted/verified: {final_reduction:.2f}% (overwriting {stats.get('size_reduction')})")
                     stats["size_reduction"] = final_reduction
            elif stats.get("size_reduction") is None:
                logger.debug("Size reduction percentage not found in final output, trying absolute sizes...")
                input_size_match = re.search(r'Input\s+size:\s+(\d+\.?\d*)\s+(\w+)', output_text, re.IGNORECASE)
                output_size_match = re.search(r'Output\s+size:\s+(\d+\.?\d*)\s+(\w+)', output_text, re.IGNORECASE)

                if input_size_match and output_size_match:
                    try:
                        input_size = float(input_size_match.group(1))
                        input_unit = input_size_match.group(2).upper()
                        output_size = float(output_size_match.group(1))
                        output_unit = output_size_match.group(2).upper()
                        unit_multipliers = {'B':1, 'KB':1024, 'MB':1024**2, 'GB':1024**3, 'TB':1024**4}
                        input_bytes = input_size * unit_multipliers.get(input_unit, 1)
                        output_bytes = output_size * unit_multipliers.get(output_unit, 1)
                        if input_bytes > 0:
                            calculated_reduction = 100.0 * (1.0 - (output_bytes / input_bytes))
                            logger.info(f"Final size reduction calculated from sizes: {calculated_reduction:.2f}%")
                            stats["size_reduction"] = calculated_reduction
                        else: logger.warning("Input size is zero, cannot calculate reduction.")
                    except (ValueError, KeyError, TypeError, IndexError) as calc_e:
                        logger.warning(f"Could not calculate size reduction from final sizes: {calc_e}")
                else:
                    logger.warning("Could not find size reduction percentage or absolute sizes in final output.")

        except (ValueError, IndexError, TypeError) as e:
            logger.warning(f"Error parsing final size reduction: {e}")

   # --- Helper functions (unchanged) ---

def clean_ab_av1_temp_folders(base_dir: str = None) -> int:
    """Clean up temporary folders created by ab-av1.


    Args:
        base_dir: Directory to search for temp folders. Defaults to current working directory.

    Returns:
        Number of temporary folders cleaned up
    """
    # Determine base directory
    if base_dir is None:
        base_dir = os.getcwd()
        logger.debug(f"Cleaning temp folders in cwd: {base_dir}")
    else:
        logger.debug(f"Cleaning temp folders in: {base_dir}")

    # Find temp folders
    try:
        base_path = Path(base_dir)
        if not base_path.is_dir():
            logger.warning(f"Base dir invalid: {base_dir}")
            return 0

        pattern = ".ab-av1-*"
        temp_folders = list(base_path.glob(pattern))
        logger.debug(f"Found {len(temp_folders)} potential temp items in {base_dir}")
    except Exception as e:
        logger.error(f"Error finding temp folders in {base_dir}: {e}")
        return 0

    # Remove the found folders
    cleaned_count = 0
    for item in temp_folders:
        try:
            if item.is_dir():
                shutil.rmtree(item)
                logger.info(f"Cleaned temp folder: {item}")
                cleaned_count += 1
            else:
                logger.debug(f"Skipping non-dir item: {item}")
        except Exception as e:
            logger.warning(f"Failed cleanup {item}: {str(e)}")

    return cleaned_count

def check_ab_av1_available() -> tuple:
    """Check if ab-av1 executable is available.
        Returns:
        Tuple of (is_available, path, message) where:
        - is_available: Boolean indicating whether ab-av1 is available
        - path: Path to the ab-av1 executable
        - message: Descriptive message about the result
    """
    app_dir = os.path.dirname(os.path.abspath(__file__))
    expected_path = os.path.join(app_dir, "ab-av1.exe")

    if os.path.exists(expected_path):
        logger.info(f"ab-av1 found: {expected_path}")
        return True, expected_path, f"ab-av1 available at {expected_path}"
    else:
        # Adjusted error message
        error_msg = f"ab-av1.exe not found. Place inside 'src' dir.\nExpected: {expected_path}"
        logger.error(error_msg)
        return False, expected_path, error_msg