# AV1 Video Converter - Technical Documentation

**Note:** This file contains detailed technical notes about the project structure, implementation details, and design choices, intended for LLM analysis and developers. For user-focused documentation, please see `README.md`.

## Project Overview

This project aims to create a user-friendly GUI application for converting video files to the AV1 codec format using the `ab-av1` tool for quality-based encoding.

### Project Goals
1.  Make high-quality AV1 encoding accessible via a GUI.
2.  Optimize encoding parameters automatically based on source video characteristics using VMAF targeting.
3.  Provide a clean, informative interface with real-time progress tracking.
4.  Support batch processing of multiple files.
5.  Use `ab-av1` tool for optimal VMAF-based encoding.

## Project Structure

The application has a modular architecture:

-   **convert.py**: Main entry point script (launcher).
-   **convert.bat**: Windows batch file for easy launching.
-   **main.py**: Defines the main application function (`main()`) which initializes logging and the GUI.
-   **utils.py**: Utility functions (logging setup, file size/time formatting, `get_video_info` via ffprobe, dependency checks, anonymization, history management, power management).
-   **video_conversion.py**: Core video conversion logic for a single file (`process_video`), interfacing with the `ab_av1_wrapper`.
-   **config.py**: Configuration constants (VMAF targets, encoding presets, etc.)
-   **ab_av1/**: Package containing the ab-av1 wrapper and related modules
    -   **wrapper.py**: Wrapper class (`AbAv1Wrapper`) for executing and managing the `ab-av1.exe` command-line tool, including output parsing, progress monitoring, error handling, and VMAF fallback logic.
    -   **parser.py**: Parser for ab-av1 and ffmpeg output streams
    -   **cleaner.py**: Utility functions for cleaning up temp directories
    -   **exceptions.py**: Custom exceptions for different error types
    -   **checker.py**: Functions to check for ab-av1 availability
-   **gui/**: Package containing all GUI-related modules.
    -   **main_window.py**: Main application window (`VideoConverterGUI` class) implementation, handling settings persistence, GUI setup, and main event loop integration.
    -   **gui_actions.py**: Handles GUI actions triggered by user interaction (e.g., browsing files/folders, opening logs/history, checking dependencies).
    -   **gui_updates.py**: Handles updating GUI elements safely from potentially different threads (e.g., progress bars, labels, statistics, timers).
    -   **conversion_controller.py**: Manages the overall conversion process state, runs the worker thread (`sequential_conversion_worker`), handles start/stop/force-stop logic, dispatches callbacks (`handle_*` functions) from the conversion process to GUI updates.
    -   **base.py**: Base GUI components (currently only `ToolTip`).
    -   **tabs/**: Directory containing tab implementations.
        -   **main_tab.py**: Defines the UI layout for the main "Convert" tab.
        -   **settings_tab.py**: Defines the UI layout for the "Settings" tab.
-   **README.md**: User-focused documentation.
-   **llm.md**: This file (developer/LLM-focused documentation).
-   **av1_converter_config.json**: Stores user settings (created on exit).
-   **conversion_history.json**: Stores records of completed conversions (created after first success).
-   **logs/**: Default directory for log files (created on first run).

## Technical Implementation

### Core Technology Stack
-   Python (>=3.6) with Tkinter for the GUI.
-   External Tools:
    -   `ab-av1.exe`: Required for the core VMAF-targeted AV1 encoding. Placed in the `src` directory.
    -   `ffmpeg.exe` (with `libsvtav1`): Required by `ab-av1` and for video analysis (`ffprobe`). Must be in the system's PATH.
-   Standard Libraries: `subprocess`, `threading`, `logging`, `json`, `os`, `sys`, `time`, `pathlib`, etc.

### Key Features (Technical Detail)
-   **VMAF-based Quality Targeting:** Leverages `ab-av1 auto-encode`'s ability to find the optimal CRF value that meets a specified VMAF target (`DEFAULT_VMAF_TARGET` in `config.py`). Includes a fallback loop (`AbAv1Wrapper.auto_encode`) that lowers the target VMAF if the initial target is unattainable, down to a minimum (`MIN_VMAF_FALLBACK_TARGET`).
-   **Two-Phase Process:** `ab-av1` implicitly performs a CRF search phase (sampling and testing) before the main encoding phase. The GUI reflects this with distinct progress reporting (Quality Detection vs. Encoding bars).
-   **Video Analysis:** Uses `ffprobe` (via `utils.get_video_info`) to get container format, codec info, resolution, duration, etc., primarily to check if conversion is needed and for logging/history.
-   **Smart Skipping:** `video_conversion.process_video` checks the output path (considering `overwrite` setting) and uses `get_video_info` to check if the input is already an AV1 video in an MKV container.
-   **Progress Tracking:** `AbAv1Wrapper.parser.parse_line` parses `ab-av1`/`ffmpeg` output using regex to extract VMAF/CRF during search and percentage/FPS/ETA/size during encoding. These stats are passed via callbacks to `conversion_controller.handle_progress`, which uses `gui_updates` functions to update the UI. ETA and estimated size are calculated within `gui_updates.update_conversion_statistics`.
-   **Multi-threading:** `conversion_controller.start_conversion` launches `sequential_conversion_worker` in a separate `threading.Thread` to avoid blocking the Tkinter main loop. `utils.update_ui_safely` uses `root.after(0, ...)` to marshal calls back to the main GUI thread.
-   **Logging:** `utils.setup_logging` configures root logger with `RotatingFileHandler` (file) and `StreamHandler` (console). `FilenamePrivacyFilter` anonymizes paths in logs if enabled.
-   **Process Management:** `AbAv1Wrapper` uses `subprocess.Popen` to run `ab-av1`. `conversion_controller.force_stop_conversion` attempts to kill the process tree (`taskkill /T` on Windows, `os.kill` with SIGTERM/SIGKILL on Unix). `pid_callback` mechanism stores the active PID. Temporary files (`.temp.mkv`) are used and cleanup is attempted on exit/error/completion.
-   **Settings/History:** JSON files (`av1_converter_config.json`, `conversion_history.json`) are used for persistence, saved in the script/executable directory. Atomic writes (`os.replace`) are used for saving.

## AB-AV1 Integration

### Integration Method (`ab_av1/wrapper.py`)
-   `AbAv1Wrapper` class encapsulates interaction.
-   `_verify_executable`: Checks for `ab-av1.exe` presence.
-   `auto_encode`: Main method executing `ab-av1 auto-encode`.
    -   Constructs command line arguments including input, output (`.temp.mkv`), preset, and VMAF target.
    -   Implements VMAF fallback loop, decrementing target if CRF search fails.
    -   Launches `ab-av1.exe` using `subprocess.Popen`, redirecting stderr to stdout.
    -   Reads output line-by-line (`iter(process.stdout.readline, "")`).
    -   Uses parser to extract progress and status information
    -   Uses callbacks (`file_info_callback`, `pid_callback`) to report status/PID to `conversion_controller`.
    -   Handles process exit codes and specific error patterns (e.g., "Failed to find a suitable crf").
    -   Raises custom exceptions (`InputFileError`, `OutputFileError`, etc.).
    -   Calls `clean_ab_av1_temp_folders`.

### Parser Implementation (`ab_av1/parser.py`)
- The `AbAv1Parser` class handles parsing output from ab-av1 and ffmpeg.
- `parse_line`: Processes each line of output using regex to detect progress updates, phase changes, and metadata like VMAF scores and CRF values.
- `parse_final_output`: Scans the complete output text after completion to extract final statistics that might have been missed during streaming.

## Stdout/Piping Challenges and Solutions

### Two-Phase Process and Parsing Differences

The application handles two distinct phases in the conversion process, each with different stdout characteristics:

1. **Quality Detection Phase**:
   - Mostly output from ab-av1's CRF search algorithm
   - Relatively structured and predictable output format
   - Example patterns: `Current progress: X%`, `Trying crf=X.X, vmaf=Y.Y`
   - More reliable progress reporting with clear percentage indicators

2. **Encoding Phase**:
   - Output comes primarily from FFmpeg
   - Less structured, varies between FFmpeg versions
   - Multiple output formats to parse: `frame=X fps=Y time=HH:MM:SS.MS speed=Zx`
   - Inconsistent output frequency
   - More prone to buffering issues

### Core Piping Implementation

```python
process = subprocess.Popen(
    cmd,
    stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    universal_newlines=True, bufsize=1,
    encoding='utf-8', errors='replace',
    env=process_env  # Environment with verbosity settings
)

# Simple blocking readline loop
for line in iter(process.stdout.readline, ''):
    line = line.strip()
    if not line: continue
    # Process line with parser
```

### Known Stdout Issues During Encoding

1. **FFmpeg Output Buffering**: FFmpeg may buffer its output when not writing directly to a terminal, causing periods of apparent inactivity in the output stream even though encoding is proceeding.

2. **Pipe Closures**: The readline loop may exit prematurely if the pipe is closed unexpectedly, which can happen during long encoding operations.

3. **Inconsistent Progress Format**: FFmpeg's progress reporting format can vary, requiring multiple regex patterns to catch all variants.

4. **Output Frequency Variability**: During intensive encoding, FFmpeg may not output progress information at regular intervals.

### Environment Variables for Maximum Verbosity

To maximize output, especially during the encoding phase, several environment variables are set:

```python
process_env["RUST_LOG"] = "trace,ab_av1=trace,ffmpeg=trace"  # Rust logging
process_env["AV1_PRINT_FFMPEG"] = "1"  # Force FFmpeg output printing
process_env["AV1_RAW_OUTPUT"] = "1"  # Pass through raw output
process_env["FFMPEG_PROGRESS"] = "1"  # Enable FFmpeg progress
process_env["SVT_VERBOSE"] = "1"  # SVT-AV1 encoder verbosity
process_env["AB_AV1_VERBOSE"] = "1"  # ab-av1 verbosity
process_env["AB_AV1_LOG_PROGRESS"] = "1"  # Progress logging
```

### Fallback Mechanisms

The application has several fallback mechanisms to handle stdout issues:

1. **Pipe Closure Detection**:
   ```python
   final_poll_code = process.poll()
   if final_poll_code is None:  # Process running but pipe closed
       logger.warning("Process still running after pipe reading completed")
       pipe_closed_prematurely = True
   ```

2. **Final Output Parsing**:
   ```python
   # Fallback if streaming parsing missed data
   stats = self.parser.parse_final_output(full_output_text, stats)
   ```

3. **Timeout-Based Process Termination**:
   ```python
   try:
       return_code = process.wait(timeout=30)  # Wait up to 30 seconds
   except subprocess.TimeoutExpired:
       logger.error("Process did not complete within timeout period")
       process.terminate()
   ```

### Potential Improvements

1. **Advanced Buffering Control**:
   - Add FFmpeg parameters like `-progress pipe:1` to force unbuffered output
   - Use additional environment variables: `FFmpeg_FORCE_STDERR=1`, `FFmpeg_FORCE_NOCOLOR=1`

2. **Timeout-Based Reading**:
   ```python
   def readline_with_timeout(stream, timeout=10):
       """Read line with timeout to detect stalled output"""
       import select
       ready, _, _ = select.select([stream], [], [], timeout)
       if ready:
           return stream.readline()
       else:
           return None  # Timeout occurred
   ```

3. **Dual Progress Monitoring**:
   - Monitor stdout for explicit progress information
   - Periodically check output file size growth as an implicit progress indicator
   - Use a dedicated thread to send signals (SIGINFO) to prompt FFmpeg to report status

4. **Output Format Detection Enhancements**:
   - Implement adaptive pattern recognition based on initial output analysis
   - Cache successful patterns for current FFmpeg version

5. **Process Communication Alternatives**:
   - Investigate using named pipes or alternative IPC methods with FFmpeg
   - Consider modifications to ab-av1 to improve its FFmpeg output handling

## Data Flow (Simplified Conversion Start)

1.  **User Clicks Start (main_tab -> main_window -> conversion_controller.start_conversion):**
    *   GUI state checked/validated (folders, extensions).
    *   `check_ffmpeg` (via `gui_actions`) called.
    *   UI buttons disabled/enabled.
    *   Conversion state variables initialized (`conversion_running`, `stop_event`, stats lists, etc.).
    *   Sleep prevention enabled (`utils.prevent_sleep_mode`).
    *   `sequential_conversion_worker` thread launched.
2.  **Worker Thread (`conversion_controller.sequential_conversion_worker`):**
    *   Scans input folder for matching video files (`Path.rglob`).
    *   Performs preliminary scan using `scan_video_needs_conversion`:
        *   Checks output existence/overwrite.
        *   Calls `utils.get_video_info` to check for AV1/MKV.
    *   Loops through files needing conversion:
        *   Updates overall progress/status labels (`gui_updates`).
        *   Resets current file details (`gui_updates.reset_current_file_details`).
        *   Calls `utils.get_video_info` for details.
        *   Calls `video_conversion.process_video`.
3.  **Single File Processing (`video_conversion.process_video`):**
    *   Determines output path.
    *   Performs pre-flight checks (output exists, get info, check AV1/MKV again).
    *   Logs input properties (`utils.log_video_properties`).
    *   Instantiates `AbAv1Wrapper`.
    *   Calls `ab_av1.auto_encode`, passing callbacks:
        *   `file_callback_dispatcher` (defined in worker).
        *   `pid_callback` -> `conversion_controller.store_process_id`.
4.  **AB-AV1 Execution (`ab_av1/wrapper.py::auto_encode`):**
    *   Runs `ab-av1.exe` via `subprocess.Popen`.
    *   Sends PID back via `pid_callback`.
    *   Reads stdout/stderr line by line.
    *   Calls `parser.parse_line` for each line.
5.  **Parsing & Callbacks (`ab_av1/parser.py`):**
    *   Parses progress/stats via regex.
    *   Updates stats dict with parsed information
    *   Calls `file_info_callback` (-> `file_callback_dispatcher` in worker).
6.  **Callback Dispatcher (`sequential_conversion_worker.<locals>.file_callback_dispatcher`):**
    *   Receives status ("progress", "completed", "error", etc.) and info dict.
    *   Calls appropriate `handle_*` function (e.g., `conversion_controller.handle_progress`).
7.  **Callback Handlers (`conversion_controller.handle_*`):**
    *   e.g., `handle_progress` calls `gui_updates.update_progress_bars` and `gui_updates.update_conversion_statistics`.
8.  **GUI Updates (`gui_updates` module):**
    *   Functions use `utils.update_ui_safely` to schedule updates on the main Tkinter thread.
9.  **Completion (`video_conversion.process_video` -> worker):**
    *   `auto_encode` returns stats dict or raises error.
    *   `process_video` logs results, returns tuple on success.
    *   Worker updates overall progress, calls `handle_completed` via dispatcher (which updates stats), appends to history (`utils.append_to_history`).
10. **Loop End / Stop (`sequential_conversion_worker` -> `conversion_controller.conversion_complete`):**
    *   Worker loop finishes or `stop_event` is set.
    *   Calls `conversion_complete`.
    *   `conversion_complete` performs final cleanup (temp folders, sleep mode), resets UI state, shows summary messagebox.