"""
Main tab module for the AV1 Video Converter application.
"""
import tkinter as tk
from tkinter import ttk
import math # For ceil

from convert_app.gui.base import ToolTip
# Import the constant directly
from convert_app.utils import DEFAULT_VMAF_TARGET

def create_main_tab(gui):
    """Create the main conversion tab"""
    main_frame = ttk.Frame(gui.main_tab)
    main_frame.pack(fill="both", expand=True, padx=10, pady=10)

    # Input/Output folder selection frame
    folder_frame = ttk.Frame(main_frame)
    folder_frame.grid(row=0, column=0, columnspan=2, sticky="ew", pady=(10, 5))

    input_label = ttk.Label(folder_frame, text="Input Folder", style="Header.TLabel")
    input_label.grid(row=0, column=0, sticky="w")
    input_entry = ttk.Entry(folder_frame, textvariable=gui.input_folder, width=30)
    input_entry.grid(row=0, column=1, sticky="ew", padx=5)
    input_btn = ttk.Button(folder_frame, text="Browse...", command=gui.browse_input_folder)
    input_btn.grid(row=0, column=2, padx=(0, 10))

    output_label = ttk.Label(folder_frame, text="Output Folder", style="Header.TLabel")
    output_label.grid(row=0, column=3, sticky="w", padx=(10, 0))
    output_entry = ttk.Entry(folder_frame, textvariable=gui.output_folder, width=30)
    output_entry.grid(row=0, column=4, sticky="ew", padx=5)
    output_btn = ttk.Button(folder_frame, text="Browse...", command=gui.browse_output_folder)
    output_btn.grid(row=0, column=5)

    folder_frame.columnconfigure(1, weight=1)
    folder_frame.columnconfigure(4, weight=1)

    # Conversion controls
    control_frame = ttk.Frame(main_frame)
    control_frame.grid(row=4, column=0, columnspan=2, sticky="ew", pady=(20, 10))

    gui.start_button = ttk.Button(control_frame, text="Start Conversion", command=gui.start_conversion)
    gui.start_button.pack(side="left", padx=5)
    ToolTip(gui.start_button, "Begin converting all selected video types in the input folder.")

    # Renamed Stop Button
    gui.stop_button = ttk.Button(control_frame, text="Stop After Current File", command=gui.stop_conversion, state="disabled")
    gui.stop_button.pack(side="left", padx=5)
    ToolTip(gui.stop_button, "Signal the converter to stop gracefully after the currently processing file is finished.")

    gui.force_stop_button = ttk.Button(control_frame, text="Force Stop", command=gui.force_stop_conversion, state="disabled")
    gui.force_stop_button.pack(side="left", padx=5)
    ToolTip(gui.force_stop_button, "Immediately terminate the current encoding process.\nMay leave temporary files if cleanup fails.")

    # Overall progress bar
    ttk.Label(main_frame, text="Overall Progress").grid(row=5, column=0, sticky="w", pady=(10, 5))
    gui.overall_progress = ttk.Progressbar(main_frame, orient="horizontal", length=100, mode="determinate")
    gui.overall_progress.grid(row=6, column=0, columnspan=2, sticky="ew", padx=5)

    # Status and total elapsed time
    status_frame = ttk.Frame(main_frame)
    status_frame.grid(row=7, column=0, columnspan=2, sticky="ew", pady=(5, 20))
    status_frame.columnconfigure(0, weight=3)
    status_frame.columnconfigure(1, weight=1)
    
    gui.status_label = ttk.Label(status_frame, text="Ready")
    gui.status_label.grid(row=0, column=0, sticky="w")
    
    total_time_frame = ttk.Frame(status_frame)
    total_time_frame.grid(row=0, column=1, sticky="e")
    ttk.Label(total_time_frame, text="Total Time:").pack(side="left", padx=(0, 5))
    gui.total_elapsed_label = ttk.Label(total_time_frame, text="-")
    gui.total_elapsed_label.pack(side="left")

    # Current file progress frame
    file_frame = ttk.LabelFrame(main_frame, text="Current File")
    file_frame.grid(row=8, column=0, columnspan=2, sticky="ew", padx=5, pady=5)
    file_frame.columnconfigure(1, weight=1) # Make progress bars expand

    # Current file label
    gui.current_file_label = ttk.Label(file_frame, text="No file processing", wraplength=650, justify=tk.LEFT) # Allow wrapping
    gui.current_file_label.grid(row=0, column=0, columnspan=3, sticky="w", padx=5, pady=5)

    # --- Dual Progress Bars ---
    # Quality Detection Bar
    ttk.Label(file_frame, text="Quality Detection:").grid(row=1, column=0, sticky="w", padx=5, pady=2)
    gui.quality_progress = ttk.Progressbar(file_frame, orient="horizontal", length=100, mode="determinate")
    gui.quality_progress.grid(row=1, column=1, sticky="ew", padx=5, pady=2)
    gui.quality_percent_label = ttk.Label(file_frame, text="0%", width=5, anchor='w')
    gui.quality_percent_label.grid(row=1, column=2, sticky="w", padx=(0, 5), pady=2)

    # Encoding Bar
    ttk.Label(file_frame, text="Encoding:").grid(row=2, column=0, sticky="w", padx=5, pady=2)
    gui.encoding_progress = ttk.Progressbar(file_frame, orient="horizontal", length=100, mode="determinate")
    gui.encoding_progress.grid(row=2, column=1, sticky="ew", padx=5, pady=2)
    gui.encoding_percent_label = ttk.Label(file_frame, text="0%", width=5, anchor='w')
    gui.encoding_percent_label.grid(row=2, column=2, sticky="w", padx=(0, 5), pady=2)
    # --- End Dual Progress Bars ---


    # Conversion details frame
    details_frame = ttk.LabelFrame(main_frame, text="Conversion Details")
    details_frame.grid(row=9, column=0, columnspan=2, sticky="ew", padx=5, pady=5)

    details_grid = ttk.Frame(details_frame)
    details_grid.pack(fill="x", padx=5, pady=5)

    left_col = ttk.Frame(details_grid)
    left_col.pack(side="left", fill="x", expand=True, padx=(0, 10))
    ttk.Label(left_col, text="Original Format:").grid(row=0, column=0, sticky="w", pady=2)
    gui.orig_format_label = ttk.Label(left_col, text="-")
    gui.orig_format_label.grid(row=0, column=1, sticky="w", pady=2)
    ttk.Label(left_col, text="Original Size:").grid(row=1, column=0, sticky="w", pady=2)
    gui.orig_size_label = ttk.Label(left_col, text="-")
    gui.orig_size_label.grid(row=1, column=1, sticky="w", pady=2)
    # Use imported constant directly
    ttk.Label(left_col, text="VMAF Target:").grid(row=2, column=0, sticky="w", pady=2)
    gui.vmaf_label = ttk.Label(left_col, text=f"{DEFAULT_VMAF_TARGET}") # Show target initially
    gui.vmaf_label.grid(row=2, column=1, sticky="w", pady=2)
    ttk.Label(left_col, text="Encoding Settings:").grid(row=3, column=0, sticky="w", pady=2)
    gui.encoding_settings_label = ttk.Label(left_col, text="-") # Will show CRF/Preset
    gui.encoding_settings_label.grid(row=3, column=1, sticky="w", pady=2)

    right_col = ttk.Frame(details_grid)
    right_col.pack(side="right", fill="x", expand=True, padx=(10, 0))
    ttk.Label(right_col, text="Elapsed Time:").grid(row=0, column=0, sticky="w", pady=2)
    gui.elapsed_label = ttk.Label(right_col, text="-")
    gui.elapsed_label.grid(row=0, column=1, sticky="w", pady=2)
    ttk.Label(right_col, text="Est. Remaining (Enc):").grid(row=1, column=0, sticky="w", pady=2) # Clarify ETA is for encoding phase
    gui.eta_label = ttk.Label(right_col, text="-")
    gui.eta_label.grid(row=1, column=1, sticky="w", pady=2)
    ttk.Label(right_col, text="Est. Final Size:").grid(row=2, column=0, sticky="w", pady=2)
    gui.output_size_label = ttk.Label(right_col, text="-")
    gui.output_size_label.grid(row=2, column=1, sticky="w", pady=2)

    # Conversion Statistics frame
    stats_frame = ttk.LabelFrame(main_frame, text="Overall Statistics (Successful Files)")
    stats_frame.grid(row=10, column=0, columnspan=2, sticky="ew", padx=5, pady=5)

    stats_grid = ttk.Frame(stats_frame)
    stats_grid.pack(fill="x", padx=10, pady=10)
    ttk.Label(stats_grid, text="Avg VMAF Score:").grid(row=0, column=0, sticky="w", padx=5, pady=2)
    gui.vmaf_stats_label = ttk.Label(stats_grid, text="-")
    gui.vmaf_stats_label.grid(row=0, column=1, sticky="w", padx=5, pady=2)
    ttk.Label(stats_grid, text="Avg CRF Value:").grid(row=1, column=0, sticky="w", padx=5, pady=2)
    gui.crf_stats_label = ttk.Label(stats_grid, text="-")
    gui.crf_stats_label.grid(row=1, column=1, sticky="w", padx=5, pady=2)
    ttk.Label(stats_grid, text="Avg Size Reduction:").grid(row=2, column=0, sticky="w", padx=5, pady=2)
    gui.size_stats_label = ttk.Label(stats_grid, text="-")
    gui.size_stats_label.grid(row=2, column=1, sticky="w", padx=5, pady=2)

    # Make UI elements respond to window resizing
    main_frame.columnconfigure(0, weight=1)
    main_frame.rowconfigure(8, weight=1) # Allow file frame to expand if needed
    main_frame.rowconfigure(9, weight=1) # Allow details frame to expand