# AV1 Video Converter

This application provides a simple graphical interface (GUI) for converting your video files to the modern, high-efficiency AV1 codec. It uses the excellent `ab-av1` tool to automatically optimize encoding settings based on visual quality (VMAF), making high-quality AV1 conversion accessible without needing complex command-line knowledge.

## Key Features

*   **Easy-to-Use Interface:** Select input and output folders, adjust basic settings, and start converting with just a few clicks.
*   **Automatic Quality Control (VMAF):** Instead of guessing bitrates, the converter targets a specific visual quality level (VMAF score, default 95) ensuring consistent results across different videos.
*   **Batch Conversion:** Automatically finds and converts supported video files (`.mp4`, `.mkv`, `.avi`, `.wmv`) within your chosen input folder and its subdirectories.
*   **Progress Tracking:** Monitor the overall batch progress and see details for the currently converting file, including quality detection and encoding phases, plus estimated time remaining.
*   **Reliable MKV Output:** Converts videos into the flexible MKV container format.
    *   *Note:* If converting an MKV file that isn't already AV1, and you are outputting to the *same folder* without enabling 'Overwrite', a suffix ` (av1)` will be added to the output filename to avoid replacing the original.

## Important Note on `ab-av1`

This project relies on the external command-line tool [ab-av1](https://github.com/alexheretic/ab-av1). While a version might be bundled for convenience, **it is strongly recommended for security and compatibility that you download the latest official release yourself** from the [ab-av1 releases page](https://github.com/alexheretic/ab-av1/releases).

Place the downloaded executable (`ab-av1.exe` on Windows, `ab-av1` on Linux/macOS) inside the `src/` directory of this project, replacing any existing file.

## Requirements

*   **Python 3.8+** with Tkinter (included in standard installations).
*   **FFmpeg:** Must be installed and available in your system's PATH. Requires a version with `libsvtav1` (SVT-AV1 encoder) support. You can download builds from [ffmpeg.org](https://ffmpeg.org/download.html) or [Gyan.dev](https://www.gyan.dev/ffmpeg/builds/) (Windows).
*   **ab-av1:** The [ab-av1](https://github.com/alexheretic/ab-av1) executable (see note above).

## Installation & Setup

1.  **Install Dependencies:** Ensure you have Python and FFmpeg installed (and FFmpeg is in your system's PATH).
2.  **Get ab-av1:** Download the executable for your platform and place it in the `src/` directory (see note above).
3.  **Run the Application:**
    ```bash
    python -m src.convert
    ```
    On Windows, you can also double-click `convert.bat`.

## How to Use

1.  **Launch:** Start the application using the methods above.
2.  **Select Folders:**
    *   Click "Browse..." for "Input Folder" to choose where your original videos are.
    *   Click "Browse..." for "Output Folder" to choose where the converted `.mkv` files will be saved. (Defaults to the input folder if left blank).
3.  **Adjust Settings (Optional):**
    *   Go to the "Settings" tab to configure options like overwriting existing files, audio handling, and file types to process.
4.  **Start:** Go back to the "Convert" tab and click "Start Conversion".
5.  **Monitor:** Watch the progress bars and status messages.
6.  **Done!** A summary message will appear upon completion. Check your output folder.

---

## Privacy Features

The application includes optional privacy features to anonymize file paths in logs and history:

*   **Log Anonymization:** When enabled in Settings, file paths and video filenames in log files are replaced with BLAKE2b hashes (e.g., `file_7f3a9c2b1e4d.mp4`).
*   **History Anonymization:** Similarly anonymizes the conversion history file.
*   **Scrub Buttons:** The Settings tab includes "Scrub Logs" and "Scrub History" buttons to retroactively anonymize existing files. This is irreversible.
*   **Reverse Lookup:** If needed, use `tools/hash_lookup.py` to find original files by their hash.

---

## Troubleshooting

### FFmpeg not found
Ensure FFmpeg is installed and available in your system PATH. On Windows, you may need to restart your terminal after installation.

### libsvtav1 not available
Your FFmpeg build must include SVT-AV1 encoder support. Download a full build from [Gyan.dev](https://www.gyan.dev/ffmpeg/builds/) (Windows) or install via your package manager (Linux/macOS).

### Conversion fails immediately
- Check that `ab-av1` executable is in the `src/` directory
- Verify the input file is a valid video with a video stream
- Check the log files in the `logs/` directory for detailed error messages

### GUI freezes during conversion
This should not happen. If it does, check the log files for errors. The conversion runs in a separate thread to keep the GUI responsive.

---

## Documentation

- [Architecture](docs/ARCHITECTURE.md) - Technical details, conversion flow diagrams, threading model
- [agents.md](agents.md) - Development guidelines and project structure