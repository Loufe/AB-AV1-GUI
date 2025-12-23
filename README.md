# AV1 Video Converter

This application provides a simple graphical interface (GUI) for converting your video files to the modern, high-efficiency AV1 codec. It uses the excellent [ab-av1](https://github.com/alexheretic/ab-av1) tool to automatically optimize encoding settings based on visual quality ([VMAF](https://github.com/Netflix/vmaf)), making high-quality AV1 conversion accessible without needing complex command-line knowledge.

## Key Features

- **Automatic Quality Control (VMAF):** Instead of guessing bitrates, the converter targets a specific visual quality level (VMAF score, default 95) ensuring consistent results across different videos.
- **Batch Conversion:** Automatically finds and converts supported video files (`.mp4`, `.mkv`, `.avi`, `.wmv`) within your chosen input folder and its subdirectories.
- **Progress Tracking:** Monitor the overall batch progress and see details for the currently converting file, including quality detection and encoding phases, plus estimated time remaining.

## Requirements

- **Python 3.8+** with Tkinter (included in standard installations).
- **FFmpeg:** Must be installed and available in your system's PATH. Requires a version with `libsvtav1` (SVT-AV1 encoder) support. You can download builds from [ffmpeg.org](https://ffmpeg.org/download.html) or [Gyan.dev](https://www.gyan.dev/ffmpeg/builds/) (Windows).
- **ab-av1:** [Download the latest release](https://github.com/alexheretic/ab-av1/releases) and place in `src/` directory.

  > [!WARNING]
  > A version may be bundled for convenience, but for security and compatibility, always download the official release yourself.

## Installation & Setup

1.  **Install Dependencies:** Ensure you have Python and FFmpeg installed (and FFmpeg is in your system's PATH).
2.  **Get ab-av1:** Download the executable for your platform and place it in the `src/` directory.
3.  **Run the Application:**
    ```bash
    python -m src.convert
    ```
    On Windows, you can also double-click `convert.bat`.

## Operation

Point the application at an input folder and it will recursively scan for video files, processing them one at a time. Files are automatically skipped if they:
- Are already AV1-encoded
- Fall below the resolution threshold (default 720p)
- Already have a converted output file (unless overwrite is enabled)
- Cannot achieve the target VMAF quality

Output files mirror the input folder structure, or go to a flat output directory if specified.

## Privacy

**Zero telemetry.** The application never contacts external servers. Even version checks require you to manually click a button—nothing happens automatically.

Optional path anonymization (Settings tab) replaces file paths in logs and conversion history with BLAKE2b hashes. If you need to trace a hash back to the original file, use `tools/hash_lookup.py`.

## Third-Party Software

This application downloads and uses:

- [FFmpeg](https://ffmpeg.org/) (LGPL 2.1+) — video encoding
- [ab-av1](https://github.com/alexheretic/ab-av1) (MIT) — VMAF-targeted quality optimization
