# src/gui/tabs/analysis_tab.py
"""
Analysis tab module for folder scanning.

This tab allows users to:
- Scan folders to see conversion statistics without converting
- View estimated savings and time for each file/folder
"""

import os
import subprocess
import sys
import tkinter as tk
from tkinter import ttk

from src.gui.base import ToolTip, TreeviewRowTooltip


def _open_in_explorer(path: str) -> None:
    """Open a file or folder in the native file explorer."""
    if sys.platform == "win32":
        os.startfile(path)
    elif sys.platform == "darwin":
        subprocess.run(["open", path], check=False)
    else:
        subprocess.run(["xdg-open", path], check=False)


def _reveal_in_explorer(file_path: str) -> None:
    """Open the containing folder and select the file."""
    if sys.platform == "win32":
        subprocess.run(["explorer", "/select,", file_path], check=False)
    elif sys.platform == "darwin":
        subprocess.run(["open", "-R", file_path], check=False)
    else:
        # Linux: just open the containing folder
        subprocess.run(["xdg-open", os.path.dirname(file_path)], check=False)


def create_analysis_tab(gui):
    """Create the analysis tab for folder scanning."""
    main = ttk.Frame(gui.analysis_tab)
    main.pack(fill="both", expand=True, padx=10, pady=10)

    # Use pack for main sections so they have independent layouts
    main.columnconfigure(0, weight=1)
    main.rowconfigure(2, weight=1)  # Tree row expands

    # --- Row 0: Folder selection and buttons (in its own frame) ---
    controls_frame = ttk.Frame(main)
    controls_frame.grid(row=0, column=0, sticky="ew", pady=(0, 5))
    controls_frame.columnconfigure(1, weight=1)  # Entry expands

    ttk.Label(controls_frame, text="Root Folder:").grid(row=0, column=0, sticky="w", padx=(0, 5))

    scan_entry = ttk.Entry(controls_frame, textvariable=gui.input_folder, width=50)
    scan_entry.grid(row=0, column=1, sticky="ew", padx=5)

    ttk.Button(controls_frame, text="Browse...", command=gui.on_browse_input_folder).grid(row=0, column=2, padx=5)

    gui.analyze_button = ttk.Button(controls_frame, text="Analyze", command=gui.on_analyze_folders)
    gui.analyze_button.grid(row=0, column=3, padx=5)
    ToolTip(gui.analyze_button, "Scan folders to find convertible files and estimate savings.")

    gui.analyze_quality_button = ttk.Button(
        controls_frame, text="Analyze Quality", command=gui.on_analyze_quality, state="disabled"
    )
    gui.analyze_quality_button.grid(row=0, column=4, padx=5)
    ToolTip(gui.analyze_quality_button, "Run CRF search on selected files for accurate predictions (~1 min/file)")

    gui.stop_analyze_button = ttk.Button(controls_frame, text="Stop", command=gui.on_stop_analysis, state="disabled")
    gui.stop_analyze_button.grid(row=0, column=5, padx=(5, 0))
    ToolTip(gui.stop_analyze_button, "Stop the current analysis scan.")

    # --- Row 1: Progress bar (1/3) and status label (2/3) ---
    progress_frame = ttk.Frame(main)
    progress_frame.grid(row=1, column=0, sticky="ew", pady=(0, 5))
    progress_frame.columnconfigure(0, weight=1)  # Progress bar: 1/3
    progress_frame.columnconfigure(1, weight=2)  # Status label: 2/3

    gui.analysis_progress = ttk.Progressbar(progress_frame, orient="horizontal", mode="determinate")
    gui.analysis_progress.grid(row=0, column=0, sticky="ew", padx=(0, 10))

    # Fixed width prevents label from resizing during rapid updates; text is truncated dynamically
    gui.analysis_status_label = ttk.Label(progress_frame, text="Ready to analyze", anchor="w", width=100)
    gui.analysis_status_label.grid(row=0, column=1, sticky="ew")

    # --- Row 2: Tree with vertical scrollbar only ---
    tree_container = ttk.Frame(main)
    tree_container.grid(row=2, column=0, sticky="nsew", pady=5)
    tree_container.columnconfigure(0, weight=1)
    tree_container.rowconfigure(0, weight=1)

    columns = ("size", "savings", "time", "efficiency")
    gui.analysis_tree = ttk.Treeview(
        tree_container, columns=columns, show="tree headings", selectmode="extended", style="Analysis.Treeview"
    )

    gui.analysis_tree.heading("#0", text="Name", anchor="w", command=lambda: gui.sort_analysis_tree("#0"))
    gui.analysis_tree.heading("size", text="Size", anchor="e", command=lambda: gui.sort_analysis_tree("size"))
    gui.analysis_tree.heading(
        "savings", text="Est. Savings", anchor="e", command=lambda: gui.sort_analysis_tree("savings")
    )
    gui.analysis_tree.heading("time", text="Est. Time", anchor="e", command=lambda: gui.sort_analysis_tree("time"))
    gui.analysis_tree.heading(
        "efficiency", text="Efficiency", anchor="e", command=lambda: gui.sort_analysis_tree("efficiency")
    )

    # Name column stretches to fill space, data columns are fixed
    gui.analysis_tree.column("#0", width=300, minwidth=150, stretch=True)
    gui.analysis_tree.column("size", width=70, minwidth=70, stretch=False, anchor="e")
    gui.analysis_tree.column("savings", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_tree.column("time", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_tree.column("efficiency", width=80, minwidth=80, stretch=False, anchor="e")

    # Configure tags for status coloring (subtle, professional colors)
    gui.analysis_tree.tag_configure("done", foreground="#2E7D32")  # Dark green
    gui.analysis_tree.tag_configure("skip", foreground="#C65D00")  # Muted amber

    scroll_y = ttk.Scrollbar(tree_container, orient="vertical", command=gui.analysis_tree.yview)
    gui.analysis_tree.configure(yscrollcommand=scroll_y.set)

    gui.analysis_tree.grid(row=0, column=0, sticky="nsew")
    scroll_y.grid(row=0, column=1, sticky="ns")

    # Click anywhere on a folder row to expand/collapse (not just the +/- button)
    def _on_tree_click(event):
        item_id = gui.analysis_tree.identify_row(event.y)
        if item_id and gui.analysis_tree.get_children(item_id):  # Has children = folder
            # Set focus so TreeviewOpen/Close event handlers work correctly
            gui.analysis_tree.focus(item_id)
            gui.analysis_tree.item(item_id, open=not gui.analysis_tree.item(item_id, "open"))

    gui.analysis_tree.bind("<Button-1>", _on_tree_click, add="+")

    # Handle native expand/collapse indicator clicks (focus is set correctly for these)
    def _on_tree_open(event):
        item_id = gui.analysis_tree.focus()
        if item_id:
            text = gui.analysis_tree.item(item_id, "text")
            if text.startswith("▶"):
                gui.analysis_tree.item(item_id, text=text.replace("▶", "▼", 1))

    def _on_tree_close(event):
        item_id = gui.analysis_tree.focus()
        if item_id:
            text = gui.analysis_tree.item(item_id, "text")
            if text.startswith("▼"):
                gui.analysis_tree.item(item_id, text=text.replace("▼", "▶", 1))

    gui.analysis_tree.bind("<<TreeviewOpen>>", _on_tree_open)
    gui.analysis_tree.bind("<<TreeviewClose>>", _on_tree_close)

    # Right-click context menu
    context_menu = tk.Menu(gui.analysis_tree, tearoff=0)

    def _show_context_menu(event):
        item_id = gui.analysis_tree.identify_row(event.y)
        if not item_id:
            return

        # Select the right-clicked item
        gui.analysis_tree.selection_set(item_id)

        # Clear menu and rebuild based on item type
        context_menu.delete(0, tk.END)

        # Check if it's a folder (has children) or file
        is_folder = bool(gui.analysis_tree.get_children(item_id))

        if is_folder:
            # Get folder path from tree hierarchy
            # Walk up to build the path from root folder + tree structure
            path_parts = []
            current = item_id
            while current:
                text = gui.analysis_tree.item(current, "text")
                # Remove arrows and emoji prefix
                name = text.replace("▶", "").replace("▼", "").replace("📁", "").replace("🎬", "").strip()
                path_parts.insert(0, name)
                current = gui.analysis_tree.parent(current)

            folder_path = os.path.join(gui.input_folder.get(), *path_parts)

            context_menu.add_command(
                label="Open in Explorer",
                command=lambda p=folder_path: _open_in_explorer(p),
            )
        else:
            # It's a file - get path from GUI's lookup method
            file_path = gui.get_file_path_for_tree_item(item_id)

            if file_path:
                context_menu.add_command(
                    label="Open File",
                    command=lambda p=file_path: _open_in_explorer(p),
                )
                context_menu.add_command(
                    label="Show in Explorer",
                    command=lambda p=file_path: _reveal_in_explorer(p),
                )

        context_menu.tk_popup(event.x_root, event.y_root)

    gui.analysis_tree.bind("<Button-3>", _show_context_menu)

    # Update quality button state when selection changes
    gui.analysis_tree.bind("<<TreeviewSelect>>", lambda e: gui.update_quality_button_state())

    # Set up row tooltips for file status explanations
    TreeviewRowTooltip(gui.analysis_tree, gui.get_analysis_tree_tooltip)

    # --- Row 3: Fixed total row (non-scrolling, always visible) ---
    # Use a separate single-row Treeview with matching columns for perfect alignment
    total_container = ttk.Frame(main)
    total_container.grid(row=3, column=0, sticky="ew", pady=(0, 0))
    total_container.columnconfigure(0, weight=1)

    gui.analysis_total_tree = ttk.Treeview(
        total_container, columns=columns, show="tree", height=1, selectmode="none"
    )

    # Match column widths exactly with main tree
    gui.analysis_total_tree.column("#0", width=300, minwidth=150, stretch=True)
    gui.analysis_total_tree.column("size", width=70, minwidth=70, stretch=False, anchor="e")
    gui.analysis_total_tree.column("savings", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_total_tree.column("time", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_total_tree.column("efficiency", width=80, minwidth=80, stretch=False, anchor="e")

    # Insert the total row (will be updated dynamically)
    gui.analysis_total_tree.insert("", "end", iid="total", text="Total", values=("—", "—", "—", "—"))

    # Add padding on right to align with scrollbar in main tree
    gui.analysis_total_tree.grid(row=0, column=0, sticky="ew", padx=(0, 17))

    # Disable interaction with total row
    gui.analysis_total_tree.bind("<Button-1>", lambda e: "break")
    gui.analysis_total_tree.bind("<Double-Button-1>", lambda e: "break")
