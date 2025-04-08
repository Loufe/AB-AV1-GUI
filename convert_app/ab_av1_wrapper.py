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
from convert_app.utils import get_video_info, anonymize_filename, DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET

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
    """Wrapper for the ab-av1 tool"""

    def __init__(self):
        app_dir = os.path.dirname(os.path.abspath(__file__))
        self.executable_path = os.path.join(app_dir, "ab-av1.exe")
        logger.debug(f"AbAv1Wrapper init - expecting executable at: {self.executable_path}")
        self._verify_executable()
        self.file_info_callback = None

    def _verify_executable(self):
        if not os.path.exists(self.executable_path):
            error_msg = (f"ab-av1.exe not found. Place in 'convert_app' dir.\nExpected: {self.executable_path}")
            logger.error(error_msg); raise FileNotFoundError(error_msg)
        logger.debug(f"AbAv1Wrapper init - verified: {self.executable_path}"); return True

    def _update_stats_from_line(self, line, stats):
        """Update statistics based on a line of output for dual progress bars"""
        # (This function remains unchanged from the previous version)
        line = line.strip()
        anonymized_input_basename = os.path.basename(stats.get("input_path", "unknown_file"))
        current_phase = stats.get("phase", "crf-search")
        progress_quality = stats.get("progress_quality", 0)
        progress_encoding = stats.get("progress_encoding", 0)
        phase_transition_match = re.search(r'ab_av1::command::encode\].*encoding', line, re.IGNORECASE)
        if phase_transition_match and current_phase == "crf-search":
            logger.info(f"Phase transition to Encoding for {anonymize_filename(stats.get('input_path', ''))}")
            stats["phase"] = "encoding"; stats["progress_quality"] = 100.0; stats["progress_encoding"] = 0.0
            if self.file_info_callback:
                self.file_info_callback(anonymized_input_basename, "progress", {
                    "progress_quality":100.0, "progress_encoding":0.0, "message":"Encoding started",
                    "phase":stats["phase"], "vmaf":stats["vmaf"], "crf":stats["crf"], "size_reduction":stats["size_reduction"]})
            return
        if current_phase == "crf-search":
            new_quality_progress = progress_quality
            crf_vmaf_match = re.search(r'crf\s+(\d+)\s+VMAF\s+(\d+\.\d+)', line, re.IGNORECASE)
            if crf_vmaf_match:
                stats["crf"] = int(crf_vmaf_match.group(1)); stats["vmaf"] = float(crf_vmaf_match.group(2))
                logger.debug(f"CRF search update: CRF={stats['crf']}, VMAF={stats['vmaf']:.2f}")
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
                         "phase":current_phase, "vmaf":stats["vmaf"], "crf":stats["crf"], "size_reduction":stats["size_reduction"]})
        elif current_phase == "encoding":
            progress_match = re.match(r'^\s*(\d{1,3}(?:\.\d+)?)\s*%\s*,\s*\d+.*fps', line)
            if progress_match:
                try:
                    encoding_percent = float(progress_match.group(1))
                    stats["progress_encoding"] = max(0.0, min(100.0, encoding_percent)); stats["progress_quality"] = 100.0
                    logger.debug(f"Encoding progress: {stats['progress_encoding']:.1f}%")
                    if self.file_info_callback:
                        self.file_info_callback(anonymized_input_basename, "progress", {
                            "progress_quality":100.0, "progress_encoding":stats["progress_encoding"],
                            "message":f"Encoding: {stats['progress_encoding']:.1f}%", "phase":current_phase,
                            "vmaf":stats["vmaf"], "crf":stats["crf"], "size_reduction":stats["size_reduction"]})
                except ValueError: logger.warning(f"Cannot parse encoding progress: {progress_match.group(1)}")
            size_match = re.search(r'\((\d+\.\d+)%\s+of\s+source\)', line)
            if size_match:
                try: stats["size_reduction"] = 100.0 - float(size_match.group(1)); logger.debug(f"Size reduction update: {stats['size_reduction']:.1f}%")
                except ValueError: logger.warning(f"Cannot parse size reduction: {size_match.group(1)}")


    def auto_encode(self, input_path, output_path,
                    progress_callback=None, file_info_callback=None, pid_callback=None):
        """Run ab-av1 auto-encode with VMAF fallback loop."""
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
        stats = {}

        while current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET:
            logger.info(f"Attempting encode for {anonymized_input_path} with VMAF target: {current_vmaf_target}")

            # --- Command Preparation (NO --no-cache flag) ---
            cmd = [
                self.executable_path, "auto-encode",
                "-i", input_path, "-o", temp_output,
                "--preset", str(preset),
                "--min-vmaf", str(current_vmaf_target)
                # Removed "--no-cache"
            ]
            cmd_str = " ".join(cmd)
            cmd_for_log = [ # Anonymized log version
                os.path.basename(self.executable_path), "auto-encode",
                "-i", os.path.basename(anonymized_input_path), # Use basename from anonymized
                "-o", os.path.basename(anonymized_temp_output), # Use basename from anonymized
                "--preset", str(preset), "--min-vmaf", str(current_vmaf_target)
                # Removed "--no-cache"
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
                process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, universal_newlines=True, bufsize=1, cwd=output_dir, startupinfo=startupinfo, encoding='utf-8')
            except Exception as e:
                error_msg = f"Failed to start process: {str(e)}"; logger.error(error_msg)
                if file_info_callback: file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"process_start_failed"})
                raise EncodingError(error_msg, command=cmd_str, error_type="process_start_failed")
            if pid_callback: pid_callback(process.pid)

            # --- Statistics Tracking & Output Parsing ---
            stats = {"phase":"crf-search", "progress_quality":0, "progress_encoding":0, "vmaf":None, "crf":None, "size_reduction":None,
                     "input_path":input_path, "output_path":output_path, "command":cmd_str, "vmaf_target_used": current_vmaf_target}
            full_output = []
            for line in iter(process.stdout.readline, ""):
                full_output.append(line); logger.debug(f"ab-av1: {line.strip()}")
                if re.search(r'error|failed|invalid', line.lower()): logger.warning(f"Possible error: {line.strip()}")
                self._update_stats_from_line(line, stats)

            # --- Process Completion Check ---
            return_code = process.wait(); full_output_text = "".join(full_output)

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


    def _parse_final_output(self, output_text, stats):
        """Extract final statistics from the complete output if not found earlier"""
        # (Unchanged)
        if stats.get("vmaf") is None:
            vmaf_matches = re.findall(r'VMAF:\s+(\d+\.\d+)', output_text, re.IGNORECASE)
            if vmaf_matches: stats["vmaf"] = float(vmaf_matches[-1]); logger.info(f"Final VMAF extracted: {stats['vmaf']:.2f}")
        if stats.get("crf") is None:
            crf_match = re.search(r'Best CRF: (\d+)', output_text)
            if crf_match: stats["crf"] = int(crf_match.group(1)); logger.info(f"Final CRF extracted: {stats['crf']}")
        input_size_match = re.search(r'Input size:\s+(\d+\.\d+)\s+(\w+)', output_text)
        output_size_match = re.search(r'Output size:\s+(\d+\.\d+)\s+(\w+)', output_text)
        size_percent_match = re.search(r'Output size:.*\((\d+\.\d+)%\s+of\s+source\)', output_text)
        if size_percent_match:
             size_percent = float(size_percent_match.group(1)); stats["size_reduction"] = 100.0 - size_percent; logger.info(f"Final size reduction extracted: {stats['size_reduction']:.2f}%")
        elif input_size_match and output_size_match and stats.get("size_reduction") is None:
            try:
                input_size=float(input_size_match.group(1)); input_unit=input_size_match.group(2).upper()
                output_size=float(output_size_match.group(1)); output_unit=output_size_match.group(2).upper()
                unit_multipliers={'B':1,'KB':1024,'MB':1024**2,'GB':1024**3,'TB':1024**4}
                input_bytes=input_size*unit_multipliers.get(input_unit,1); output_bytes=output_size*unit_multipliers.get(output_unit,1)
                if input_bytes > 0: stats["size_reduction"]=100.0*(1.0-(output_bytes/input_bytes)); logger.info(f"Final size reduction calculated: {stats['size_reduction']:.2f}%")
            except Exception as e: logger.warning(f"Could not calculate size reduction from final output: {e}")


