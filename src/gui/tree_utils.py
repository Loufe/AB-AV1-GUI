"""Tree view utility functions shared across tabs."""

import tkinter as tk
from tkinter import ttk

from src.gui.constants import COLOR_MENU_ACTIVE_BG, COLOR_MENU_ACTIVE_FG, COLOR_MENU_BACKGROUND


def get_column_name(tree: ttk.Treeview, column_id: str) -> str | None:
    """Convert a Treeview column ID to its column name.

    Treeview.identify_column() returns positional IDs like "#0", "#1", etc.
    This function converts them to the actual column names defined when
    creating the Treeview (e.g., "operation", "status").

    Args:
        tree: The Treeview widget
        column_id: Column ID from identify_column(), e.g., "#0", "#1"

    Returns:
        The column name for data columns (e.g., "operation"),
        "#0" for the tree column, or None if the ID is invalid.

    Example:
        col_id = tree.identify_column(event.x)  # Returns "#4"
        col_name = get_column_name(tree, col_id)  # Returns "operation"
        if col_name == "operation":
            ...
    """
    if column_id == "#0":
        return "#0"
    try:
        col_index = int(column_id[1:]) - 1
        columns = tree["columns"]
        if 0 <= col_index < len(columns):
            return columns[col_index]
        return None
    except (ValueError, IndexError):
        return None


def setup_expand_collapse_icons(tree: ttk.Treeview) -> None:
    """Bind tree events to update folder expand/collapse icons (▶/▼).

    Args:
        tree: The Treeview widget to configure.
    """

    def _on_tree_open(event):
        item_id = tree.focus()
        if item_id:
            text = tree.item(item_id, "text")
            if text.startswith("▶"):
                tree.item(item_id, text=text.replace("▶", "▼", 1))

    def _on_tree_close(event):
        item_id = tree.focus()
        if item_id:
            text = tree.item(item_id, "text")
            if text.startswith("▼"):
                tree.item(item_id, text=text.replace("▼", "▶", 1))

    tree.bind("<<TreeviewOpen>>", _on_tree_open)
    tree.bind("<<TreeviewClose>>", _on_tree_close)


def setup_click_expand_collapse(tree: ttk.Treeview) -> None:
    """Bind click anywhere on folder row to expand/collapse.

    Args:
        tree: The Treeview widget to configure.
    """

    def _on_tree_click(event):
        item_id = tree.identify_row(event.y)
        if item_id and tree.get_children(item_id):  # Has children = folder
            # Don't call tree.focus() - it resets the anchor used for shift-click selection
            tree.item(item_id, open=not tree.item(item_id, "open"))

    tree.bind("<Button-1>", _on_tree_click, add="+")


def create_styled_context_menu(parent: tk.Widget) -> tk.Menu:
    """Create a context menu with standard application styling.

    Args:
        parent: The parent widget for the menu.

    Returns:
        A styled tk.Menu instance.
    """
    return tk.Menu(
        parent,
        tearoff=0,
        background=COLOR_MENU_BACKGROUND,
        activebackground=COLOR_MENU_ACTIVE_BG,
        activeforeground=COLOR_MENU_ACTIVE_FG,
    )
