# src/gui/tabs/convert_tab.py
"""
Queue tab module for the AV1 Video Converter application.

This tab allows users to:
- Add files/folders to a processing queue
- Configure output mode per item (Replace, Suffix, Separate Folder)
- Start/stop batch processing (conversions and analysis)
- Monitor processing progress
"""

import tkinter as tk
from tkinter import ttk

from src.config import DEFAULT_VMAF_TARGET
from src.gui.base import ToolTip, TreeviewHeaderTooltip, open_in_explorer, reveal_in_explorer
from src.gui.constants import (
    COLOR_STATUS_ERROR,
    COLOR_STATUS_INFO,
    COLOR_STATUS_PENDING,
    COLOR_STATUS_SUCCESS,
    COLOR_STATUS_WARNING,
)
from src.gui.tree_utils import create_styled_context_menu, setup_expand_collapse_icons
from src.gui.widgets.operation_dropdown import (
    OPERATION_DISPLAY_TO_ENUM,
    OPERATION_OPTIONS_WITH_LAYER2,
    OPERATION_OPTIONS_WITHOUT_LAYER2,
    OperationDropdownManager,
)
from src.history_index import compute_path_hash, get_history_index
from src.models import OperationType


def create_convert_tab(gui):
    """Create the main queue tab."""
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
    ToolTip(gui.add_folder_button, "Add a folder to the queue")

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

    gui.start_button = ttk.Button(right_buttons, text="Start Queue", command=gui.on_start_conversion)
    gui.start_button.pack(side="left", padx=5)
    ToolTip(gui.start_button, "Begin processing all queued items")

    gui.stop_button = ttk.Button(
        right_buttons, text="Stop After File", command=gui.on_stop_conversion, state="disabled"
    )
    gui.stop_button.pack(side="left", padx=5)
    ToolTip(gui.stop_button, "Stop after current file completes")

    gui.force_stop_button = ttk.Button(
        right_buttons, text="Force Stop", command=gui.on_force_stop_conversion, state="disabled"
    )
    gui.force_stop_button.pack(side="left", padx=(5, 0))
    ToolTip(gui.force_stop_button, "Immediately stop processing")

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

    columns = ("format", "size", "est_time", "operation", "output", "status")
    gui.queue_tree = ttk.Treeview(
        tree_container, columns=columns, show="tree headings", selectmode="extended", style="Analysis.Treeview"
    )

    gui.queue_tree.heading("#0", text="#  Name", anchor="w")
    gui.queue_tree.heading("format", text="Format", anchor="w")
    gui.queue_tree.heading("size", text="Size", anchor="e")
    gui.queue_tree.heading("est_time", text="Est. Time", anchor="e")
    gui.queue_tree.heading("operation", text="Operation", anchor="w")
    gui.queue_tree.heading("output", text="Output", anchor="w")
    gui.queue_tree.heading("status", text="Status", anchor="w")

    gui.queue_tree.column("#0", width=250, minwidth=150, stretch=True)
    gui.queue_tree.column("format", width=120, minwidth=100, stretch=False, anchor="w")
    gui.queue_tree.column("size", width=80, minwidth=60, stretch=False, anchor="e")
    gui.queue_tree.column("est_time", width=80, minwidth=60, stretch=False, anchor="e")
    gui.queue_tree.column("operation", width=110, minwidth=90, stretch=False)
    gui.queue_tree.column("output", width=100, minwidth=80, stretch=False)
    gui.queue_tree.column("status", width=120, minwidth=100, stretch=False)

    scroll_y = ttk.Scrollbar(tree_container, orient="vertical", command=gui.queue_tree.yview)
    gui.queue_tree.configure(yscrollcommand=scroll_y.set)

    gui.queue_tree.grid(row=0, column=0, sticky="nsew")
    scroll_y.grid(row=0, column=1, sticky="ns")

    # File status tags for nested file items
    gui.queue_tree.tag_configure("file_pending", foreground=COLOR_STATUS_PENDING)
    gui.queue_tree.tag_configure("file_converting", foreground=COLOR_STATUS_INFO)
    gui.queue_tree.tag_configure("file_done", foreground=COLOR_STATUS_SUCCESS)
    gui.queue_tree.tag_configure("file_skipped", foreground=COLOR_STATUS_WARNING)
    gui.queue_tree.tag_configure("file_error", foreground=COLOR_STATUS_ERROR)

    # Set up column header tooltips
    TreeviewHeaderTooltip(gui.queue_tree, {
        "operation": "Analyze = find optimal quality settings only.\nConvert = full encoding (analyzes if needed).",
        "output": "Where converted file will be saved.\nReplace / Suffix (_av1) / Separate folder.",
    })

    # Initialize operation dropdown manager for in-cell editing
    operation_dropdown = OperationDropdownManager(gui)

    # Click handler: operation column dropdown OR folder expand/collapse
    def _on_tree_click(event):
        # Check for operation column click first
        if operation_dropdown.is_operation_column_click(event) and operation_dropdown.show_dropdown(event):
            return "break"  # Prevent other handlers

        # Folder expand/collapse on tree column
        item_id = gui.queue_tree.identify_row(event.y)
        col_id = gui.queue_tree.identify_column(event.x)
        if item_id and col_id == "#0" and gui.queue_tree.get_children(item_id):
            # Don't call focus() - it resets the anchor used for shift-click selection
            gui.queue_tree.item(item_id, open=not gui.queue_tree.item(item_id, "open"))
        return None

    gui.queue_tree.bind("<Button-1>", _on_tree_click, add="+")

    # Drag-and-drop reordering
    def _on_drag_motion(event):
        """Move selected items during drag."""
        if gui.session.running:
            return  # Don't allow reordering during conversion
        tv = event.widget
        target = tv.identify_row(event.y)
        if target:
            target_index = tv.index(target)
            for item in tv.selection():
                tv.move(item, "", target_index)
            # Sync the underlying data model
            gui.sync_queue_order_from_tree()

    def _on_drag_end(event):
        """Finalize selection on release."""
        tv = event.widget
        item = tv.identify_row(event.y)
        if item and item in tv.selection():
            tv.selection_set(item)

    gui.queue_tree.bind("<B1-Motion>", _on_drag_motion, add="+")
    gui.queue_tree.bind("<ButtonRelease-1>", _on_drag_end, add="+")

    # Set up expand/collapse icon updates using shared utility
    setup_expand_collapse_icons(gui.queue_tree)

    # Selection change updates properties panel
    gui.queue_tree.bind("<<TreeviewSelect>>", lambda e: gui.on_queue_selection_changed())

    def _on_operation_change_from_menu(queue_item, selected_display: str) -> None:
        """Handle operation change from context menu submenu."""
        new_operation = OPERATION_DISPLAY_TO_ENUM.get(selected_display)
        if new_operation is None:
            return

        # Handle "Re-analyze + Convert" - clear cached Layer 2 data
        if selected_display == "Re-analyze + Convert":
            path_hash = compute_path_hash(queue_item.source_path)
            index = get_history_index()
            record = index.get(path_hash)
            if record:
                record.best_crf = None
                record.best_vmaf_achieved = None
                record.predicted_output_size = None
                record.predicted_size_reduction = None
                index.save()

        # Update queue item if operation changed
        if queue_item.operation_type != new_operation:
            queue_item.operation_type = new_operation
            gui.save_queue_to_config()
            gui.refresh_queue_tree()

    def _show_context_menu(event):
        item_id = gui.queue_tree.identify_row(event.y)
        if not item_id:
            return

        # Check if this is a nested file row (not a queue item)
        file_path = gui.get_file_path_for_queue_tree_item(item_id)
        if file_path:
            menu = create_styled_context_menu(gui.queue_tree)
            menu.add_command(label="Open File", command=lambda p=file_path: open_in_explorer(p))
            menu.add_command(label="Show in Explorer", command=lambda p=file_path: reveal_in_explorer(p))
            menu.update_idletasks()
            menu.tk_popup(event.x_root, event.y_root)
            return

        # Preserve multi-selection if right-clicking within existing selection
        current_selection = gui.queue_tree.selection()
        if item_id in current_selection and len(current_selection) > 1:
            selected_items = current_selection
        else:
            gui.queue_tree.selection_set(item_id)
            selected_items = (item_id,)

        menu = create_styled_context_menu(gui.queue_tree)

        # Multi-selection: show batch actions only
        if len(selected_items) > 1:
            menu.add_command(label=f"Remove {len(selected_items)} Items", command=gui.on_remove_from_queue)
            menu.update_idletasks()
            menu.tk_popup(event.x_root, event.y_root)
            return

        # Single selection: show item-specific actions
        queue_item = gui.get_queue_item_for_tree_item(item_id)
        source_path = queue_item.source_path if queue_item else None
        is_folder = queue_item.is_folder if queue_item else False

        if source_path:
            if is_folder:
                menu.add_command(label="Open in Explorer", command=lambda path=source_path: open_in_explorer(path))
            else:
                menu.add_command(label="Open File", command=lambda path=source_path: open_in_explorer(path))
                menu.add_command(label="Show in Explorer", command=lambda path=source_path: reveal_in_explorer(path))
            menu.add_separator()

        # Add operation options (not during processing)
        if queue_item and not gui.session.running:
            # Determine available options based on Layer 2 data
            path_hash = compute_path_hash(queue_item.source_path)
            record = get_history_index().get(path_hash)
            has_layer2 = bool(record and record.best_crf is not None and record.best_vmaf_achieved is not None)

            # Determine current operation for indicator
            if queue_item.operation_type == OperationType.ANALYZE:
                current_display = "Analyze Only"
            else:
                current_display = "Convert" if has_layer2 else "Analyze + Convert"

            # Add operation options directly (no submenu)
            options = OPERATION_OPTIONS_WITH_LAYER2 if has_layer2 else OPERATION_OPTIONS_WITHOUT_LAYER2
            for option in options:
                prefix = "● " if option == current_display else "   "
                menu.add_command(
                    label=f"{prefix}{option}",
                    command=lambda opt=option, qi=queue_item: _on_operation_change_from_menu(qi, opt),
                )
            menu.add_separator()

        menu.add_command(label="Remove", command=gui.on_remove_from_queue)

        # Force geometry recalculation before showing (fixes Windows shadow rendering)
        menu.update_idletasks()
        menu.tk_popup(event.x_root, event.y_root)

    gui.queue_tree.bind("<Button-3>", _show_context_menu)

    # Delete key removes selected items
    gui.queue_tree.bind("<Delete>", lambda e: gui.on_remove_from_queue())

    # Double-click opens file/folder in explorer
    def _on_double_click(event):
        item_id = gui.queue_tree.identify_row(event.y)
        if not item_id:
            return
        # Check for nested file row first
        file_path = gui.get_file_path_for_queue_tree_item(item_id)
        if file_path:
            open_in_explorer(file_path)
            return
        # Otherwise check for queue item
        source_path = gui.get_queue_source_path_for_tree_item(item_id)
        if source_path:
            open_in_explorer(source_path)

    gui.queue_tree.bind("<Double-Button-1>", _on_double_click)

    # --- Row 3: Total row (non-scrolling) ---
    total_container = ttk.Frame(main_frame)
    total_container.grid(row=3, column=0, sticky="ew", pady=(0, 5))
    total_container.columnconfigure(0, weight=1)

    gui.queue_total_tree = ttk.Treeview(total_container, columns=columns, show="tree", height=1, selectmode="none")
    gui.queue_total_tree.column("#0", width=250, minwidth=150, stretch=True)
    gui.queue_total_tree.column("format", width=120, minwidth=100, stretch=False, anchor="w")
    gui.queue_total_tree.column("size", width=80, minwidth=60, stretch=False, anchor="e")
    gui.queue_total_tree.column("est_time", width=80, minwidth=60, stretch=False, anchor="e")
    gui.queue_total_tree.column("operation", width=110, minwidth=90, stretch=False)
    gui.queue_total_tree.column("output", width=100, minwidth=80, stretch=False)
    gui.queue_total_tree.column("status", width=120, minwidth=100, stretch=False)

    gui.queue_total_tree.insert("", "end", iid="total", text="Total", values=("", "", "", "", "", "0 items"))
    gui.queue_total_tree.grid(row=0, column=0, sticky="ew", padx=(0, 17))

    gui.queue_total_tree.bind("<Button-1>", lambda e: "break")
    gui.queue_total_tree.bind("<Button-3>", lambda e: "break")
    gui.queue_total_tree.bind("<Double-Button-1>", lambda e: "break")

    # --- Row 4: Properties panel for selected item ---
    gui.queue_properties_frame = ttk.LabelFrame(main_frame, text="Selected Item")
    gui.queue_properties_frame.grid(row=4, column=0, sticky="ew", pady=5)
    gui.queue_properties_frame.columnconfigure(1, weight=1)

    # Output Mode dropdown
    ttk.Label(gui.queue_properties_frame, text="Output Mode:").grid(row=0, column=0, sticky="w", padx=10, pady=3)
    gui.item_output_mode = tk.StringVar(value="replace")
    gui.item_mode_combo = ttk.Combobox(
        gui.queue_properties_frame, textvariable=gui.item_output_mode, width=18, state="readonly"
    )
    gui.item_mode_combo["values"] = ("replace", "suffix", "separate_folder")
    gui.item_mode_combo.grid(row=0, column=1, sticky="w", padx=5, pady=3)
    gui.item_mode_combo.bind("<<ComboboxSelected>>", lambda e: gui.on_item_output_mode_changed())

    # Suffix entry
    ttk.Label(gui.queue_properties_frame, text="Suffix:").grid(row=0, column=2, sticky="w", padx=(20, 5), pady=3)
    gui.item_suffix = tk.StringVar(value="_av1")
    gui.item_suffix_entry = ttk.Entry(gui.queue_properties_frame, textvariable=gui.item_suffix, width=10)
    gui.item_suffix_entry.grid(row=0, column=3, sticky="w", padx=5, pady=3)
    gui.item_suffix_entry.bind("<FocusOut>", lambda e: gui.on_item_suffix_changed())

    # Output folder
    ttk.Label(gui.queue_properties_frame, text="Output Folder:").grid(row=1, column=0, sticky="w", padx=10, pady=3)
    gui.item_output_folder = tk.StringVar()
    gui.item_folder_entry = ttk.Entry(gui.queue_properties_frame, textvariable=gui.item_output_folder)
    gui.item_folder_entry.grid(row=1, column=1, columnspan=2, sticky="ew", padx=5, pady=3)
    gui.item_folder_browse_button = ttk.Button(
        gui.queue_properties_frame, text="Browse...", command=gui.on_browse_item_output_folder
    )
    gui.item_folder_browse_button.grid(row=1, column=3, padx=(5, 10), pady=3)

    # Source path (read-only)
    ttk.Label(gui.queue_properties_frame, text="Source:").grid(row=2, column=0, sticky="w", padx=10, pady=3)
    gui.item_source_label = ttk.Label(gui.queue_properties_frame, text="Select an item to configure", anchor="w")
    gui.item_source_label.grid(row=2, column=1, columnspan=3, sticky="ew", padx=5, pady=3)

    # Start with controls disabled (no selection)
    gui.item_mode_combo.config(state="disabled")
    gui.item_suffix_entry.config(state="disabled")
    gui.item_folder_entry.config(state="disabled")
    gui.item_folder_browse_button.config(state="disabled")

    # --- Row 5: Current file progress (same as old main_tab) ---
    file_frame = ttk.LabelFrame(main_frame, text="Current File")
    file_frame.grid(row=5, column=0, sticky="ew", pady=5)
    file_frame.columnconfigure(1, weight=1)

    gui.current_file_label = ttk.Label(file_frame, text="No file processing", wraplength=650, justify=tk.LEFT)
    gui.current_file_label.grid(row=0, column=0, columnspan=3, sticky="w", padx=5, pady=(3, 2))

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
    details_grid.pack(fill="x", padx=5, pady=5)

    # Left column
    left_col = ttk.Frame(details_grid)
    left_col.pack(side="left", fill="x", expand=True, padx=(0, 10))

    ttk.Label(left_col, text="Original Format:").grid(row=0, column=0, sticky="w", pady=2)
    gui.orig_format_label = ttk.Label(left_col, text="-")
    gui.orig_format_label.grid(row=0, column=1, sticky="w", pady=2)

    ttk.Label(left_col, text="Original Size:").grid(row=1, column=0, sticky="w", pady=2)
    gui.orig_size_label = ttk.Label(left_col, text="-")
    gui.orig_size_label.grid(row=1, column=1, sticky="w", pady=2)

    ttk.Label(left_col, text="VMAF Target:").grid(row=2, column=0, sticky="w", pady=2)
    gui.vmaf_label = ttk.Label(left_col, text=f"{DEFAULT_VMAF_TARGET}")
    gui.vmaf_label.grid(row=2, column=1, sticky="w", pady=2)

    ttk.Label(left_col, text="Encoding Settings:").grid(row=3, column=0, sticky="w", pady=2)
    gui.encoding_settings_label = ttk.Label(left_col, text="-")
    gui.encoding_settings_label.grid(row=3, column=1, sticky="w", pady=2)

    # Right column
    right_col = ttk.Frame(details_grid)
    right_col.pack(side="right", fill="x", expand=True, padx=(10, 0))

    ttk.Label(right_col, text="Elapsed Time:").grid(row=0, column=0, sticky="w", pady=2)
    gui.elapsed_label = ttk.Label(right_col, text="-")
    gui.elapsed_label.grid(row=0, column=1, sticky="w", pady=2)

    ttk.Label(right_col, text="Est. Remaining (Enc):").grid(row=1, column=0, sticky="w", pady=2)
    gui.eta_label = ttk.Label(right_col, text="-")
    gui.eta_label.grid(row=1, column=1, sticky="w", pady=2)

    ttk.Label(right_col, text="Est. Final Size:").grid(row=2, column=0, sticky="w", pady=2)
    gui.output_size_label = ttk.Label(right_col, text="-")
    gui.output_size_label.grid(row=2, column=1, sticky="w", pady=2)
