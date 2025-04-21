# src/ab_av1/wrapper.py
"""
Wrapper class for the ab-av1 tool in the AV1 Video Converter application.

Handles executing ab-av1, managing the process, VMAF fallback,
and coordinating with the parser for output analysis.
"""
import os
import subprocess
import re # Still needed for error pattern matching in fallback logic
import logging
import json
import shutil
import tempfile
from pathlib import Path
import select # Needed for non-blocking read on Unix
import time # Needed for sleep
import sys # Needed for architecture check

# Project imports
from src.config import (
    DEFAULT_VMAF_TARGET, DEFAULT_ENCODING_PRESET,
    MIN_VMAF_FALLBACK_TARGET, VMAF_FALLBACK_STEP
)
from src.utils import get_video_info, anonymize_filename, format_file_size
# Import exceptions, cleaner, parser from this package
from .exceptions import (
    AbAv1Error, InputFileError, OutputFileError, VMAFError, EncodingError
)
from .cleaner import clean_ab_av1_temp_folders
from .parser import AbAv1Parser

# Windows specific imports for non-blocking read
if os.name == 'nt':
    import msvcrt
    import ctypes
    from ctypes import wintypes

    GENERIC_READ = 0x80000000
    FILE_FLAG_OVERLAPPED = 0x40000000
    OPEN_EXISTING = 3
    INVALID_HANDLE_VALUE = wintypes.HANDLE(-1).value
    PIPE_NOWAIT = 0x00000001

    kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)

    # Define ULONG_PTR conditionally based on architecture
    if sys.maxsize > 2**32: # 64-bit Python
        ULONG_PTR = ctypes.c_uint64
    else: # 32-bit Python
        ULONG_PTR = ctypes.c_uint32
    wintypes.ULONG_PTR = ULONG_PTR # Add it to wintypes for convenience if needed elsewhere, though direct use is fine


    # Define necessary C structures and function prototypes
    class OVERLAPPED(ctypes.Structure):
        _fields_ = [
            ('Internal', ULONG_PTR),      # Use the defined ULONG_PTR
            ('InternalHigh', ULONG_PTR),  # Use the defined ULONG_PTR
            ('Offset', wintypes.DWORD),
            ('OffsetHigh', wintypes.DWORD),
            ('hEvent', wintypes.HANDLE),
        ]

    LPOVERLAPPED = ctypes.POINTER(OVERLAPPED)

    kernel32.CreateFileW.restype = wintypes.HANDLE
    kernel32.CreateFileW.argtypes = (
        wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
        ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE
    )

    kernel32.PeekNamedPipe.argtypes = (
        wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
        wintypes.LPDWORD, wintypes.LPDWORD, wintypes.LPDWORD
    )

    kernel32.ReadFile.argtypes = (
        wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
        wintypes.LPDWORD, LPOVERLAPPED
    )
    kernel32.ReadFile.restype = wintypes.BOOL

    kernel32.CancelIoEx.argtypes = (wintypes.HANDLE, LPOVERLAPPED)
    kernel32.CancelIoEx.restype = wintypes.BOOL

    kernel32.CloseHandle.argtypes = (wintypes.HANDLE,)
    kernel32.CloseHandle.restype = wintypes.BOOL


logger = logging.getLogger(__name__)


