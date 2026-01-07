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

from src.config import ANALYSIS_TREE_HEADINGS
from src.gui.base import ToolTip, TreeviewHeaderTooltip, TreeviewRowTooltip, open_in_explorer, reveal_in_explorer
from src.gui.constants import (
    COLOR_BADGE_BACKGROUND,
    COLOR_BADGE_TEXT,
    COLOR_STATUS_DISABLED,
    COLOR_STATUS_INFO,
    COLOR_STATUS_INFO_LIGHT,
    COLOR_STATUS_SUCCESS,
    COLOR_STATUS_WARNING,
    FONT_SYSTEM_OVERLAY,
    SCROLLBAR_WIDTH_PADDING,
    TOOLTIP_TIME_COLUMN,
)
from src.gui.tree_utils import create_styled_context_menu, setup_expand_collapse_icons
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

    columns = ("format", "size", "savings", "time", "efficiency")
    gui.analysis_tree = ttk.Treeview(
        tree_container, columns=columns, show="tree headings", selectmode="extended", style="Analysis.Treeview"
    )

    for col, text in ANALYSIS_TREE_HEADINGS.items():
        gui.analysis_tree.heading(col, text=text, anchor="center", command=lambda c=col: gui.sort_analysis_tree(c))

    # Name column stretches to fill space, data columns are fixed
    gui.analysis_tree.column("#0", width=300, minwidth=150, stretch=True)
    gui.analysis_tree.column("format", width=120, minwidth=100, stretch=False, anchor="w")
    gui.analysis_tree.column("size", width=70, minwidth=70, stretch=False, anchor="e")
    gui.analysis_tree.column("savings", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_tree.column("time", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_tree.column("efficiency", width=80, minwidth=80, stretch=False, anchor="e")

    # Configure tags for status coloring (subtle, professional colors)
    gui.analysis_tree.tag_configure("done", foreground=COLOR_STATUS_SUCCESS)  # Dark green
    gui.analysis_tree.tag_configure("skip", foreground=COLOR_STATUS_WARNING)  # Muted amber
    gui.analysis_tree.tag_configure("av1", foreground=COLOR_STATUS_DISABLED)  # Gray - already AV1, no action needed
    gui.analysis_tree.tag_configure("in_queue", foreground=COLOR_STATUS_INFO)  # Dark blue - queued file/folder
    gui.analysis_tree.tag_configure("partial_queue", foreground=COLOR_STATUS_INFO_LIGHT)  # Light blue - partial queue

    scroll_y = ttk.Scrollbar(tree_container, orient="vertical", command=gui.analysis_tree.yview)
    gui.analysis_tree.configure(yscrollcommand=scroll_y.set)

    gui.analysis_tree.grid(row=0, column=0, sticky="nsew")
    scroll_y.grid(row=0, column=1, sticky="ns")

    # Scanning badge - small floating indicator shown during folder discovery
    # Tree remains visible behind it; interaction blocked via event handlers
    gui.analysis_scan_badge = tk.Label(
        tree_container,
        text="Scanning folder...",
        font=FONT_SYSTEM_OVERLAY,
        bg=COLOR_BADGE_BACKGROUND,
        fg=COLOR_BADGE_TEXT,
        padx=16,
        pady=6,
    )

    # Set up expand/collapse icons (▶/▼ visual updates)
    setup_expand_collapse_icons(gui.analysis_tree)

    # Custom click handler: blocks all interaction during scan, otherwise expands/collapses folders
    def _on_tree_click(event):
        if getattr(gui, "_scanning", False):
            return "break"  # Block all clicks during scan
        item_id = gui.analysis_tree.identify_row(event.y)
        if item_id and gui.analysis_tree.get_children(item_id):  # Has children = folder
            gui.analysis_tree.item(item_id, open=not gui.analysis_tree.item(item_id, "open"))
        return None

    gui.analysis_tree.bind("<Button-1>", _on_tree_click, add=True)

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

    def _get_file_paths_under_tree_item(item_id: str) -> list[str]:
        """Recursively collect all file paths under a tree item.

        Uses _tree_item_map to get paths, avoiding filesystem rescan.
        """
        file_paths: list[str] = []
        stack = [item_id]
        while stack:
            current = stack.pop()
            children = gui.analysis_tree.get_children(current)
            if children:
                # It's a folder - add children to stack
                stack.extend(children)
            else:
                # It's a file - get path from tree_item_map
                path = gui.get_file_path_for_tree_item(current)
                if path:
                    file_paths.append(path)
        return file_paths

    def _add_selected_items_to_queue(selected_items: tuple[str, ...], operation_type: OperationType) -> None:
        """Add multiple selected items from analysis tree to queue with specified operation type."""
        items: list[tuple[str, bool]] = []
        precomputed_folder_files: dict[str, list[str]] = {}

        for item_id in selected_items:
            is_folder = bool(gui.analysis_tree.get_children(item_id))
            if is_folder:
                folder_path = _get_folder_path_from_tree_item(item_id)
                file_paths = _get_file_paths_under_tree_item(item_id)
                items.append((folder_path, True))
                if file_paths:
                    precomputed_folder_files[folder_path] = file_paths
            else:
                file_path = gui.get_file_path_for_tree_item(item_id)
                if file_path:
                    items.append((file_path, False))

        # Use bulk add - shows preview if there are conflicts
        if items:
            gui.add_items_to_queue(
                items, operation_type, force_preview=len(items) > 1, precomputed_folder_files=precomputed_folder_files
            )

    def _show_context_menu(event):
        if getattr(gui, "_scanning", False):
            return  # Block interaction during scan
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
        menu = create_styled_context_menu(gui.analysis_tree)

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

            def _add_folder_to_queue(item: str, op_type: OperationType) -> None:
                """Add folder to queue using precomputed file list from tree structure."""
                fp = _get_folder_path_from_tree_item(item)
                file_paths = _get_file_paths_under_tree_item(item)
                precomputed = {fp: file_paths} if file_paths else None
                gui.add_items_to_queue(
                    [(fp, True)], op_type, force_preview=len(file_paths) > 1, precomputed_folder_files=precomputed
                )

            menu.add_command(label="Open in Explorer", command=lambda path=folder_path: open_in_explorer(path))
            menu.add_separator()
            menu.add_command(
                label="Add Folder to Queue: Convert",
                command=lambda item=item_id: _add_folder_to_queue(item, OperationType.CONVERT),
            )
            menu.add_command(
                label="Add Folder to Queue: Analyze",
                command=lambda item=item_id: _add_folder_to_queue(item, OperationType.ANALYZE),
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
        if getattr(gui, "_scanning", False):
            return  # Block interaction during scan
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

    # Set up column header tooltips
    TreeviewHeaderTooltip(gui.analysis_tree, {
        "savings": (
            "Estimated space saved after conversion.\n"
            "'~' prefix = estimate from similar files.\n"
            "No prefix = precise prediction from CRF analysis."
        ),
        "time": TOOLTIP_TIME_COLUMN,
        "efficiency": "GB saved per hour of conversion time.\nHigher = more space savings for your time.",
    })

    # --- Row 2: Fixed total row (non-scrolling, always visible) ---
    # Use a separate single-row Treeview with matching columns for perfect alignment
    total_container = ttk.Frame(main)
    total_container.grid(row=2, column=0, sticky="ew", pady=(0, 0))
    total_container.columnconfigure(0, weight=1)

    gui.analysis_total_tree = ttk.Treeview(total_container, columns=columns, show="tree", height=1, selectmode="none")

    # Match column widths exactly with main tree
    gui.analysis_total_tree.column("#0", width=300, minwidth=150, stretch=True)
    gui.analysis_total_tree.column("format", width=120, minwidth=100, stretch=False, anchor="w")
    gui.analysis_total_tree.column("size", width=70, minwidth=70, stretch=False, anchor="e")
    gui.analysis_total_tree.column("savings", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_total_tree.column("time", width=90, minwidth=90, stretch=False, anchor="e")
    gui.analysis_total_tree.column("efficiency", width=80, minwidth=80, stretch=False, anchor="e")

    # Insert the total row (will be updated dynamically)
    gui.analysis_total_tree.insert("", "end", iid="total", text="Total", values=("", "—", "—", "—", "—"))

    # Add padding on right to align with scrollbar in main tree
    gui.analysis_total_tree.grid(row=0, column=0, sticky="ew", padx=(0, SCROLLBAR_WIDTH_PADDING))

    # Disable interaction with total row
    gui.analysis_total_tree.bind("<Button-1>", lambda e: "break")
    gui.analysis_total_tree.bind("<Button-3>", lambda e: "break")
    gui.analysis_total_tree.bind("<Double-Button-1>", lambda e: "break")
