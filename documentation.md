## Getting Started

### Installation Requirements
- Python 3.6 or higher
- FFmpeg with SVT-AV1 support
- ab-av1.exe in the convert_app directory

### Running the Application
To start the application, simply run the `convert.py` script in the root directory:

```
python convert.py
```

The GUI will launch, allowing you to select input/output folders and start converting videos.

# AV1 Video Converter Documentation

This file contains comprehensive documentation for the AV1 Video Converter application.

## Project Overview

### Project Goals
This project aims to create a user-friendly GUI application for converting video files to the AV1 codec format, which offers better compression than older video codecs while maintaining high quality. The key goals are:

1. Make high-quality AV1 encoding accessible to non-technical users
2. Optimize encoding parameters automatically based on source video characteristics
3. Provide a clean, informative interface with real-time progress tracking
4. Support batch processing of multiple files with intelligent quality settings
5. Use ab-av1 tool for optimal VMAF-based encoding

### Project Structure
The application has a modular architecture:

- **convert_app/**: The main package containing all application modules
  - **main.py**: Initializes and runs the application
  - **utils.py**: Utility functions for logging and common operations
  - **video_analysis.py**: Functions for analyzing video characteristics
  - **video_conversion.py**: Core video conversion functionality
  - **ab_av1_wrapper.py**: Wrapper for ab-av1 tool integration
  - **gui/**: Package containing all GUI-related modules
    - **main_window.py**: Main application window implementation
    - **operations.py**: GUI operation handlers
    - **base.py**: Base GUI components
    - **tabs/**: Directory containing tab implementations
      - **main_tab.py**: Main conversion tab
      - **settings_tab.py**: Settings configuration tab
      - **log_tab.py**: Log display tab

## Technical Implementation

### Core Technology Stack
The application uses:
- Python with Tkinter for the GUI
- ab-av1 tool for AV1 encoding (which uses FFmpeg with SVT-AV1 encoder)
- File analysis to determine optimal encoding parameters
- Multi-threading to keep the UI responsive during conversion

### Key Features
- VMAF-based automatic quality targeting via ab-av1
- Two-phase encoding process: CRF search followed by actual encoding
- Automatic video analysis for optimal encoding settings
- Smart codec detection to skip already-encoded files
- Progress tracking with estimated time remaining
- Multi-threaded operation for responsive UI
- Detailed logging and statistics during conversion
- Batch processing with resumable state

## AB-AV1 Integration

### What is AB-AV1?
AB-AV1 is a video encoding tool focused on automated AV1 encoding with VMAF-based quality control. Key features include:

1. **CRF Auto-Calculation**: Uses VMAF samples to automatically calculate the most efficient CRF values
2. **Quality-Based Encoding**: Maintains consistent visual quality rather than targeting bitrate
3. **Optimized AV1 Settings**: Uses optimized encoder parameters for the AV1 codec

### ab-av1 Commands Used
The application uses the ab-av1 "auto-encode" command which performs:
1. CRF search - Testing multiple CRF values to find the optimal one meeting the target VMAF score
2. Full encoding using the determined CRF value

### Integration Method
The application integrates ab-av1 through:
1. A dedicated wrapper class in ab_av1_wrapper.py
2. Subprocess communication for executing and monitoring the tool
3. Output parsing to extract progress information, CRF values, and VMAF scores
4. Enhanced error handling and logging

## User Guide

### Application Interface
The application has three main tabs:
1. **Convert Tab**: Main conversion interface with file selection and progress tracking
2. **Settings Tab**: Configuration options for encoding
3. **Log Tab**: Detailed conversion logs and statistics

### Quality Presets
Three quality presets are available:
- **High Quality**: Uses a lower preset value (4) for better quality, slower encoding
- **Balanced**: Default option with preset 6, balanced between quality and speed
- **High Compression**: Uses a higher preset value (8) for faster encoding, smaller files

### Workflow
1. Select input and output folders
2. Choose file types to process
3. Configure quality settings
4. Start the conversion
5. Monitor progress with detailed statistics

## Advanced Features

### VMAF Targeting
The application targets a VMAF score of 95 by default, which provides visually transparent quality for most content.

### CRF Selection
Rather than manually setting CRF values, the application:
1. Analyzes sample sections of the video
2. Tests multiple CRF values to find the optimal setting
3. Chooses the highest CRF (most compression) that still meets the target VMAF score

### Statistics Tracking
The application tracks and displays:
- VMAF scores achieved for each file
- CRF values determined by the analysis
- Size reduction percentages
- Encoding times

## Technical Reference

### Core Modules
- **ab_av1_wrapper.py**: Handles interaction with the ab-av1 tool
- **video_conversion.py**: Manages the conversion process and file handling
- **video_analysis.py**: Provides basic video information analysis

### GUI Components
- **main_window.py**: Creates the application window and manages the overall UI
- **main_tab.py**: Implements the conversion interface
- **settings_tab.py**: Implements the settings configuration interface
- **log_tab.py**: Implements the log display and statistics

### Data Flow
1. User selects files through the GUI
2. Application analyzes each video file
3. ab-av1 tool determines optimal CRF value through VMAF sampling
4. Full encoding is performed with the determined parameters
5. Progress and results are reported back to the UI
