# src/convert.py
#!/usr/bin/env python3
"""
AV1 Video Converter - Launcher Script (within src package)

This is the main entry point for the AV1 Video Converter application.
It imports and calls the main function from the main module within this package.
Run using 'python -m src.convert' from the project root directory.
"""

import sys
import traceback  # Import traceback for better error reporting

# Platform-specific modules for single key press
try:
    import msvcrt  # For Windows

    def wait_for_key(message="Press any key to exit..."):
        print(message, end="", flush=True)
        getch = getattr(msvcrt, "getch", None)
        if getch:
            getch()
        print()  # Print a newline after key press
except ImportError:
    # For Unix/Linux/macOS
    import termios
    import tty

    def wait_for_key(message="Press any key to exit..."):
        if not sys.stdin.isatty():
            print()  # Just print newline and return if not interactive
            return
        print(message, end="", flush=True)
        fd = sys.stdin.fileno()
        old_settings = termios.tcgetattr(fd)
        try:
            tty.setraw(fd)
            sys.stdin.read(1)
        finally:
            termios.tcsetattr(fd, termios.TCSADRAIN, old_settings)
        print()  # Print a newline after key press

# No sys.path manipulation needed when running with 'python -m' from root.

# Import the main function using a relative import
try:
    from .main import main as run_application  # type: ignore[unresolved-import]
except ImportError as e:
    print(f"Error importing '.main' within 'src' package: {e}", file=sys.stderr)
    print(f"Current sys.path: {sys.path}", file=sys.stderr)
    print("Ensure 'main.py' exists in the 'src' directory.", file=sys.stderr)
    # Show full traceback for debugging import issues
    print("\n--- Import Traceback ---", file=sys.stderr)
    traceback.print_exc()
    print("--- End Traceback ---\n", file=sys.stderr)
    wait_for_key()  # Keep console open
    sys.exit(1)
except Exception as e:
    print(f"An unexpected error occurred during initial import: {e}", file=sys.stderr)
    traceback.print_exc()
    wait_for_key()  # Keep console open
    sys.exit(1)


# Main execution block
if __name__ == "__main__":
    exit_code = 0  # Default to success
    try:
        run_application()  # Call the imported main function
    except Exception as e:
        exit_code = 1  # Set error exit code
        # Catch any exceptions that might escape the main function's error handling
        print("\n--- UNHANDLED APPLICATION ERROR ---", file=sys.stderr)
        print(f"An unexpected error occurred: {e}", file=sys.stderr)
        print("Please check the logs for more details.", file=sys.stderr)
        print("\n--- Traceback ---", file=sys.stderr)
        traceback.print_exc()
        print("--- End Traceback ---\n", file=sys.stderr)
        # Don't exit here, let finally run
    finally:
        # This block executes whether there was an error or not
        print("\nApplication finished.")
        # Keep console open until a key is pressed
        wait_for_key()
        sys.exit(exit_code)  # Exit with appropriate code after key press
