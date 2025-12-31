"""Tree view utility functions shared across tabs."""

import tkinter as tk
from tkinter import ttk


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
            tree.focus(item_id)
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
        background="#ffffff",
        activebackground="#0078d4",
        activeforeground="#ffffff",
    )
