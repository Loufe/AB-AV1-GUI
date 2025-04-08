#!/usr/bin/env python3
"""
AV1 Video Converter - Main Launcher

This is the main entry point for the AV1 Video Converter application.
It launches the GUI interface from the convert_app package.
"""
import os
import sys
import traceback

# Main entry point
if __name__ == "__main__":
    # Add the current directory to the Python path
    current_dir = os.path.dirname(os.path.abspath(__file__))
    if current_dir not in sys.path:
        sys.path.insert(0, current_dir)
    
    # Import and run the main function from the application
    try:
        from convert_app.main import main
        main()
    except ImportError as e:
        print(f"Error importing the application: {str(e)}")
        print("\nPlease make sure you have all the required dependencies installed:")
        print("- Python 3.6 or higher")
        print("- FFmpeg with SVT-AV1 support")
        print("- Required Python packages")
        traceback.print_exc()
        print("\nPress Enter to exit...")
        input()
    except Exception as e:
        print(f"Error launching the application: {str(e)}")
        traceback.print_exc()
        print("\nPress Enter to exit...")
        input()