class AbAv1Wrapper:
    """Wrapper for the ab-av1 tool providing high-level encoding interface.

    This class handles execution of ab-av1.exe, monitors progress via a parser,
    and manages VMAF-based encoding with automatic fallback.
    """

    def _log_consolidated_progress(self, stats, current_temp_size=None, estimated_final_size=None):
        """Log a consolidated progress message to reduce the number of log lines.

        Args:
            stats: Dictionary containing encoding statistics.
            current_temp_size: Current size of the output file in bytes.
            estimated_final_size: Estimated final size of the output file in bytes.
        """
        try:
            # Create consolidated progress message
            progress_parts = []
            encoding_progress = stats.get("progress_encoding", 0)
            progress_parts.append(f"{encoding_progress:.1f}%")
            progress_parts.append(f"phase={stats.get('phase', 'encoding')}")
            if stats.get("last_ffmpeg_fps"):
                progress_parts.append(f"{stats['last_ffmpeg_fps']} fps")
            if stats.get("eta_text"):
                progress_parts.append(f"ETA: {stats['eta_text']}")
            if stats.get("input_path"):
                filename = os.path.basename(stats["input_path"])
                progress_parts.append(f"file={anonymize_filename(filename)}")
            if current_temp_size is not None and estimated_final_size is not None:
                progress_parts.append(f"Size: {format_file_size(current_temp_size)}/{format_file_size(estimated_final_size)}")

            # Prefer estimated reduction if available, otherwise calculate
            if stats.get("estimated_size_reduction") is not None:
                 progress_parts.append(f"reduction={stats['estimated_size_reduction']:.1f}% [Est]")
            elif stats.get("size_reduction") is not None:
                 progress_parts.append(f"reduction={stats['size_reduction']:.1f}%")
            # Fallback calculation removed here as it's complex and estimation is done by parser now

            logger.info(f"PROGRESS: {' | '.join(progress_parts)}")
        except Exception as e:
            logger.error(f"Error in _log_consolidated_progress: {e}")


    def __init__(self):
        """Initialize the wrapper, find executable, prepare parser."""
        # Determine executable path relative to this file's location (src/ab_av1/wrapper.py)
        script_dir = os.path.dirname(os.path.abspath(__file__)) # src/ab_av1/
        src_dir = os.path.dirname(script_dir) # src/
        self.executable_path = os.path.abspath(os.path.join(src_dir, "ab-av1.exe"))

        logger.debug(f"AbAv1Wrapper init - expecting executable at: {self.executable_path}")
        self._verify_executable()

        # Initialize the parser - callback will be set in auto_encode
        self.parser = AbAv1Parser()
        self.file_info_callback = None # Store callback reference separately

    def _verify_executable(self) -> bool:
        """Verify that the ab-av1 executable exists at the expected location.

        Returns:
            True if the executable exists.

        Raises:
            FileNotFoundError: If the executable is not found.
        """
        if not os.path.exists(self.executable_path):
            error_msg = (f"ab-av1.exe not found. Place inside 'src' dir.\nExpected: {self.executable_path}")
            logger.error(error_msg); raise FileNotFoundError(error_msg)
        logger.debug(f"AbAv1Wrapper init - verified: {self.executable_path}"); return True

    def _read_stdout_non_blocking(self, process, stats):
        """Read stdout from the process in a non-blocking way (stderr assumed redirected to stdout)."""
        output_buffer = b""
        full_output_lines = []
        stream_closed = False
        stream = process.stdout # Only monitor stdout

        while True:
            ready_to_read = False # Flag indicating data is ready
            try:
                # Check if process has exited prematurely
                process_terminated = process.poll() is not None
                if process_terminated and not ready_to_read:
                    logger.debug("Process terminated, checking for final output.")
                    time.sleep(0.1) # Give buffers a moment
                    # Allow one final check/read attempt below

                if stream_closed:
                    logger.debug("Stream flagged as closed.")
                    if process_terminated:
                        logger.debug("Stream closed and process terminated, exiting loop.")
                        break
                    else:
                        # Stream closed but process still running? Unusual. Wait.
                        time.sleep(0.1)
                        continue

                if os.name == 'nt':
                    # Windows: Use PeekNamedPipe
                    bytes_available = wintypes.DWORD(0)
                    total_bytes_avail = wintypes.DWORD(0) # For PeekNamedPipe

                    handle_stdout = msvcrt.get_osfhandle(stream.fileno())
                    # Peek first to see if there's data without blocking
                    peek_success = kernel32.PeekNamedPipe(
                        handle_stdout, None, 0, None,
                        ctypes.byref(total_bytes_avail), # Total bytes available
                        ctypes.byref(bytes_available) # Bytes left in message (not useful here)
                    )

                    if peek_success:
                        if total_bytes_avail.value > 0:
                             logger.debug(f"PeekNamedPipe: {total_bytes_avail.value} bytes available.")
                             ready_to_read = True
                        else:
                             # No data currently available
                             logger.debug("PeekNamedPipe: No data available.")
                             ready_to_read = False
                    else:
                         # Check for common errors indicating pipe closure
                         err = ctypes.get_last_error()
                         if err in (109, 232): # ERROR_BROKEN_PIPE, ERROR_NO_DATA (maybe?)
                             logger.debug(f"PeekNamedPipe indicated stdout closed (Error: {err}).")
                             stream_closed = True
                             ready_to_read = False # Ensure we don't try reading
                         else:
                             # Log other errors but assume closed
                             logger.warning(f"PeekNamedPipe error on stdout (Handle: {handle_stdout}, Error: {err})")
                             stream_closed = True
                             ready_to_read = False

                else:
                    # Unix: Use select
                    try:
                         # Check if stream is ready for reading with a small timeout
                         ready, _, _ = select.select([stream], [], [], 0.05) # Short timeout
                         if ready:
                             logger.debug("select indicates stdout is ready.")
                             ready_to_read = True
                         else:
                             logger.debug("select: stdout not ready.")
                             ready_to_read = False
                    except ValueError: # Happens if file descriptor is closed unexpectedly
                        logger.warning("ValueError during select, stdout likely closed.")
                        stream_closed = True
                        ready_to_read = False
                    except Exception as sel_err:
                        logger.error(f"Unexpected error during select: {sel_err}")
                        stream_closed = True # Assume closed on error
                        ready_to_read = False

                # Read from stream if ready
                if ready_to_read:
                    try:
                        # Read reasonably large chunks
                        chunk = os.read(stream.fileno(), 4096)
                        if chunk == b"": # Indicates EOF
                            logger.debug("EOF detected on stdout.")
                            stream_closed = True
                        else:
                            logger.debug(f"Read {len(chunk)} bytes from stdout.")
                            output_buffer += chunk
                            # Attempt to process lines
                            buffer = output_buffer
                            while b'\n' in buffer:
                                line_bytes, buffer = buffer.split(b'\n', 1)
                                output_buffer = buffer # Put remaining part back
                                # Decode carefully
                                try:
                                    line = line_bytes.decode('utf-8', errors='replace').strip()
                                    if line: # Process non-empty lines
                                         full_output_lines.append(line + '\n') # Store line ending
                                         logger.debug(f"RAW_PIPE: {line}")
                                         stats = self.parser.parse_line(line, stats)
                                except Exception as decode_err:
                                    logger.error(f"Error decoding/parsing line: {decode_err} - Line (bytes): {line_bytes[:100]}...")
                    except BlockingIOError:
                        # This shouldn't happen with select/PeekNamedPipe checking first, but handle defensively
                        logger.debug("BlockingIOError during read (unexpected).")
                        pass
                    except OSError as e:
                        # Handle errors like "Bad file descriptor" if pipe closed between check and read
                        logger.warning(f"OSError reading from stdout: {e}")
                        stream_closed = True
                    except Exception as read_err:
                        logger.error(f"Unexpected error reading stdout: {read_err}", exc_info=True)
                        stream_closed = True

                # Check exit condition: Process terminated AND stream is closed/EOF
                if process_terminated and stream_closed:
                     logger.debug("Process terminated and stdout closed, exiting read loop.")
                     break

                # Small sleep if nothing was ready to avoid busy-waiting 100% CPU
                # Needed especially for Windows PeekNamedPipe loop
                if not ready_to_read and not process_terminated:
                    time.sleep(0.05)

            except KeyboardInterrupt:
                 logger.warning("Keyboard interrupt during pipe read.")
                 raise
            except Exception as loop_err:
                 logger.error(f"Error in pipe reading loop: {loop_err}", exc_info=True)
                 # Attempt to break gracefully
                 break

        # Process any remaining data in buffer after loop exit
        if output_buffer:
            try:
                line = output_buffer.decode('utf-8', errors='replace').strip()
                if line:
                    full_output_lines.append(line + '\n')
                    logger.debug(f"RAW_PIPE_REMAINING: {line}")
                    stats = self.parser.parse_line(line, stats)
            except Exception as decode_err:
                logger.error(f"Error decoding/parsing remaining buffer: {decode_err}")

        return "".join(full_output_lines), stats


    def auto_encode(self, input_path: str, output_path: str,
                    file_info_callback: callable = None,
                    pid_callback: callable = None,
                    total_duration_seconds: float = 0.0) -> dict:
        """Run ab-av1 auto-encode with VMAF fallback loop.

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
        self.parser.file_info_callback = file_info_callback
        preset = DEFAULT_ENCODING_PRESET
        initial_min_vmaf = DEFAULT_VMAF_TARGET

        anonymized_input_path = anonymize_filename(input_path)
        if not os.path.exists(input_path):
            # ... (input validation remains the same) ...
            error_msg = f"Input not found: {anonymized_input_path}"; logger.error(error_msg)
            if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"missing_input"})
            raise InputFileError(error_msg, error_type="missing_input")
        try:
            video_info = get_video_info(input_path)
            if not video_info or "streams" not in video_info: raise InputFileError("Invalid video file", error_type="invalid_video")
            if not any(s.get("codec_type") == "video" for s in video_info.get("streams",[])): raise InputFileError("No video stream", error_type="no_video_stream")
            try:
                original_size = os.path.getsize(input_path)
                stats = {"original_size": original_size}
                logger.info(f"Original file size: {original_size} bytes")
            except Exception as size_e:
                logger.warning(f"Couldn't get original file size: {size_e}")
                stats = {}
        except AbAv1Error: raise # Propagate AbAv1 specific errors
        except Exception as e:
             error_msg=f"Error analyzing {anonymized_input_path}: {str(e)}"; logger.error(error_msg)
             if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"analysis_failed"})
             raise InputFileError(error_msg, error_type="analysis_failed") from e


        if not output_path.lower().endswith('.mkv'): output_path = os.path.splitext(output_path)[0] + '.mkv'
        output_dir = os.path.dirname(output_path)
        try: os.makedirs(output_dir, exist_ok=True)
        except Exception as e:
             # ... (output dir creation error handling remains the same) ...
            error_msg = f"Cannot create output dir: {str(e)}"; logger.error(error_msg)
            if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"output_dir_creation_failed"})
            raise OutputFileError(error_msg, error_type="output_dir_creation_failed") from e
        temp_output = output_path + ".temp.mkv"
        anonymized_output_path = anonymize_filename(output_path)
        anonymized_temp_output = anonymize_filename(temp_output)

        current_vmaf_target = initial_min_vmaf
        last_error_info = None
        success = False
        full_output_text = "" # Initialize full output text

        stats.update({
            "phase": "crf-search", "progress_quality": 0, "progress_encoding": 0,
            "vmaf": None, "crf": None, "size_reduction": None,
            "input_path": input_path, "output_path": output_path, "command": "",
            "vmaf_target_used": current_vmaf_target, "last_ffmpeg_fps": None,
            "eta_text": None, "total_duration_seconds": total_duration_seconds,
            "last_reported_encoding_progress": -1.0, "estimated_output_size": None,
            "estimated_size_reduction": None
        })


        while current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET:
            # ... (reset stats for new attempt remains the same) ...
            stats["phase"] = "crf-search"; stats["progress_quality"] = 0; stats["progress_encoding"] = 0
            stats["vmaf"] = None; stats["crf"] = None; stats["last_reported_encoding_progress"] = -1.0
            stats["vmaf_target_used"] = current_vmaf_target; stats["estimated_output_size"] = None
            stats["estimated_size_reduction"] = None; stats["eta_text"] = None; stats["last_ffmpeg_fps"] = None

            logger.info(f"Attempting encode for {anonymized_input_path} with VMAF target: {current_vmaf_target}")

            cmd = [
                self.executable_path, "auto-encode",
                "-i", input_path, "-o", temp_output,
                "--preset", str(preset),
                "--min-vmaf", str(current_vmaf_target)
            ]
            # ... (command logging remains the same) ...
            cmd_str = " ".join(cmd); stats["command"] = cmd_str
            cmd_for_log = [ os.path.basename(self.executable_path), "auto-encode", "-i", os.path.basename(anonymized_input_path), "-o", os.path.basename(anonymized_temp_output), "--preset", str(preset), "--min-vmaf", str(current_vmaf_target) ]
            cmd_str_log = " ".join(cmd_for_log)
            logger.debug(f"Running: {cmd_str_log}"); logger.debug(f"Full cmd: {cmd_str}")


            if self.file_info_callback:
                 # ... (starting/retrying callback logic remains the same) ...
                 if current_vmaf_target != initial_min_vmaf: self.file_info_callback(os.path.basename(input_path), "retrying", { "message": f"Retrying with VMAF target: {current_vmaf_target}", "original_vmaf": initial_min_vmaf, "fallback_vmaf": current_vmaf_target, "used_fallback": True })
                 elif 'original_size' in stats: self.file_info_callback(os.path.basename(input_path), "starting")


            # --- Process Execution & Non-Blocking Read ---
            process = None
            full_output_text = "" # Reset output text for this attempt
            log_consolidation_counter = 0
            try:
                startupinfo = None
                creationflags = 0
                if os.name == 'nt':
                    startupinfo = subprocess.STARTUPINFO()
                    startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
                    startupinfo.wShowWindow = subprocess.SW_HIDE
                    # Prevent console window from appearing
                    creationflags = subprocess.CREATE_NO_WINDOW

                # *** Change: Redirect stderr to stdout again ***
                process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                          bufsize=0, # Use 0 for unbuffered binary IO
                                          cwd=output_dir,
                                          startupinfo=startupinfo,
                                          creationflags=creationflags)

                if pid_callback: pid_callback(process.pid)

                # *** Change: Use the modified non-blocking read function for stdout only ***
                full_output_text, stats = self._read_stdout_non_blocking(process, stats)

                # Estimate size and log consolidated progress periodically *during* the read loop
                # (This logic might need adjustment if stats aren't updated frequently enough by parser)
                # Maybe move estimation inside _read_pipes_non_blocking? For now, estimate after read loop.

                return_code = process.poll() # Get final return code after read loop finishes
                logger.info(f"Process finished with return code: {return_code}")


            except FileNotFoundError as e: # Specific error if ab-av1.exe is missing
                error_msg = f"Executable not found: {self.executable_path} - {e}"
                logger.error(error_msg)
                if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"executable_not_found"})
                raise FileNotFoundError(error_msg) from e # Re-raise specific error
            except Exception as e:
                error_msg = f"Failed to start or manage process: {str(e)}"; logger.error(error_msg, exc_info=True)
                if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg,"type":"process_management_failed"})
                # Ensure process is cleaned up if it exists
                if process and process.poll() is None:
                    try: process.terminate(); time.sleep(0.2); process.kill()
                    except: pass
                raise EncodingError(error_msg, command=cmd_str, error_type="process_management_failed") from e


            # --- Check Result ---
            if return_code == 0:
                success = True
                logger.info(f"Encode succeeded for {anonymized_input_path} with VMAF target {current_vmaf_target}")
                break # Exit the while loop on success
            else:
                # --- Error Handling (remains largely the same, uses full_output_text) ---
                error_type="unknown"; error_details="Unknown error"
                # ... (use full_output_text for error pattern matching) ...
                if re.search(r'ffmpeg.*?:\s*Invalid\s+data\s+found', full_output_text, re.IGNORECASE): error_type="invalid_input_data"; error_details="Invalid data in input"
                elif re.search(r'No\s+such\s+file\s+or\s+directory', full_output_text, re.IGNORECASE): error_type="file_not_found"; error_details="Input not found/inaccessible"
                elif re.search(r'failed\s+to\s+open\s+file', full_output_text, re.IGNORECASE): error_type="file_open_failed"; error_details="Failed to open input"
                elif re.search(r'permission\s+denied', full_output_text, re.IGNORECASE): error_type="permission_denied"; error_details="Permission denied"
                elif re.search(r'vmaf\s+.*?error', full_output_text, re.IGNORECASE): error_type="vmaf_calculation_failed"; error_details="VMAF calculation failed"
                elif re.search(r'encode\s+.*?error', full_output_text, re.IGNORECASE): error_type="encoding_failed"; error_details="Encoding failed"
                elif re.search(r'out\s+of\s+memory', full_output_text, re.IGNORECASE): error_type="memory_error"; error_details="Out of memory"
                elif re.search(r'Failed\s+to\s+find\s+a\s+suitable\s+crf', full_output_text, re.IGNORECASE): error_type = "crf_search_failed"; error_details = f"Could not find suitable CRF for VMAF {current_vmaf_target}"
                elif 'ab-av1.exe' in full_output_text and 'not recognized' in full_output_text: error_type = "executable_not_found"; error_details = "ab-av1.exe command failed (not found or path issue?)"


                last_error_info = {"return_code": return_code, "error_type": error_type, "error_details": error_details, "command": cmd_str_log, "output_tail": full_output_text.splitlines()[-20:]} # Use collected text
                logger.warning(f"Attempt failed for VMAF {current_vmaf_target} (rc={return_code}): {error_details}")
                logger.debug(f"Full output for failed attempt:\n{full_output_text[-1000:]}") # Log tail of output

                if error_type == "crf_search_failed":
                    current_vmaf_target -= VMAF_FALLBACK_STEP
                    if current_vmaf_target >= MIN_VMAF_FALLBACK_TARGET:
                        logger.info(f"Retrying {anonymized_input_path} with VMAF target: {current_vmaf_target}")
                        continue
                    else:
                        logger.error(f"CRF search failed down to VMAF {MIN_VMAF_FALLBACK_TARGET}.")
                        last_error_info["error_details"] = f"CRF search failed down to VMAF {MIN_VMAF_FALLBACK_TARGET}"
                        break
                else:
                    logger.error(f"Non-recoverable error '{error_type}'. Stopping attempts.")
                    break

        # --- End of Fallback Loop ---

        if not success:
            # ... (Failure reporting remains the same, uses last_error_info) ...
            if last_error_info:
                error_msg = f"ab-av1 failed (rc={last_error_info['return_code']}): {last_error_info['error_details']}"
                logger.error(error_msg); logger.error(f"Last Cmd: {last_error_info['command']}"); logger.error(f"Last Output tail:\n{last_error_info['output_tail']}")
                if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg, "type":last_error_info['error_type'], "details":last_error_info['error_details'], "command":last_error_info['command']})
                error_type = last_error_info['error_type']
                exc_map = { "invalid_input_data": InputFileError, "file_not_found": InputFileError, "file_open_failed": InputFileError, "no_video_stream": InputFileError, "analysis_failed": InputFileError, "vmaf_calculation_failed": VMAFError, "encoding_failed": EncodingError, "memory_error": EncodingError, "crf_search_failed": EncodingError, "executable_not_found": EncodingError, "permission_denied": EncodingError, "output_dir_creation_failed": OutputFileError, "rename_failed": OutputFileError }
                exception_class = exc_map.get(error_type, AbAv1Error)
                raise exception_class(error_msg, command=last_error_info['command'], output=full_output_text, error_type=error_type)
            else:
                generic_error_msg = f"Encode failed for {anonymized_input_path} unknown reasons."; logger.error(generic_error_msg)
                if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":generic_error_msg, "type":"unknown_loop_error"})
                raise AbAv1Error(generic_error_msg, error_type="unknown_loop_error")


        # --- Success Path ---
        logger.info(f"ab-av1 completed successfully for {anonymized_input_path} (used VMAF target {stats.get('vmaf_target_used', '?')})")
        # Run final parse on full output for verification/fallback
        stats = self.parser.parse_final_output(full_output_text, stats)

        # ... (Logging final stats, moving temp file, completion callback, temp folder cleanup remains the same) ...
        if stats.get("crf") is not None: logger.info(f"Final CRF: {stats['crf']}")
        if stats.get("vmaf") is not None: logger.info(f"Final VMAF: {stats['vmaf']:.2f}")
        if stats.get("size_reduction") is not None: logger.info(f"Final Size reduction: {stats['size_reduction']:.2f}%")

        try:
            if os.path.exists(output_path): logger.warning(f"Overwriting: {anonymized_output_path}"); os.remove(output_path)
            shutil.move(temp_output, output_path); logger.info(f"Moved temp to final: {anonymized_output_path}")
        except Exception as e:
            error_msg=f"Failed move {os.path.basename(anonymized_temp_output)} to {os.path.basename(anonymized_output_path)}: {str(e)}"; logger.error(error_msg)
            if self.file_info_callback: self.file_info_callback(os.path.basename(input_path), "failed", {"message":error_msg, "type":"rename_failed"})
            cleaned_count = clean_ab_av1_temp_folders(output_dir)
            if cleaned_count > 0: logger.info(f"Cleaned {cleaned_count} temp folders after rename failure.")
            raise OutputFileError(error_msg, command=cmd_str_log, error_type="rename_failed") from e

        if self.file_info_callback:
            final_stats_for_callback = { "message":f"Complete (VMAF {stats.get('vmaf', 'N/A'):.2f} @ Target {stats.get('vmaf_target_used','?')})", "vmaf":stats.get("vmaf"), "crf":stats.get("crf"), "vmaf_target_used": stats.get('vmaf_target_used'), "size_reduction": stats.get('size_reduction'), "output_path": output_path }
            try: final_size = os.path.getsize(output_path); final_stats_for_callback["output_size"] = final_size
            except Exception: pass
            self.file_info_callback(os.path.basename(input_path), "completed", final_stats_for_callback)

        cleaned_count = clean_ab_av1_temp_folders(output_dir)
        if cleaned_count > 0: logger.info(f"Cleaned {cleaned_count} temp folders.")


        self.file_info_callback = None
        self.parser.file_info_callback = None
        return stats

    # Removed _update_stats_from_line method
    # Removed _parse_final_output method