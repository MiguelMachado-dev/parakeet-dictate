import tkinter as tk
from tkinter import filedialog, ttk
from pathlib import Path

from .app_rules import AppRule
from .config import ConfigError, parse_config
from .foreground import running_executables
from .settings_widgets import ScrollFrame

PRESETS = (
    ("Translate to English", "Translate the transcript to English."),
    ("Casual", "Make the tone casual and friendly, as in a chat message."),
    ("Formal", "Make the tone formal and polite."),
    ("Blank rule", ""),
)


class RulesEditor(ttk.Frame):
    def __init__(self, parent, changed, error, running_apps=running_executables):
        super().__init__(parent, padding=12)
        self.changed, self.error, self.running_apps = changed, error, running_apps
        self.rules: list[AppRule] = []
        self.selected: int | None = None
        self.loading = False
        self.columnconfigure(1, weight=1)
        self.rowconfigure(0, weight=1)
        left = ttk.Frame(self, padding=(0, 4, 12, 0))
        left.grid(row=0, column=0, sticky="nsew")
        ttk.Label(left, text="Applied from top to bottom.", style="Hint.TLabel").pack(anchor="w", pady=(0, 8))
        self.list = ttk.Treeview(left, show="tree", selectmode="browse", height=10)
        scale = float(self.tk.call("tk", "scaling")) / (96 / 72)
        self.list.column("#0", width=round(200 * scale), stretch=False)
        self.list.pack(fill="both", expand=True)
        self.list.bind("<<TreeviewSelect>>", self._select)
        actions = ttk.Frame(left)
        actions.pack(fill="x", pady=(8, 0))
        add = ttk.Menubutton(actions, text="Add rule")
        menu = tk.Menu(add, tearoff=False)
        for name, instructions in PRESETS:
            menu.add_command(label=name, command=lambda n=name, i=instructions: self.add_rule(n, i))
        add.configure(menu=menu)
        add.pack(side="left", fill="x", expand=True)
        self.remove = ttk.Button(actions, text="Remove", command=self.remove_rule)
        self.remove.pack(side="left", padx=(6, 0))
        moves = ttk.Frame(left)
        moves.pack(fill="x", pady=(6, 0))
        self.up = ttk.Button(moves, text="Move up", command=lambda: self.move_rule(-1))
        self.down = ttk.Button(moves, text="Move down", command=lambda: self.move_rule(1))
        self.up.pack(side="left", fill="x", expand=True)
        self.down.pack(side="left", fill="x", expand=True, padx=(6, 0))

        self.scroll = ScrollFrame(self)
        self.scroll.grid(row=0, column=1, sticky="nsew")
        body = self.scroll.body
        body.columnconfigure(0, weight=1)
        self.name = tk.StringVar(body)
        self.enabled = tk.BooleanVar(body)
        self.executable = tk.StringVar(body)
        ttk.Label(body, text="Rule name").grid(row=0, column=0, sticky="w")
        self.name_entry = ttk.Entry(body, textvariable=self.name)
        self.name_entry.grid(row=1, column=0, sticky="ew", pady=(4, 10))
        self.toggle = ttk.Checkbutton(body, text="Enable this rule", variable=self.enabled)
        self.toggle.grid(row=2, column=0, sticky="w", pady=(0, 16))
        ttk.Label(body, text="Instructions").grid(row=3, column=0, sticky="w")
        self.instructions = tk.Text(body, height=4, width=32, wrap="word", font="TkDefaultFont", relief="solid", borderwidth=1)
        self.instructions.grid(row=4, column=0, sticky="ew", pady=(4, 16))
        self.instructions.bind("<<Modified>>", self._text_changed)
        ttk.Label(body, text="Apps").grid(row=5, column=0, sticky="w")
        self.apps = tk.Listbox(body, height=4, font="TkDefaultFont", exportselection=False, relief="solid", borderwidth=1)
        self.apps.grid(row=6, column=0, sticky="ew", pady=(4, 6))
        self.remove_app_button = ttk.Button(body, text="Remove selected app", command=self.remove_app)
        self.remove_app_button.grid(row=7, column=0, sticky="w", pady=(0, 10))
        ttk.Label(body, text="Add a running app or enter its executable name").grid(row=8, column=0, sticky="w")
        self.app_combo = ttk.Combobox(body, textvariable=self.executable, postcommand=self.refresh_apps)
        self.app_combo.grid(row=9, column=0, sticky="ew", pady=(4, 6))
        app_actions = ttk.Frame(body)
        app_actions.grid(row=10, column=0, sticky="w")
        self.add_app_button = ttk.Button(app_actions, text="Add app", command=self.add_app)
        self.add_app_button.pack(side="left")
        self.browse_button = ttk.Button(app_actions, text="Browse…", command=self.browse)
        self.browse_button.pack(side="left", padx=(6, 0))
        ttk.Label(body, text="Rules use the app in front when recording ends. An active rule applies even with post-processing off.",
                  style="Hint.TLabel", wraplength=360).grid(row=11, column=0, sticky="ew", pady=(18, 0))
        self.name.trace_add("write", self._changed)
        self.enabled.trace_add("write", self._changed)
        self.set_rules(())

    def _changed(self, *unused):
        if not self.loading:
            self._commit()
            self.changed()

    def _text_changed(self, event):
        if self.instructions.edit_modified():
            self.instructions.edit_modified(False)
            self._changed()

    def _commit(self):
        if self.selected is not None and not self.loading:
            rule = AppRule(self.name.get(), self.instructions.get("1.0", "end-1c"), tuple(self.apps.get(0, "end")), self.enabled.get())
            self.rules[self.selected] = rule
            self.list.item(str(self.selected), text=(rule.name or "Untitled") + ("" if rule.enabled else " (off)"))

    def get_rules(self) -> tuple[AppRule, ...]:
        self._commit()
        return tuple(self.rules)

    def set_rules(self, rules, selection=None):
        self.loading = True
        self.rules = list(rules)
        self.selected = None
        self.list.delete(*self.list.get_children())
        for index, rule in enumerate(self.rules):
            self.list.insert("", "end", iid=str(index), text=(rule.name or "Untitled") + ("" if rule.enabled else " (off)"))
        if self.rules:
            self.list.selection_set(str(0 if selection is None else selection))
        self.loading = False
        self._select()

    def _select(self, event=None):
        if self.loading:
            return
        selected = self.list.selection()
        index = int(selected[0]) if selected else None
        if index == self.selected and index is not None:
            return
        self._commit()
        self.selected = index
        self.loading = True
        rule = self.rules[index] if index is not None else AppRule("", "", enabled=False)
        self.name.set(rule.name)
        self.enabled.set(rule.enabled)
        self.instructions.configure(state="normal")
        self.instructions.delete("1.0", "end")
        self.instructions.insert("1.0", rule.instructions)
        self.instructions.edit_modified(False)
        self.apps.configure(state="normal")
        self.apps.delete(0, "end")
        for app in rule.apps:
            self.apps.insert("end", app)
        self.executable.set("")
        state = "normal" if index is not None else "disabled"
        for widget in (self.name_entry, self.toggle, self.instructions, self.apps, self.app_combo,
                       self.add_app_button, self.browse_button, self.remove_app_button, self.remove):
            widget.configure(state=state)
        self.up.configure(state="normal" if index is not None and index > 0 else "disabled")
        self.down.configure(state="normal" if index is not None and index < len(self.rules) - 1 else "disabled")
        self.loading = False

    def add_rule(self, name="New rule", instructions=""):
        self._commit()
        self.set_rules([*self.rules, AppRule(name, instructions)], len(self.rules))
        self.name_entry.focus_set()
        self.changed()

    def remove_rule(self):
        if self.selected is not None:
            rules = self.rules.copy()
            del rules[self.selected]
            self.set_rules(rules, min(self.selected, len(rules) - 1) if rules else None)
            self.changed()

    def move_rule(self, offset):
        self._commit()
        if self.selected is None or not 0 <= self.selected + offset < len(self.rules):
            return
        index, target = self.selected, self.selected + offset
        rules = self.rules.copy()
        rules[index], rules[target] = rules[target], rules[index]
        self.set_rules(rules, target)
        self.changed()

    def refresh_apps(self):
        self.app_combo.configure(values=self.running_apps())

    def add_app(self, name=None):
        if self.selected is None:
            return
        name = (name if name is not None else self.executable.get()).strip()
        try:
            parse_config({"app_rules": [{"name": "App", "instructions": "", "apps": [name]}]})
        except ConfigError:
            self.error("Enter an executable name such as Slack.exe, or choose an app with Browse.")
            return
        if name.casefold() not in {app.casefold() for app in self.apps.get(0, "end")}:
            self.apps.insert("end", name)
            self._changed()
        self.executable.set("")

    def remove_app(self):
        for index in reversed(self.apps.curselection()):
            self.apps.delete(index)
        self._changed()

    def browse(self):
        for path in filedialog.askopenfilenames(parent=self, title="Choose apps", filetypes=[("Windows apps", "*.exe")]):
            self.add_app(Path(path).name)
