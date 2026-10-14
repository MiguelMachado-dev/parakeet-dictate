import tkinter as tk
from tkinter import ttk


class ScrollFrame(ttk.Frame):
    def __init__(self, parent):
        super().__init__(parent)
        self.canvas = tk.Canvas(self, highlightthickness=0, background=ttk.Style().lookup("TFrame", "background"))
        scroll = ttk.Scrollbar(self, orient="vertical", command=self.canvas.yview)
        self.canvas.configure(yscrollcommand=scroll.set)
        self.canvas.pack(side="left", fill="both", expand=True)
        scroll.pack(side="right", fill="y")
        self.body = ttk.Frame(self.canvas, padding=20)
        window = self.canvas.create_window(0, 0, window=self.body, anchor="nw")
        self.body.bind("<Configure>", lambda event: self.canvas.configure(scrollregion=self.canvas.bbox("all")))
        self.canvas.bind("<Configure>", lambda event: self.canvas.itemconfigure(window, width=event.width))

    def contains(self, widget) -> bool:
        while widget is not None:
            if widget is self:
                return True
            widget = getattr(widget, "master", None)
        return False

    def reveal(self, widget) -> None:
        self.update_idletasks()
        top = widget.winfo_rooty() - self.canvas.winfo_rooty()
        bottom = top + widget.winfo_height()
        height = max(1, self.body.winfo_height())
        start = self.canvas.canvasy(0)
        if top < 32:
            self.canvas.yview_moveto(max(0, start + top - 32) / height)
        elif bottom > self.canvas.winfo_height():
            self.canvas.yview_moveto((start + bottom - self.canvas.winfo_height() + 12) / height)
