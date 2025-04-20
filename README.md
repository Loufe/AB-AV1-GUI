# AV1 Video Converter

A user-friendly GUI application for converting video files to the high-efficiency AV1 codec format using VMAF-based quality targeting.

## Features

-   **Simple Interface:** Easy selection of input/output folders.
-   **Batch Processing:** Convert multiple video files (`.mp4`, `.mkv`, `.avi`, `.wmv`) found within the input folder and its subdirectories.
-   **Quality Focused:** Automatically determines encoding settings (CRF) to meet a target VMAF score (default 95) for consistent visual quality, using the `ab-av1` tool.
-   **Smart Skipping:** Automatically skips files that are already AV1 in an MKV container, or files that already exist in the output location (unless overwrite is enabled).
-   **Progress Tracking:** Displays overall progress, current file progress (quality detection & encoding phases), estimated time remaining (for encoding), and estimated final file size.
-   **Detailed Statistics:** Shows original/output format & size, VMAF target/result, CRF used, elapsed time per file, and overall average statistics (VMAF, CRF, Size Reduction) for successful conversions.
-   **Control:** Start, Stop Gracefully (after current file), and Force Stop conversion options.
-   **Responsive UI:** Uses multi-threading to keep the interface responsive during conversions.
-   **Logging:** Creates detailed log files for each run in a `logs` subfolder.
-   **Settings Persistence:** Saves input/output folders and settings between sessions.
-   **Conversion History:** Records details of successful conversions in `conversion_history.json`.
-   **Filename Privacy:** Options to anonymize filenames in logs and history files.
-   **Audio Handling:** Option to copy audio tracks or re-encode non-AAC/Opus audio to AAC or Opus.
-   **Power Management (Windows):** Prevents the computer from sleeping during active conversions.

## Requirements

-   **Python:** Version 3.6 or higher recommended.
-   **FFmpeg:** Must be installed and available in the system's PATH. Requires a version with `libsvtav1` (SVT-AV1 encoder) support. You can download builds from [ffmpeg.org](https://ffmpeg.org/download.html) or [Gyan.dev](https://www.gyan.dev/ffmpeg/builds/) (for Windows).
-   **ab-av1:** The `ab-av1.exe` executable must be placed inside the `src/convert_app` directory alongside the Python files. *(Note: You need to obtain this executable separately)*.

## Getting Started

1.  **Ensure Requirements:** Install Python and FFmpeg (add FFmpeg to your PATH). Place `ab-av1.exe` in the `src/convert_app` folder.
2.  **Run the Application:**
    *   **Windows:** Double-click `convert.bat`.
    *   **Other Platforms (Linux/macOS):** Open a terminal in the project's root directory and run:
        ```bash
        python convert.py
        ```

## Basic Workflow

1.  **Launch:** Run the application using the methods above.
2.  **Select Folders:**
    *   Click "Browse..." for "Input Folder" to choose the directory containing videos you want to convert.
    *   Click "Browse..." for "Output Folder" to choose where the converted `.mkv` files will be saved. Subfolder structures will be preserved. If left blank, it defaults to the input folder.
3.  **Configure Settings (Optional):**
    *   Go to the "Settings" tab.
    *   Check/uncheck "Overwrite output file..."
    *   Select the video file extensions (`.mp4`, `.mkv`, etc.) to process.
    *   Configure audio conversion options if needed.
    *   Adjust logging/history anonymization preferences.
4.  **Start Conversion:** Go back to the "Convert" tab and click "Start Conversion".
5.  **Monitor:** Observe the progress bars, status messages, and detailed statistics.
6.  **Completion:** A summary message box will appear when the batch is finished or stopped. Check the output folder and the `conversion_history.json` file.

## Project Structure (High-Level)

-   `convert.py`: Launcher script.
-   `convert.bat`: Windows launcher.
-   `src/`: Contains the core application code.
    -   `convert_app/`: Main Python package.
        -   `gui/`: GUI related modules (main window, tabs, actions, updates).
        -   `main.py`: Application initialization logic.
        -   `ab_av1_wrapper.py`: Interface to the `ab-av1` tool.
        -   `video_conversion.py`: Logic for processing a single video file.
        -   `utils.py`: Shared utility functions (logging, history, etc.).
-   `logs/`: Default location for log files.
-   `av1_converter_config.json`: Saved application settings.
-   `conversion_history.json`: Log of successful conversions.
-   `llm.md`: Detailed technical documentation for developers.
-   `README.md`: This file.