# --- Helper functions (unchanged) ---
def clean_ab_av1_temp_folders(base_dir=None):
    if base_dir is None: base_dir = os.getcwd(); logger.debug(f"Cleaning temp folders in cwd: {base_dir}")
    else: logger.debug(f"Cleaning temp folders in: {base_dir}")
    try:
        base_path = Path(base_dir);
        if not base_path.is_dir(): logger.warning(f"Base dir invalid: {base_dir}"); return 0
        pattern = ".ab-av1-*"; temp_folders = list(base_path.glob(pattern)); logger.debug(f"Found {len(temp_folders)} potential temp items in {base_dir}")
    except Exception as e: logger.error(f"Error finding temp folders in {base_dir}: {e}"); return 0
    cleaned_count = 0
    for item in temp_folders:
        try:
            if item.is_dir(): shutil.rmtree(item); logger.info(f"Cleaned temp folder: {item}"); cleaned_count += 1
            else: logger.debug(f"Skipping non-dir item: {item}")
        except Exception as e: logger.warning(f"Failed cleanup {item}: {str(e)}")
    return cleaned_count

def check_ab_av1_available():
    app_dir = os.path.dirname(os.path.abspath(__file__)); expected_path = os.path.join(app_dir, "ab-av1.exe")
    if os.path.exists(expected_path): logger.info(f"ab-av1 found: {expected_path}"); return True, expected_path, f"ab-av1 available at {expected_path}"
    else: error_msg = (f"ab-av1.exe not found. Place in 'convert_app' dir.\nExpected: {expected_path}"); logger.error(error_msg); return False, expected_path, error_msg