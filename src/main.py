#src/main.py
"""
Main application logic module for the AV1 Video Converter application.
Defines the main() function to be called by the launcher.
"""
import logging
import sys
import tkinter as tk
from tkinter import messagebox

# Project imports needed within main() or for GUI class
from src.gui.main_window import VideoConverterGUI
from src.utils import setup_logging


def main():
    """Initializes and runs the AV1 Video Converter application."""
    # Setup logging early
    log_file = None
    try:
        log_file = setup_logging()
        logging.info("=== Starting AV1 Video Converter ===")
        if log_file: logging.info(f"Log file: {log_file}")
        logging.info(f"System: {sys.platform}, Python: {sys.version}")
    except Exception as e:
        print(f"FATAL: Failed to set up logging: {e}", file=sys.stderr)
        try: # Try a simple Tkinter message box
            root_err = tk.Tk()
            root_err.withdraw()
            messagebox.showerror("Logging Error", f"Failed to initialize logging:\n{e}\n\nApplication cannot start.")
            root_err.destroy()
        except Exception as tk_e:
            print(f"Could not display Tkinter error message: {tk_e}", file=sys.stderr)
        sys.exit(1)

    # Check for command line arguments (GUI only)
    if len(sys.argv) > 1:
        print("Command line mode is not supported. Please run without arguments.", file=sys.stderr)
        logging.warning("Attempted run with command line arguments (not supported). Exiting.")
        sys.exit(1)

    # Create the root window
    root = tk.Tk()
    app = None  # Initialize app

    try:
        # Create the application instance
        app = VideoConverterGUI(root)

        # Register window protocol for proper cleanup
        root.protocol("WM_DELETE_WINDOW", app.on_exit)

        # Start the Tkinter main event loop
        logging.info("Starting Tkinter main loop...")
        root.mainloop()
        logging.info("Tkinter main loop finished.")

    except Exception as e:
        logging.critical(f"An unhandled error occurred during application startup or runtime: {e}", exc_info=True)
        # Attempt cleanup if app object exists
        if app and hasattr(app, "on_exit"):
            try:
                logging.info("Attempting application cleanup via on_exit due to error...")
                app.on_exit()
            except Exception as cleanup_e:
                logging.error(f"Error during on_exit cleanup: {cleanup_e}", exc_info=True)
        elif root:
             try:
                 logging.info("Attempting to destroy root window due to error...")
                 root.destroy()
             except Exception as destroy_e:
                 logging.exception(f"Error destroying root window during error handling: {destroy_e}")

        # Try to show a final error message
        try:
            err_root = tk.Tk()
            err_root.withdraw()
            messagebox.showerror("Fatal Application Error", f"An unexpected error occurred:\n\n{e}\n\nSee logs for details. Application will exit.")
            err_root.destroy()
        except Exception as tk_e:
            print(f"Could not display final Tkinter error message: {tk_e}", file=sys.stderr)

        sys.exit(1) # Exit with error status

# No `if __name__ == "__main__":` block here anymore.
# This module only DEFINES main().
