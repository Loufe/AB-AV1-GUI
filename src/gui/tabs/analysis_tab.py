# src/gui/tabs/analysis_tab.py
"""
Analysis tab module for folder scanning.

This tab allows users to:
- Scan folders to see conversion statistics without converting
- View estimated savings and time for each file/folder
"""

import os
import tkinter as tk
from tkinter import ttk

from src.gui.base import ToolTip, TreeviewRowTooltip, open_in_explorer, reveal_in_explorer
from src.models import OperationType


def create_analysis_tab(gui):
    """Create the analysis tab for folder scanning."""
    main = ttk.Frame(gui.analysis_tab)
    main.pack(fill="both", expand=True, padx=10, pady=10)

    # Use pack for main sections so they have independent layouts
    main.columnconfigure(0, weight=1)
    main.rowconfigure(1, weight=1)  # Tree row expands

    # --- Row 0: Folder selection and buttons (in its own frame) ---
    controls_frame = ttk.Frame(main)
    controls_frame.grid(row=0, column=0, sticky="ew", pady=(0, 5))
    controls_frame.columnconfigure(1, weight=1)  # Entry expands

    ttk.Label(controls_frame, text="Root Folder:").grid(row=0, column=0, sticky="w", padx=(0, 5))

    scan_entry = ttk.Entry(controls_frame, textvariable=gui.input_folder, width=50)
    scan_entry.grid(row=0, column=1, sticky="ew", padx=5)

    ttk.Button(controls_frame, text="Browse...", command=gui.on_browse_input_folder).grid(row=0, column=2, padx=5)

    gui.analyze_button = ttk.Button(controls_frame, text="Basic Scan", command=gui.on_analyze_folders)
    gui.analyze_button.grid(row=0, column=3, padx=5)
    ToolTip(gui.analyze_button, "Scan folders to find convertible files and estimate savings.")

    gui.add_all_analyze_button = ttk.Button(
        controls_frame, text="Add All: Analyze", command=gui.on_add_all_analyze, state="disabled"
    )
    gui.add_all_analyze_button.grid(row=0, column=4, padx=5)
    ToolTip(gui.add_all_analyze_button, "Add all discovered files to queue for CRF analysis")

    gui.add_all_convert_button = ttk.Button(
        controls_frame, text="Add All: Convert", command=gui.on_add_all_convert, state="disabled"
    )
    gui.add_all_convert_button.grid(row=0, column=5, padx=(5, 0))
    ToolTip(gui.add_all_convert_button, "Add all discovered files to queue for conversion")

    # --- Row 1: Tree with vertical scrollbar only ---
    tree_container = ttk.Frame(main)
    tree_container.grid(row=1, column=0, sticky="nsew", pady=5)
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
    gui.analysis_tree.tag_configure("in_queue", foreground="#1565C0")  # Dark blue - queued file/folder
    gui.analysis_tree.tag_configure("partial_queue", foreground="#64B5F6")  # Light blue - folder with some queued

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

    def _create_context_menu() -> tk.Menu:
        """Create a fresh context menu with consistent styling."""
        return tk.Menu(
            gui.analysis_tree, tearoff=0, background="#ffffff", activebackground="#0078d4", activeforeground="#ffffff"
        )

    def _get_folder_path_from_tree_item(item_id: str) -> str:
        """Reconstruct folder path from tree hierarchy."""
        path_parts = []
        current = item_id
        while current:
            text = gui.analysis_tree.item(current, "text")
            # Remove arrows and emoji prefix
            name = text.replace("▶", "").replace("▼", "").replace("📁", "").replace("🎬", "").strip()
            path_parts.insert(0, name)
            current = gui.analysis_tree.parent(current)
        return os.path.join(gui.input_folder.get(), *path_parts)

    def _add_selected_items_to_queue(selected_items: tuple[str, ...], operation_type: OperationType) -> None:
        """Add multiple selected items from analysis tree to queue with specified operation type."""
        # Collect items first
        items: list[tuple[str, bool]] = []
        for item_id in selected_items:
            is_folder = bool(gui.analysis_tree.get_children(item_id))
            if is_folder:
                folder_path = _get_folder_path_from_tree_item(item_id)
                items.append((folder_path, True))
            else:
                file_path = gui.get_file_path_for_tree_item(item_id)
                if file_path:
                    items.append((file_path, False))

        # Use bulk add - shows preview if there are conflicts
        if items:
            gui.add_items_to_queue(items, operation_type, force_preview=len(items) > 1)

    def _show_context_menu(event):
        item_id = gui.analysis_tree.identify_row(event.y)
        if not item_id:
            return

        # Preserve multi-selection if right-clicking within existing selection
        current_selection = gui.analysis_tree.selection()
        if item_id in current_selection and len(current_selection) > 1:
            # Keep multi-selection
            selected_items = current_selection
        else:
            # Select just the right-clicked item
            gui.analysis_tree.selection_set(item_id)
            selected_items = (item_id,)

        # Create fresh menu each time (fixes Windows shadow rendering issues)
        menu = _create_context_menu()

        # Multi-selection: show batch actions
        if len(selected_items) > 1:
            menu.add_command(
                label=f"Add {len(selected_items)} Items to Queue: Convert",
                command=lambda items=selected_items: _add_selected_items_to_queue(items, OperationType.CONVERT),
            )
            menu.add_command(
                label=f"Add {len(selected_items)} Items to Queue: Analyze",
                command=lambda items=selected_items: _add_selected_items_to_queue(items, OperationType.ANALYZE),
            )

            menu.update_idletasks()
            menu.tk_popup(event.x_root, event.y_root)
            return

        # Single selection: show item-specific actions
        is_folder = bool(gui.analysis_tree.get_children(item_id))

        if is_folder:
            folder_path = _get_folder_path_from_tree_item(item_id)

            menu.add_command(label="Open in Explorer", command=lambda path=folder_path: open_in_explorer(path))
            menu.add_separator()
            menu.add_command(
                label="Add Folder to Queue: Convert",
                command=lambda path=folder_path: gui.add_to_queue(
                    path, is_folder=True, operation_type=OperationType.CONVERT
                ),
            )
            menu.add_command(
                label="Add Folder to Queue: Analyze",
                command=lambda path=folder_path: gui.add_to_queue(
                    path, is_folder=True, operation_type=OperationType.ANALYZE
                ),
            )
        else:
            # It's a file - get path from GUI's lookup method
            file_path = gui.get_file_path_for_tree_item(item_id)

            if file_path:
                menu.add_command(label="Open File", command=lambda path=file_path: open_in_explorer(path))
                menu.add_command(label="Show in Explorer", command=lambda path=file_path: reveal_in_explorer(path))
                menu.add_separator()
                menu.add_command(
                    label="Add to Queue: Convert",
                    command=lambda path=file_path: gui.add_to_queue(
                        path, is_folder=False, operation_type=OperationType.CONVERT
                    ),
                )
                menu.add_command(
                    label="Add to Queue: Analyze",
                    command=lambda path=file_path: gui.add_to_queue(
                        path, is_folder=False, operation_type=OperationType.ANALYZE
                    ),
                )

        # Force geometry recalculation before showing (fixes Windows shadow rendering)
        menu.update_idletasks()
        menu.tk_popup(event.x_root, event.y_root)

    gui.analysis_tree.bind("<Button-3>", _show_context_menu)

    # Double-click opens file/folder in explorer
    def _on_double_click(event):
        item_id = gui.analysis_tree.identify_row(event.y)
        if not item_id:
            return
        is_folder = bool(gui.analysis_tree.get_children(item_id))
        path = _get_folder_path_from_tree_item(item_id) if is_folder else gui.get_file_path_for_tree_item(item_id)
        if path:
            open_in_explorer(path)

    gui.analysis_tree.bind("<Double-Button-1>", _on_double_click)

    # Set up row tooltips for file status explanations
    TreeviewRowTooltip(gui.analysis_tree, gui.get_analysis_tree_tooltip)

    # --- Row 2: Fixed total row (non-scrolling, always visible) ---
    # Use a separate single-row Treeview with matching columns for perfect alignment
    total_container = ttk.Frame(main)
    total_container.grid(row=2, column=0, sticky="ew", pady=(0, 0))
    total_container.columnconfigure(0, weight=1)

    gui.analysis_total_tree = ttk.Treeview(total_container, columns=columns, show="tree", height=1, selectmode="none")

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
    gui.analysis_total_tree.bind("<Button-3>", lambda e: "break")
    gui.analysis_total_tree.bind("<Double-Button-1>", lambda e: "break")
