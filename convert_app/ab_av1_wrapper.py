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
# Use constants from utils
from convert_app.utils import get_video_info, anonymize_filename, DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET, format_file_size

logger = logging.getLogger(__name__)

# --- Constants for Fallback ---
MIN_VMAF_FALLBACK_TARGET = 90
VMAF_FALLBACK_STEP = 1

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
            error_msg = (f"ab-av1.exe not found. Place in 'convert_app' dir.\nExpected: {self.executable_path}")
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
        """Update statistics based on a line of output for dual progress bars"""
        line = line.strip()
        try:
            anonymized_input_basename = os.path.basename(stats.get("input_path", "unknown_file"))
            current_phase = stats.get("phase", "crf-search")
            progress_quality = stats.get("progress_quality", 0)
            progress_encoding = stats.get("progress_encoding", 0)

            # PHASE TRANSITION DETECTION
            phase_transition_match = re.search(r'ab_av1::command::encode\].*encoding', line, re.IGNORECASE)
            if phase_transition_match and current_phase == "crf-search":
                logger.info(f"Phase transition to Encoding for {anonymize_filename(stats.get('input_path', ''))}")
                stats["phase"] = "encoding"; stats["progress_quality"] = 100.0; stats["progress_encoding"] = 0.0
                if self.file_info_callback:
                    self.file_info_callback(anonymized_input_basename, "progress", {
                        "progress_quality":100.0, "progress_encoding":0.0, "message":"Encoding started",
                        "phase":stats["phase"], "vmaf":stats["vmaf"], "crf":stats["crf"], 
                        "size_reduction":stats["size_reduction"],
                        "original_size": stats.get("original_size")
                    })
                return

            # CRF SEARCH PHASE
            if current_phase == "crf-search":
                new_quality_progress = progress_quality
                crf_vmaf_match = re.search(r'crf\s+(\d+)\s+VMAF\s+(\d+\.\d+)', line, re.IGNORECASE)
                if crf_vmaf_match:
                    stats["crf"] = int(crf_vmaf_match.group(1)); stats["vmaf"] = float(crf_vmaf_match.group(2))
                    logger.info(f"CRF search update: CRF={stats['crf']}, VMAF={stats['vmaf']:.2f}")
                    new_quality_progress = min(90.0, progress_quality + 10.0)
                best_crf_match = re.search(r'Best CRF:\s+(\d+)', line, re.IGNORECASE)
                if best_crf_match:
                     stats["crf"] = int(best_crf_match.group(1)); logger.info(f"Best CRF determined: {stats['crf']}")
                     new_quality_progress = 95.0
                if new_quality_progress > progress_quality:
                     stats["progress_quality"] = new_quality_progress
                     if self.file_info_callback:
                         self.file_info_callback(anonymized_input_basename, "progress", {
                             "progress_quality":stats["progress_quality"], "progress_encoding":0,
                             "message":f"Detecting Quality (CRF:{stats.get('crf','?')}, VMAF:{stats.get('vmaf','?'):.1f})",
                             "phase":current_phase, "vmaf":stats["vmaf"], "crf":stats["crf"], 
                             "size_reduction":stats["size_reduction"],
                             "original_size": stats.get("original_size")
                         })

            # ENCODING PHASE
            elif current_phase == "encoding":
                # Check for progress percentage update
                progress_match = re.match(r'^\s*(\d{1,3}(?:\.\d+)?)\s*%\s*,\s*\d+.*fps', line)
                if progress_match:
                    try:
                        encoding_percent = float(progress_match.group(1))
                        stats["progress_encoding"] = max(0.0, min(100.0, encoding_percent))
                        stats["progress_quality"] = 100.0
                        # Log at INFO level for console visibility
                        logger.info(f"Encoding progress: {stats['progress_encoding']:.1f}%")
                        
                        # IMPORTANT: Always include all available stats in callback
                        callback_data = {
                            "progress_quality":100.0, 
                            "progress_encoding":stats["progress_encoding"],
                            "message":f"Encoding: {stats['progress_encoding']:.1f}%", 
                            "phase":current_phase,
                            "original_size": stats.get("original_size"),
                            "vmaf": stats.get("vmaf"),
                            "crf": stats.get("crf")
                        }
                        
                        # Add size reduction info if available
                        if "size_reduction" in stats and stats["size_reduction"] is not None:
                            callback_data["size_reduction"] = stats["size_reduction"]
                            # Calculate estimated output size if we have the original size
                            if "original_size" in stats and stats["original_size"]:
                                size_percentage = 100.0 - stats["size_reduction"]
                                output_size = stats["original_size"] * (size_percentage / 100.0)
                                callback_data["output_size"] = output_size
                                logger.debug(f"Added output_size to callback: {output_size}")
                        
                        if self.file_info_callback:
                            logger.debug(f"Sending progress callback: {callback_data}")
                            self.file_info_callback(anonymized_input_basename, "progress", callback_data)
                    except ValueError: 
                        logger.warning(f"Cannot parse encoding progress: {progress_match.group(1)}")
                
                # Check for size information
                size_match = re.search(r'\((\d+\.\d+)%\s+of\s+source\)', line)
                if size_match:
                    try: 
                        size_percentage = float(size_match.group(1))
                        stats["size_reduction"] = 100.0 - size_percentage
                        # Log at INFO level for better visibility
                        logger.info(f"Size reduction update: {stats['size_reduction']:.1f}% (output: {size_percentage:.1f}% of original)")
                        
                        # Send a dedicated update with this new size information
                        if "original_size" in stats and stats["original_size"] and self.file_info_callback:
                            original_size = stats["original_size"]
                            output_size_estimate = original_size * (size_percentage / 100.0)
                            
                            # Send update with size info
                            self.file_info_callback(anonymized_input_basename, "progress", {
                                "progress_quality": 100.0,
                                "progress_encoding": stats["progress_encoding"],
                                "message": f"Encoding: {stats['progress_encoding']:.1f}%",
                                "phase": current_phase,
                                "vmaf": stats.get("vmaf"),
                                "crf": stats.get("crf"),
                                "size_reduction": stats["size_reduction"],
                                "output_size": output_size_estimate,
                                "original_size": original_size
                            })
                    except ValueError: 
                        logger.warning(f"Cannot parse size reduction: {size_match.group(1)}")
        except Exception as e:
            logger.error(f"Error processing output line: {e} (line: {line[:50]}...)")


    def auto_encode(self, input_path: str, output_path: str,
                    progress_callback: callable = None, file_info_callback: callable = None, 
                    pid_callback: callable = None) -> dict:
        """Run ab-av1 auto-encode with VMAF fallback loop.
        
        This function performs the actual encoding process with automatic VMAF target
        fallback if the initial target cannot be achieved.
        
        Args:
            input_path: Path to the input video file
            output_path: Path where the output file should be saved
            progress_callback: Optional callback for reporting progress (unused but kept for API compatibility)
            file_info_callback: Optional callback for reporting file status changes
            pid_callback: Optional callback to receive the process ID
            
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
            "vmaf_target_used": current_vmaf_target
        })

        while current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET:
            logger.info(f"Attempting encode for {anonymized_input_path} with VMAF target: {current_vmaf_target}")

            # --- Command Preparation ---
            cmd = [
                self.executable_path, "auto-encode",
                "-i", input_path, "-o", temp_output,
                "--preset", str(preset),
                "--min-vmaf", str(current_vmaf_target)

            ]
            cmd_str = " ".join(cmd)
            stats["command"] = cmd_str
            
            cmd_for_log = [ # Anonymized log version
                os.path.basename(self.executable_path), "auto-encode",
                "-i", os.path.basename(anonymized_input_path), # Use basename from anonymized
                "-o", os.path.basename(anonymized_temp_output), # Use basename from anonymized
                "--preset", str(preset), "--min-vmaf", str(current_vmaf_target)

            ]
            cmd_str_log = " ".join(cmd_for_log)
            logger.debug(f"Running: {cmd_str_log}"); logger.debug(f"Full cmd: {cmd_str}")
            if file_info_callback and current_vmaf_target != initial_min_vmaf:
                 file_info_callback(os.path.basename(input_path), "retrying", {
                     "message": f"Retrying with VMAF target: {current_vmaf_target}",
                     "original_vmaf": initial_min_vmaf, "fallback_vmaf": current_vmaf_target
                 })
            elif file_info_callback and not stats:
                 file_info_callback(os.path.basename(input_path), "starting")

            # --- Process Execution ---
            process = None
            try:
                startupinfo = None;
                if os.name == 'nt': startupinfo = subprocess.STARTUPINFO(); startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW; startupinfo.wShowWindow = subprocess.SW_HIDE
                
                # CRITICAL CHANGE: Capture both stdout and stderr
                process = subprocess.Popen(cmd, 
                                          stdout=subprocess.PIPE, 
                                          stderr=subprocess.STDOUT, # Redirect stderr to stdout
                                          universal_newlines=True, 
                                          bufsize=1, # Line buffered
                                          cwd=output_dir, 
                                          startupinfo=startupinfo, 
                                          encoding='utf-8')
                
                # Tell process use unbuffered output if possible
                if hasattr(process, 'stdout'):
                    try:
                        if hasattr(process.stdout, 'reconfigure'):
                            process.stdout.reconfigure(write_through=True)
                        else:
                            logger.debug("stdout.reconfigure not available in this Python version")
                    except Exception as e:
                        logger.debug(f"Could not reconfigure stdout: {e}")
            except Exception as e:
                error_msg = f"Failed to start process: {str(e)}"; logger.error(error_msg)
                if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"process_start_failed"})
                raise EncodingError(error_msg, command=cmd_str, error_type="process_start_failed")
            if pid_callback: pid_callback(process.pid)

            # --- Statistics Tracking & Output Parsing ---
            stats["phase"] = "crf-search"
            stats["progress_quality"] = 0
            stats["progress_encoding"] = 0
            stats["vmaf"] = None
            stats["crf"] = None
            stats["size_reduction"] = None
            stats["vmaf_target_used"] = current_vmaf_target
            
            full_output = []
            
            # Main output processing loop
            try:
                for line in iter(process.stdout.readline, ""):
                    # Store the full output
                    full_output.append(line) 
                    
                    # Process the line for progress updates
                    stripped_line = line.strip()
                    
                    # Log all non-empty lines at INFO level during encoding with progress info
                    if stats.get("phase") == "encoding" and stripped_line and "%" in stripped_line:
                        logger.info(f"[FFMPEG] {stripped_line}")
                    elif stripped_line:  # Log other non-empty lines at DEBUG
                        logger.debug(f"ab-av1: {stripped_line}")
                        
                    # Flag errors
                    if re.search(r'error|failed|invalid', stripped_line.lower()): 
                        logger.warning(f"Possible error: {stripped_line}")
                    
                    # Use broader pattern matching for encoding progress
                    if stats.get("phase") == "encoding" and re.search(r'\d+(\.\d+)?%', stripped_line):
                        progress_parts = re.search(r'(\d+(\.\d+)?)%', stripped_line)
                        if progress_parts:
                            try:
                                encoding_progress = float(progress_parts.group(1))
                                logger.info(f"Detected progress: {encoding_progress}%")
                                # Update the progress state
                                stats["progress_encoding"] = encoding_progress
                                
                                # Check if temp output file exists and get its size for estimation
                                temp_output_path = str(temp_output)
                                if os.path.exists(temp_output_path):
                                    current_temp_size = os.path.getsize(temp_output_path)
                                    if current_temp_size > 0 and encoding_progress > 0:
                                        # Estimate final size based on current progress
                                        estimated_final_size = current_temp_size / (encoding_progress / 100.0)
                                        logger.info(f"Current partial output: {format_file_size(current_temp_size)}, estimated final: {format_file_size(estimated_final_size)}")
                                        
                                        # Update callback with this size information
                                        if self.file_info_callback and "original_size" in stats:
                                            original_size = stats["original_size"]
                                            # Calculate size reduction based on estimate
                                            if original_size > 0:
                                                reduction_percent = 100.0 - ((estimated_final_size / original_size) * 100.0)
                                                stats["estimated_output_size"] = estimated_final_size
                                                stats["estimated_size_reduction"] = reduction_percent
                                                
                                                # Send update specifically for size information
                                                self.file_info_callback(os.path.basename(stats.get("input_path", "unknown")), "progress", {
                                                    "phase": "encoding",
                                                    "progress_encoding": encoding_progress,
                                                    "output_size": estimated_final_size,
                                                    "original_size": original_size,
                                                    "size_reduction": reduction_percent,
                                                    "is_estimate": True
                                                })
                            except Exception as e:
                                logger.debug(f"Error processing progress: {e}")
                    
                    # Update statistics based on this line
                    self._update_stats_from_line(line, stats)
            except Exception as e:
                logger.error(f"Error reading process output: {e}")
                
            # Wait for process to finish
            return_code = process.wait()
            full_output_text = "".join(full_output)

            if return_code == 0:
                success = True; logger.info(f"Encode succeeded for {anonymized_input_path} with VMAF target {current_vmaf_target}"); break

            # --- Error Handling for this attempt ---
            else:
                error_type="unknown"; error_details="Unknown error"
                if re.search(r'ffmpeg.*?: Invalid\s+data\s+found', full_output_text, re.IGNORECASE): error_type="invalid_input_data"; error_details="Invalid data in input"
                elif re.search(r'No\s+such\s+file\s+or\s+directory', full_output_text, re.IGNORECASE): error_type="file_not_found"; error_details="Input not found/inaccessible"
                elif re.search(r'failed\s+to\s+open\s+file', full_output_text, re.IGNORECASE): error_type="file_open_failed"; error_details="Failed to open input"
                elif re.search(r'permission\s+denied', full_output_text, re.IGNORECASE): error_type="permission_denied"; error_details="Permission denied"
                elif re.search(r'vmaf.*?error', full_output_text, re.IGNORECASE): error_type="vmaf_calculation_failed"; error_details="VMAF calculation failed"
                elif re.search(r'encode.*?error', full_output_text, re.IGNORECASE): error_type="encoding_failed"; error_details="Encoding failed"
                elif re.search(r'out\s+of\s+memory', full_output_text, re.IGNORECASE): error_type="memory_error"; error_details="Out of memory"
                elif re.search(r'Failed\s+to\s+find\s+a\s+suitable\s+crf', full_output_text, re.IGNORECASE):
                    error_type = "crf_search_failed"; error_details = f"Could not find suitable CRF for VMAF {current_vmaf_target}"

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
                elif error_type in ["encoding_failed", "memory_error", "crf_search_failed"]: raise EncodingError(error_msg, command=last_error_info['command'], output=full_output_text, error_type=error_type)
                else: raise AbAv1Error(error_msg, command=last_error_info['command'], output=full_output_text, error_type=error_type)
            else:
                generic_error_msg = f"Encode failed for {anonymized_input_path} unknown reasons."; logger.error(generic_error_msg)
                if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":generic_error_msg, "type":"unknown_loop_error"})
                raise AbAv1Error(generic_error_msg)

        # --- Success Path ---
        logger.info(f"ab-av1 completed successfully for {anonymized_input_path} (used VMAF target {stats.get('vmaf_target_used', '?')})")
        self._parse_final_output(full_output_text, stats)
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
            raise OutputFileError(error_msg, command=cmd_str_log, error_type="rename_failed")

        # Send completion update
        if file_info_callback:
            final_stats_for_callback = {
                "message":f"Complete (VMAF {stats.get('vmaf','N/A'):.2f} @ Target {stats.get('vmaf_target_used','?')})",
                "vmaf":stats.get("vmaf"), "crf":stats.get("crf"),
                "vmaf_target_used": stats.get('vmaf_target_used')}
            file_info_callback(os.path.basename(input_path), "completed", final_stats_for_callback)

        cleaned_count = clean_ab_av1_temp_folders(output_dir);
        if cleaned_count > 0: logger.info(f"Cleaned {cleaned_count} temp folders.")
        self.file_info_callback = None
        return stats


    def _parse_final_output(self, output_text: str, stats: dict) -> None:
        """Extract final statistics from the complete output if not found earlier.
        
        Args:
            output_text: The complete console output text from ab-av1
            stats: Dictionary to update with extracted information
        """
        # Check for VMAF
        if stats.get("vmaf") is None:
            vmaf_matches = re.findall(r'VMAF:\s+(\d+\.\d+)', output_text, re.IGNORECASE)
            if vmaf_matches: 
                stats["vmaf"] = float(vmaf_matches[-1])
                logger.info(f"Final VMAF extracted: {stats['vmaf']:.2f}")
                
        # Check for CRF
        if stats.get("crf") is None:
            crf_match = re.search(r'Best CRF: (\d+)', output_text)
            if crf_match: 
                stats["crf"] = int(crf_match.group(1))
                logger.info(f"Final CRF extracted: {stats['crf']}")
                
        # Get input and output size information
        input_size_match = re.search(r'Input size:\s+(\d+\.\d+)\s+(\w+)', output_text)
        output_size_match = re.search(r'Output size:\s+(\d+\.\d+)\s+(\w+)', output_text)
        size_percent_match = re.search(r'Output size:.*\((\d+\.\d+)%\s+of\s+source\)', output_text)
        
        # Parse size reduction percentage
        if size_percent_match:
            size_percent = float(size_percent_match.group(1))
            stats["size_reduction"] = 100.0 - size_percent
            logger.info(f"Final size reduction extracted: {stats['size_reduction']:.2f}%")
        # Try to calculate it from sizes if available
        elif input_size_match and output_size_match and stats.get("size_reduction") is None:
            try:
                # Parse size values
                input_size = float(input_size_match.group(1))
                input_unit = input_size_match.group(2).upper()
                output_size = float(output_size_match.group(1))
                output_unit = output_size_match.group(2).upper()
                
                # Convert to bytes
                unit_multipliers = {'B':1, 'KB':1024, 'MB':1024**2, 'GB':1024**3, 'TB':1024**4}
                input_bytes = input_size * unit_multipliers.get(input_unit, 1)
                output_bytes = output_size * unit_multipliers.get(output_unit, 1)
                
                # Calculate reduction percentage
                if input_bytes > 0:
                    stats["size_reduction"] = 100.0 * (1.0 - (output_bytes/input_bytes))
                    logger.info(f"Final size reduction calculated: {stats['size_reduction']:.2f}%")
            except Exception as e:
                logger.warning(f"Could not calculate size reduction from final output: {e}")


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
        error_msg = f"ab-av1.exe not found. Place in 'convert_app' dir.\nExpected: {expected_path}"
        logger.error(error_msg)
        return False, expected_path, error_msg