# src/gui/tabs/convert_tab.py
"""
Convert tab module for the AV1 Video Converter application.

This tab allows users to:
- Add files/folders to a conversion queue
- Configure output mode per item (Replace, Suffix, Separate Folder)
- Start/stop batch conversions
- Monitor conversion progress
"""

import tkinter as tk
from tkinter import ttk

from src.config import DEFAULT_VMAF_TARGET
from src.gui.base import ToolTip


def create_convert_tab(gui):
    """Create the main conversion tab."""
    main_frame = ttk.Frame(gui.convert_tab)
    main_frame.pack(fill="both", expand=True, padx=10, pady=10)
    main_frame.columnconfigure(0, weight=1)
    main_frame.rowconfigure(2, weight=1)  # Tree row expands

    # --- Row 0: Control buttons ---
    controls_frame = ttk.Frame(main_frame)
    controls_frame.grid(row=0, column=0, sticky="ew", pady=(0, 5))

    # Left side: Queue management buttons
    left_buttons = ttk.Frame(controls_frame)
    left_buttons.pack(side="left")

    gui.add_folder_button = ttk.Button(left_buttons, text="+ Add Folder", command=gui.on_add_folder_to_queue)
    gui.add_folder_button.pack(side="left", padx=(0, 5))
    ToolTip(gui.add_folder_button, "Add a folder to the conversion queue")

    gui.add_files_button = ttk.Button(left_buttons, text="+ Add Files", command=gui.on_add_files_to_queue)
    gui.add_files_button.pack(side="left", padx=5)
    ToolTip(gui.add_files_button, "Add individual video files to the queue")

    gui.remove_queue_button = ttk.Button(left_buttons, text="Remove", command=gui.on_remove_from_queue)
    gui.remove_queue_button.pack(side="left", padx=5)
    ToolTip(gui.remove_queue_button, "Remove selected items from queue")

    gui.clear_queue_button = ttk.Button(left_buttons, text="Clear", command=gui.on_clear_queue)
    gui.clear_queue_button.pack(side="left", padx=5)
    ToolTip(gui.clear_queue_button, "Clear all items from queue")

    # Right side: Conversion control buttons
    right_buttons = ttk.Frame(controls_frame)
    right_buttons.pack(side="right")

    gui.start_button = ttk.Button(right_buttons, text="Start Conversion", command=gui.on_start_conversion)
    gui.start_button.pack(side="left", padx=5)
    ToolTip(gui.start_button, "Begin converting all queued items")

    gui.stop_button = ttk.Button(
        right_buttons, text="Stop After File", command=gui.on_stop_conversion, state="disabled"
    )
    gui.stop_button.pack(side="left", padx=5)
    ToolTip(gui.stop_button, "Stop after current file completes")

    gui.force_stop_button = ttk.Button(
        right_buttons, text="Force Stop", command=gui.on_force_stop_conversion, state="disabled"
    )
    gui.force_stop_button.pack(side="left", padx=(5, 0))
    ToolTip(gui.force_stop_button, "Immediately stop conversion")

    # --- Row 1: Progress bar, status, and time display ---
    progress_frame = ttk.Frame(main_frame)
    progress_frame.grid(row=1, column=0, sticky="ew", pady=(0, 5))
    progress_frame.columnconfigure(1, weight=1)

    gui.overall_progress = ttk.Progressbar(progress_frame, orient="horizontal", mode="determinate")
    gui.overall_progress.grid(row=0, column=0, sticky="ew", padx=(0, 10))
    progress_frame.columnconfigure(0, weight=1)

    gui.status_label = ttk.Label(progress_frame, text="Ready - Add items to queue", anchor="w")
    gui.status_label.grid(row=0, column=1, sticky="ew", padx=(0, 10))

    # Time display frame for both elapsed and remaining time
    time_frame = ttk.Frame(progress_frame)
    time_frame.grid(row=0, column=2, sticky="e")

    ttk.Label(time_frame, text="Total Time:").pack(side="left", padx=(0, 5))
    gui.total_elapsed_label = ttk.Label(time_frame, text="-")
    gui.total_elapsed_label.pack(side="left")

    ttk.Label(time_frame, text="|").pack(side="left", padx=(8, 8))

    ttk.Label(time_frame, text="Est. Remaining:").pack(side="left", padx=(0, 5))
    gui.total_remaining_label = ttk.Label(time_frame, text="-")
    gui.total_remaining_label.pack(side="left")

    # --- Row 2: Queue Tree with vertical scrollbar ---
    tree_container = ttk.Frame(main_frame)
    tree_container.grid(row=2, column=0, sticky="nsew", pady=5)
    tree_container.columnconfigure(0, weight=1)
    tree_container.rowconfigure(0, weight=1)

    columns = ("output", "status", "progress")
    gui.queue_tree = ttk.Treeview(
        tree_container, columns=columns, show="tree headings", selectmode="extended"
    )

    gui.queue_tree.heading("#0", text="Name", anchor="w")
    gui.queue_tree.heading("output", text="Output Mode", anchor="w")
    gui.queue_tree.heading("status", text="Status", anchor="w")
    gui.queue_tree.heading("progress", text="Progress", anchor="e")

    gui.queue_tree.column("#0", width=350, minwidth=200, stretch=True)
    gui.queue_tree.column("output", width=120, minwidth=100, stretch=False)
    gui.queue_tree.column("status", width=100, minwidth=80, stretch=False)
    gui.queue_tree.column("progress", width=80, minwidth=60, stretch=False, anchor="e")

    scroll_y = ttk.Scrollbar(tree_container, orient="vertical", command=gui.queue_tree.yview)
    gui.queue_tree.configure(yscrollcommand=scroll_y.set)

    gui.queue_tree.grid(row=0, column=0, sticky="nsew")
    scroll_y.grid(row=0, column=1, sticky="ns")

    # Click to expand/collapse folders
    def _on_tree_click(event):
        item_id = gui.queue_tree.identify_row(event.y)
        if item_id and gui.queue_tree.get_children(item_id):
            gui.queue_tree.focus(item_id)
            gui.queue_tree.item(item_id, open=not gui.queue_tree.item(item_id, "open"))

    gui.queue_tree.bind("<Button-1>", _on_tree_click, add="+")

    # Expand/collapse icon updates
    def _on_tree_open(event):
        item_id = gui.queue_tree.focus()
        if item_id:
            text = gui.queue_tree.item(item_id, "text")
            if text.startswith("▶"):
                gui.queue_tree.item(item_id, text=text.replace("▶", "▼", 1))

    def _on_tree_close(event):
        item_id = gui.queue_tree.focus()
        if item_id:
            text = gui.queue_tree.item(item_id, "text")
            if text.startswith("▼"):
                gui.queue_tree.item(item_id, text=text.replace("▼", "▶", 1))

    gui.queue_tree.bind("<<TreeviewOpen>>", _on_tree_open)
    gui.queue_tree.bind("<<TreeviewClose>>", _on_tree_close)

    # Selection change updates properties panel
    gui.queue_tree.bind("<<TreeviewSelect>>", lambda e: gui.on_queue_selection_changed())

    # --- Row 3: Total row (non-scrolling) ---
    total_container = ttk.Frame(main_frame)
    total_container.grid(row=3, column=0, sticky="ew", pady=(0, 5))
    total_container.columnconfigure(0, weight=1)

    gui.queue_total_tree = ttk.Treeview(
        total_container, columns=columns, show="tree", height=1, selectmode="none"
    )
    gui.queue_total_tree.column("#0", width=350, minwidth=200, stretch=True)
    gui.queue_total_tree.column("output", width=120, minwidth=100, stretch=False)
    gui.queue_total_tree.column("status", width=100, minwidth=80, stretch=False)
    gui.queue_total_tree.column("progress", width=80, minwidth=60, stretch=False, anchor="e")

    gui.queue_total_tree.insert("", "end", iid="total", text="Total", values=("", "0 items", "—"))
    gui.queue_total_tree.grid(row=0, column=0, sticky="ew", padx=(0, 17))

    gui.queue_total_tree.bind("<Button-1>", lambda e: "break")

    # --- Row 4: Properties panel for selected item ---
    gui.queue_properties_frame = ttk.LabelFrame(main_frame, text="Selected Item")
    gui.queue_properties_frame.grid(row=4, column=0, sticky="ew", pady=5)
    gui.queue_properties_frame.columnconfigure(1, weight=1)

    # Output Mode dropdown
    ttk.Label(gui.queue_properties_frame, text="Output Mode:").grid(row=0, column=0, sticky="w", padx=10, pady=5)
    gui.item_output_mode = tk.StringVar(value="replace")
    gui.item_mode_combo = ttk.Combobox(
        gui.queue_properties_frame, textvariable=gui.item_output_mode, width=18, state="readonly"
    )
    gui.item_mode_combo["values"] = ("replace", "suffix", "separate_folder")
    gui.item_mode_combo.grid(row=0, column=1, sticky="w", padx=5, pady=5)
    gui.item_mode_combo.bind("<<ComboboxSelected>>", lambda e: gui.on_item_output_mode_changed())

    # Suffix entry
    ttk.Label(gui.queue_properties_frame, text="Suffix:").grid(row=0, column=2, sticky="w", padx=(20, 5), pady=5)
    gui.item_suffix = tk.StringVar(value="_av1")
    gui.item_suffix_entry = ttk.Entry(gui.queue_properties_frame, textvariable=gui.item_suffix, width=10)
    gui.item_suffix_entry.grid(row=0, column=3, sticky="w", padx=5, pady=5)
    gui.item_suffix_entry.bind("<FocusOut>", lambda e: gui.on_item_suffix_changed())

    # Output folder
    ttk.Label(gui.queue_properties_frame, text="Output Folder:").grid(row=1, column=0, sticky="w", padx=10, pady=5)
    gui.item_output_folder = tk.StringVar()
    gui.item_folder_entry = ttk.Entry(gui.queue_properties_frame, textvariable=gui.item_output_folder)
    gui.item_folder_entry.grid(row=1, column=1, columnspan=2, sticky="ew", padx=5, pady=5)
    ttk.Button(gui.queue_properties_frame, text="Browse...", command=gui.on_browse_item_output_folder).grid(
        row=1, column=3, padx=(5, 10), pady=5
    )

    # Source path (read-only)
    ttk.Label(gui.queue_properties_frame, text="Source:").grid(row=2, column=0, sticky="w", padx=10, pady=(5, 10))
    gui.item_source_label = ttk.Label(gui.queue_properties_frame, text="No item selected", anchor="w")
    gui.item_source_label.grid(row=2, column=1, columnspan=3, sticky="ew", padx=5, pady=(5, 10))

    # Initially hide properties panel until selection
    gui.queue_properties_frame.grid_remove()

    # --- Row 5: Current file progress (same as old main_tab) ---
    file_frame = ttk.LabelFrame(main_frame, text="Current File")
    file_frame.grid(row=5, column=0, sticky="ew", pady=5)
    file_frame.columnconfigure(1, weight=1)

    gui.current_file_label = ttk.Label(file_frame, text="No file processing", wraplength=650, justify=tk.LEFT)
    gui.current_file_label.grid(row=0, column=0, columnspan=3, sticky="w", padx=5, pady=5)

    ttk.Label(file_frame, text="Quality Detection:").grid(row=1, column=0, sticky="w", padx=5, pady=2)
    gui.quality_progress = ttk.Progressbar(file_frame, orient="horizontal", length=100, mode="determinate")
    gui.quality_progress.grid(row=1, column=1, sticky="ew", padx=5, pady=2)
    gui.quality_percent_label = ttk.Label(file_frame, text="0%", width=5, anchor="w")
    gui.quality_percent_label.grid(row=1, column=2, sticky="w", padx=(0, 5), pady=2)

    ttk.Label(file_frame, text="Encoding:").grid(row=2, column=0, sticky="w", padx=5, pady=2)
    gui.encoding_progress = ttk.Progressbar(file_frame, orient="horizontal", length=100, mode="determinate")
    gui.encoding_progress.grid(row=2, column=1, sticky="ew", padx=5, pady=2)
    gui.encoding_percent_label = ttk.Label(file_frame, text="0%", width=5, anchor="w")
    gui.encoding_percent_label.grid(row=2, column=2, sticky="w", padx=(0, 5), pady=2)

    # --- Row 6: Conversion Details ---
    details_frame = ttk.LabelFrame(main_frame, text="Conversion Details")
    details_frame.grid(row=6, column=0, sticky="ew", pady=5)

    details_grid = ttk.Frame(details_frame)
    details_grid.pack(fill="x", padx=5, pady=10)

    # Left column
    left_col = ttk.Frame(details_grid)
    left_col.pack(side="left", fill="x", expand=True, padx=(0, 10))

    ttk.Label(left_col, text="Original Format:").grid(row=0, column=0, sticky="w", pady=3)
    gui.orig_format_label = ttk.Label(left_col, text="-")
    gui.orig_format_label.grid(row=0, column=1, sticky="w", pady=3)

    ttk.Label(left_col, text="Original Size:").grid(row=1, column=0, sticky="w", pady=3)
    gui.orig_size_label = ttk.Label(left_col, text="-")
    gui.orig_size_label.grid(row=1, column=1, sticky="w", pady=3)

    ttk.Label(left_col, text="VMAF Target:").grid(row=2, column=0, sticky="w", pady=3)
    gui.vmaf_label = ttk.Label(left_col, text=f"{DEFAULT_VMAF_TARGET}")
    gui.vmaf_label.grid(row=2, column=1, sticky="w", pady=3)

    ttk.Label(left_col, text="Encoding Settings:").grid(row=3, column=0, sticky="w", pady=3)
    gui.encoding_settings_label = ttk.Label(left_col, text="-")
    gui.encoding_settings_label.grid(row=3, column=1, sticky="w", pady=3)

    # Right column
    right_col = ttk.Frame(details_grid)
    right_col.pack(side="right", fill="x", expand=True, padx=(10, 0))

    ttk.Label(right_col, text="Elapsed Time:").grid(row=0, column=0, sticky="w", pady=3)
    gui.elapsed_label = ttk.Label(right_col, text="-")
    gui.elapsed_label.grid(row=0, column=1, sticky="w", pady=3)

    ttk.Label(right_col, text="Est. Remaining (Enc):").grid(row=1, column=0, sticky="w", pady=3)
    gui.eta_label = ttk.Label(right_col, text="-")
    gui.eta_label.grid(row=1, column=1, sticky="w", pady=3)

    ttk.Label(right_col, text="Est. Final Size:").grid(row=2, column=0, sticky="w", pady=3)
    gui.output_size_label = ttk.Label(right_col, text="-")
    gui.output_size_label.grid(row=2, column=1, sticky="w", pady=3)
