# AV1 Video Converter

A user-friendly GUI application for converting video files to the AV1 codec format.

## Features

- Automatic video analysis for optimal encoding settings
- Smart codec detection to skip already-encoded files
- Pause/resume capability for long-running conversions
- Progress tracking with estimated time remaining
- Multi-threaded operation for responsive UI
- Detailed logging and statistics during conversion
- Batch processing with resumable state

## Getting Started

### Windows

Simply run `convert.bat` to start the application.

### Other Platforms

Run the main.py script:

```
python main.py
```

## Requirements

- Python 3.6 or higher
- FFmpeg with SVT-AV1 support
- Required Python packages (see requirements.txt)

## Project Structure

The application has a modular structure:

- **main.py**: The main entry point script that launches the application
- **convert.bat**: Windows batch file for easy launching
- **convert_app/**: The main package containing all application modules
  - **main.py**: Initializes and runs the application
  - **utils.py**: Utility functions for logging and common operations
  - **video_analysis.py**: Functions for analyzing video characteristics
  - **video_conversion.py**: Core video conversion functionality
  - **gui/**: Package containing all GUI-related modules
    - **main_window.py**: Main application window implementation
    - **operations.py**: GUI operation handlers
    - **tabs/**: Directory containing tab implementations
      - **main_tab.py**: Main conversion tab
      - **settings_tab.py**: Settings configuration tab
      - **log_tab.py**: Log display tab
