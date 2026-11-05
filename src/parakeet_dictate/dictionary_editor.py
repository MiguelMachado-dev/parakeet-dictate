"""Editable original/replacement pairs for the dictionary settings page."""

import tkinter as tk
from dataclasses import dataclass
from tkinter import ttk

from .dictionary import Replacement


@dataclass
class ReplacementRow:
    frame: ttk.Frame
    source: tk.StringVar
    target: tk.StringVar
    remove: ttk.Button


class ReplacementRows(ttk.Frame):
    def __init__(self, parent, changed):
        super().__init__(parent)
        self.changed = changed
        self.rows: list[ReplacementRow] = []
        self.columnconfigure(0, weight=1)
        headings = ttk.Frame(self)
        headings.grid(row=0, column=0, sticky="ew")
        headings.columnconfigure((0, 1), weight=1, uniform="input")
        ttk.Label(headings, text="Original").grid(row=0, column=0, sticky="w")
        ttk.Label(headings, text="Replace with").grid(row=0, column=1, sticky="w")
        self.button_spacer = ttk.Frame(headings, width=38)
        self.button_spacer.grid(row=0, column=2)
        self.set_replacements(())

    def _create_row(self, source="", target=""):
        frame = ttk.Frame(self)
        frame.columnconfigure((0, 1), weight=1, uniform="input")
        original = tk.StringVar(self, value=source)
        replacement = tk.StringVar(self, value=target)
        ttk.Entry(frame, textvariable=original, width=20).grid(row=0, column=0, sticky="ew", padx=(0, 8))
        ttk.Entry(frame, textvariable=replacement, width=20).grid(row=0, column=1, sticky="ew", padx=(0, 8))
        remove = ttk.Button(frame, text="−", width=3)
        row = ReplacementRow(frame, original, replacement, remove)
        remove.configure(command=lambda: self.remove_row(row))
        remove.grid(row=0, column=2, padx=(0, 4))
        ttk.Button(frame, text="+", width=3, command=lambda: self.add_row(after=row)).grid(row=0, column=3)
        for variable in (original, replacement):
            variable.trace_add("write", lambda *args: self._edited())
        return row

    def _edited(self):
        self._layout()
        self.changed()

    def _layout(self):
        has_remove = len(self.rows) > 1 or any(row.source.get() or row.target.get() for row in self.rows)
        self.button_spacer.configure(width=80 if has_remove else 38)
        for index, row in enumerate(self.rows, 1):
            row.frame.grid(row=index, column=0, sticky="ew", pady=(6, 0))
            if has_remove:
                row.remove.grid()
            else:
                row.remove.grid_remove()

    def add_row(self, after=None, source="", target=""):
        row = self._create_row(source, target)
        index = self.rows.index(after) + 1 if after is not None else len(self.rows)
        self.rows.insert(index, row)
        self._layout()
        self.changed()
        return row

    def remove_row(self, row):
        self.rows.remove(row)
        row.frame.destroy()
        if not self.rows:
            self.rows.append(self._create_row())
        self._layout()
        self.changed()

    def set_replacements(self, entries):
        for row in self.rows:
            row.frame.destroy()
        self.rows = [self._create_row(entry.source, entry.target) for entry in entries] or [self._create_row()]
        self._layout()

    def get_replacements(self):
        entries = []
        for index, row in enumerate(self.rows, 1):
            source, target = row.source.get().strip(), row.target.get().strip()
            if not source and not target:
                continue
            if not source or not target:
                raise ValueError(f"Replacement row {index}: enter both original and replacement.")
            entries.append(Replacement(source, target))
        return tuple(entries)
