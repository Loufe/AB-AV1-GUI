#src/convert.py
#!/usr/bin/env python3
"""
AV1 Video Converter - Launcher Script

This is the main entry point for the AV1 Video Converter application.
It imports and calls the main function from the convert_app package.
"""
import sys
import os
import traceback # Import traceback for better error reporting

# Make sure the project root directory (containing convert_app) is in the Python path
# This allows running 'python convert.py' from the root directory
project_root = os.path.dirname(os.path.abspath(__file__))
if project_root not in sys.path:
    sys.path.insert(0, project_root)

# Import the main function
try:
    from convert_app.main import main as run_application # Rename imported main
except ImportError as e:
    print(f"Error importing application components: {e}", file=sys.stderr)
    print("Please ensure the application structure is correct and dependencies are installed.", file=sys.stderr)
    # Show full traceback for debugging import issues
    print("\n--- Import Traceback ---", file=sys.stderr)
    traceback.print_exc()
    print("--- End Traceback ---\n", file=sys.stderr)
    input("Press Enter to exit...") # Keep console open
    sys.exit(1)
except Exception as e:
    print(f"An unexpected error occurred during initial import: {e}", file=sys.stderr)
    traceback.print_exc()
    input("Press Enter to exit...") # Keep console open
    sys.exit(1)


# Main execution block
if __name__ == "__main__":
    try:
        run_application() # Call the imported main function
    except Exception as e:
        # Catch any exceptions that might escape the main function's error handling
        print(f"\n--- UNHANDLED APPLICATION ERROR ---", file=sys.stderr)
        print(f"An unexpected error occurred: {e}", file=sys.stderr)
        print(f"Please check the logs for more details.", file=sys.stderr)
        print("\n--- Traceback ---", file=sys.stderr)
        traceback.print_exc()
        print("--- End Traceback ---\n", file=sys.stderr)
        input("Press Enter to exit...") # Keep console open
        sys.exit(1)
    finally:
        # This block executes whether there was an error or not
        print("\nApplication finished.")
        # Optional: keep console open after normal exit if desired
        # input("Press Enter to exit...")