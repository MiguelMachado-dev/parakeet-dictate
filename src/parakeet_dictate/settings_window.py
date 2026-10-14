import ctypes
import queue
import threading
import time
import tkinter as tk
from dataclasses import asdict
from tkinter import filedialog, font, messagebox, ttk

from .config import ROOT, Config, ConfigError, PostprocessConfig, parse_config
from .polish import PolishError, create_polisher
from .rules_editor import RulesEditor
from .settings_store import KEY_NAMES, SettingsStore, config_values
from .settings_widgets import ScrollFrame

PROVIDERS = {"Z.ai (GLM)": "zai", "Anthropic (Claude)": "anthropic"}
ENGINES = {"NVIDIA GPU (CUDA)": "cuda", "CPU": "cpu"}
EFFORTS = {"Low": "low", "Medium": "medium", "High": "high", "Extra high": "xhigh", "Max": "max"}
SETTINGS_MUTEX = "Local\\parakeet-dictate-settings"
SHOW_EVENT = "Local\\parakeet-dictate-show-settings"


def apply_settings() -> str:
    from .app import RELOAD_EVENT_NAME, launch_background, signal_event
    if signal_event(RELOAD_EVENT_NAME):
        return "Saved. The app will reload after the current dictation finishes."
    launch_background("--notify-ready")
    return "Saved. Starting Parakeet Dictate with these settings."


class SettingsWindow:
    def __init__(self, root: tk.Tk, store: SettingsStore | None = None, on_apply=apply_settings):
        self.root, self.store, self.on_apply = root, store or SettingsStore(), on_apply
        self.closed, self.loading, self.dirty = False, True, False
        self.after_ids = set()
        self.snapshot = self.store.load()
        self.vars = {}
        self.keys = {name: tk.StringVar(root) for name in KEY_NAMES}
        self.pages = []
        self._provider = "zai"
        self.provider_values = {}
        root.title("Parakeet Dictate — Settings")
        try:
            root.iconbitmap(str(ROOT / "assets" / "parakeet.ico"))
        except tk.TclError:
            pass
        for name in ("TkDefaultFont", "TkTextFont", "TkMenuFont"):
            font.nametofont(name, root=root).configure(family="Segoe UI", size=10)
        style = ttk.Style(root)
        style.theme_use("vista")
        style.configure("Title.TLabel", font=("Segoe UI", 18, "bold"))
        style.configure("Heading.TLabel", font=("Segoe UI", 11, "bold"))
        style.configure("Hint.TLabel", foreground="#505050", font=("Segoe UI", 9))
        style.configure("TNotebook.Tab", padding=(14, 7))
        scale = float(root.tk.call("tk", "scaling")) / (96 / 72)
        style.configure("Treeview", rowheight=font.nametofont("TkDefaultFont", root=root).metrics("linespace") + round(10 * scale))
        width = min(round(820 * scale), root.winfo_screenwidth() - 80)
        height = min(round(720 * scale), root.winfo_screenheight() - 100)
        root.geometry(f"{width}x{height}+{max(0, (root.winfo_screenwidth() - width) // 2)}+{max(0, (root.winfo_screenheight() - height) // 2)}")
        root.minsize(min(width, round(700 * scale)), min(height, round(500 * scale)))
        outer = ttk.Frame(root, padding=18)
        outer.pack(fill="both", expand=True)
        ttk.Label(outer, text="Settings", style="Title.TLabel").pack(anchor="w")
        ttk.Label(outer, text="Dictation, post-processing, and the apps you use.", style="Hint.TLabel").pack(anchor="w", pady=(2, 16))
        self.notebook = ttk.Notebook(outer)
        self.notebook.pack(fill="both", expand=True)
        self._general()
        self._postprocess()
        self.rules = RulesEditor(self.notebook, self.changed, self.error)
        self.notebook.add(self.rules, text="App rules")
        self.pages.append(self.rules.scroll)
        self._server()
        footer = ttk.Frame(outer, padding=(0, 12, 0, 0))
        footer.pack(fill="x")
        self.status = tk.StringVar(root)
        self.status_label = ttk.Label(footer, textvariable=self.status, wraplength=700)
        self.status_label.pack(anchor="w", fill="x", pady=(0, 10))
        buttons = ttk.Frame(footer)
        buttons.pack(fill="x")
        ttk.Button(buttons, text="Reload", command=self.reload).pack(side="left")
        self.save_button = ttk.Button(buttons, text="Save and apply", command=self.save)
        self.save_button.pack(side="right")
        ttk.Button(buttons, text="Close", command=self.close).pack(side="right", padx=(0, 8))
        root.protocol("WM_DELETE_WINDOW", self.close)
        root.bind("<Control-s>", lambda event: self.save())
        root.bind("<Escape>", lambda event: self.close())
        root.bind("<MouseWheel>", self._wheel, add=True)
        root.bind("<FocusIn>", self._focus, add=True)
        outer.bind("<Configure>", lambda event: self.status_label.configure(wraplength=max(100, event.width - 30)))
        self._load()

    def _var(self, name, boolean=False):
        variable = tk.BooleanVar(self.root) if boolean else tk.StringVar(self.root)
        variable.trace_add("write", lambda *args: self.changed())
        self.vars[name] = variable
        return variable

    def _page(self, title):
        page = ScrollFrame(self.notebook)
        self.notebook.add(page, text=title)
        self.pages.append(page)
        page.body.columnconfigure(1, weight=1)
        return page.body

    def _row(self, parent, row, label, name, choices=None):
        ttk.Label(parent, text=label).grid(row=row, column=0, sticky="nw", padx=(0, 24), pady=(6, 12))
        var = self._var(name)
        widget = ttk.Entry(parent, textvariable=var) if choices is None else ttk.Combobox(parent, textvariable=var, values=choices, state="readonly")
        widget.grid(row=row, column=1, sticky="ew", pady=(2, 10))
        return widget

    def _check(self, parent, row, text, name):
        widget = ttk.Checkbutton(parent, text=text, variable=self._var(name, boolean=True))
        widget.grid(row=row, column=0, columnspan=2, sticky="w", pady=(4, 10))
        return widget

    def _hint(self, parent, row, text):
        ttk.Label(parent, text=text, style="Hint.TLabel", wraplength=580).grid(row=row, column=0, columnspan=2, sticky="ew", pady=(0, 16))

    def _general(self):
        page = self._page("General")
        candidates = ["ctrl_r", "ctrl_l", "shift_r", "shift_l", "alt_r", "alt_l", "cmd_r", "cmd_l",
                      "pause", "scroll_lock", "insert", *[f"f{i}" for i in range(1, 25)], self.snapshot.config.hotkey]
        self.hotkeys = {Config(hotkey=key).hotkey_label: key for key in candidates}
        self._row(page, 0, "Dictation hotkey", "hotkey", tuple(self.hotkeys))
        self._hint(page, 1, "Hold to talk, or tap for hands-free recording. Esc cancels. On some keyboard layouts, Right Alt is AltGr.")
        self._row(page, 2, "Speech recognition", "provider", tuple(ENGINES))
        self._row(page, 3, "Overlay", "overlay", ("Fluid", "Bars", "Off"))
        self._check(page, 4, "Play start, stop, cancel, and error sounds", "sounds")
        self._check(page, 5, "Add a space after each dictation", "append_space")
        ttk.Separator(page).grid(row=6, column=0, columnspan=2, sticky="ew", pady=(8, 16))
        ttk.Label(page, text="Advanced", style="Heading.TLabel").grid(row=7, column=0, columnspan=2, sticky="w", pady=(0, 10))
        self._row(page, 8, "Tap threshold (ms)", "tap_threshold_ms")
        self._row(page, 9, "Model folder", "model_dir")
        ttk.Button(page, text="Choose folder…", command=self._choose_model).grid(row=10, column=1, sticky="w")

    def _postprocess(self):
        page = self._page("Post-processing")
        self.postprocess_page = self.pages[-1]
        self._check(page, 0, "Post-process dictations", "postprocess.enabled")
        self._hint(page, 1, "Clean up fillers, repetitions, and punctuation before pasting. Transcript text goes to the chosen provider; audio stays on this PC.")
        provider = self._row(page, 2, "Provider", "postprocess.provider", tuple(PROVIDERS))
        provider.bind("<<ComboboxSelected>>", lambda event: self._change_provider())
        self.key_label = ttk.Label(page, text="Z.ai API key")
        self.key_label.grid(row=3, column=0, sticky="w", padx=(0, 24))
        key_row = ttk.Frame(page)
        key_row.grid(row=3, column=1, sticky="ew", pady=(2, 6))
        key_row.columnconfigure(0, weight=1)
        self.key_entry = ttk.Entry(key_row, show="•")
        self.key_entry.grid(row=0, column=0, sticky="ew")
        self.show_key = tk.BooleanVar(self.root)
        ttk.Checkbutton(key_row, text="Show", variable=self.show_key,
                        command=lambda: self.key_entry.configure(show="" if self.show_key.get() else "•")).grid(row=0, column=1, padx=(8, 0))
        self.key_hint = ttk.Label(page, style="Hint.TLabel", wraplength=420)
        self.key_hint.grid(row=4, column=1, sticky="w", pady=(0, 12))
        for var in self.keys.values():
            var.trace_add("write", lambda *args: self.changed())
        self.model_box = self._row(page, 5, "Model", "postprocess.model", ())
        self.model_box.configure(state="normal")
        self.thinking = self._check(page, 6, "Thinking before replying (Claude)", "postprocess.thinking")
        self.thinking.configure(command=self._thinking_changed)
        self.effort_box = self._row(page, 7, "Effort", "postprocess.effort", ())
        self._row(page, 8, "Timeout (seconds)", "postprocess.timeout_s")
        ttk.Label(page, text="Extra instructions").grid(row=9, column=0, sticky="nw", padx=(0, 24), pady=(5, 0))
        self.extra = tk.Text(page, height=3, width=30, font="TkDefaultFont", wrap="word", relief="solid", borderwidth=1)
        self.extra.grid(row=9, column=1, sticky="ew", pady=(2, 6))
        self.extra.bind("<<Modified>>", self._extra_changed)
        ttk.Label(page, text="Names and terms to spell your way.", style="Hint.TLabel").grid(row=10, column=1, sticky="w", pady=(0, 14))
        self._row(page, 11, "API endpoint", "postprocess.endpoint")
        self.test_status = tk.StringVar(self.root)
        self.test_button = ttk.Button(page, text="Test provider", command=self.test_provider)
        self.test_button.grid(row=12, column=1, sticky="w", pady=(4, 8))
        self.test_label = ttk.Label(page, textvariable=self.test_status, style="Hint.TLabel", wraplength=420)
        self.test_label.grid(row=13, column=1, sticky="w")

    def _server(self):
        page = self._page("Server for Mac")
        self._check(page, 0, "Allow a Mac to dictate through this PC", "server.enabled")
        self._hint(page, 1, "The PC transcribes the audio and uses its own post-processing provider. On the Mac, paste the URL and token from the tray's Copy Mac connection info.")
        self._row(page, 2, "Port", "server.port")
        self._row(page, 3, "Listen address", "server.bind")
        self._hint(page, 4, "0.0.0.0 listens on every network adapter. Keep the connection token private. If you change the port, update the Windows Firewall rule for Parakeet Dictate too.")

    def _load(self):
        self.loading = True
        config = self.snapshot.config
        values = config_values(config)
        for name in self.vars:
            section, dot, key = name.partition(".")
            value = values[section][key] if dot else values[section]
            if name == "hotkey": value = config.hotkey_label
            if name == "provider": value = next(k for k, v in ENGINES.items() if v == value)
            if name == "overlay": value = value.capitalize()
            if name == "postprocess.provider": value = next(k for k, v in PROVIDERS.items() if v == value)
            if name == "postprocess.effort": value = self._effort_label(config.postprocess)
            self.vars[name].set(value)
        for name, var in self.keys.items(): var.set(self.snapshot.keys[name])
        self.extra.delete("1.0", "end")
        self.extra.insert("1.0", config.postprocess.extra_instructions)
        self.extra.edit_modified(False)
        self.provider_values = {provider: PostprocessConfig(provider=provider) for provider in PROVIDERS.values()}
        self.provider_values[config.postprocess.provider] = config.postprocess
        self._provider = config.postprocess.provider
        self._show_provider()
        self.rules.set_rules(config.app_rules)
        self.loading = False
        self.dirty = False
        self.save_button.configure(state="disabled")
        self.status.set("Changes take effect after Save and apply.")
        self.status_label.configure(foreground="#505050")

    @staticmethod
    def _effort_label(config):
        effort = config.effort if config.provider == "anthropic" else config.reasoning_effort
        return next(label for label, value in EFFORTS.items() if value == effort)

    def _change_provider(self):
        old = self.provider_values[self._provider]
        fields = asdict(old)
        fields.update(model=self.vars["postprocess.model"].get(), endpoint=self.vars["postprocess.endpoint"].get(),
                      thinking=self.vars["postprocess.thinking"].get())
        fields["effort" if self._provider == "anthropic" else "reasoning_effort"] = EFFORTS[self.vars["postprocess.effort"].get()]
        self.provider_values[self._provider] = PostprocessConfig(**fields)
        self._provider = PROVIDERS[self.vars["postprocess.provider"].get()]
        new = self.provider_values[self._provider]
        for name in ("model", "endpoint", "thinking"):
            self.vars[f"postprocess.{name}"].set(getattr(new, name))
        self.vars["postprocess.effort"].set(self._effort_label(new))
        self._show_provider()
        self.changed()

    def _show_provider(self):
        config = PostprocessConfig(provider=self._provider)
        self.key_label.configure(text="Anthropic API key" if self._provider == "anthropic" else "Z.ai API key")
        self.key_entry.configure(textvariable=self.keys[config.api_key_name])
        managed = config.api_key_name in self.snapshot.environment_keys
        self.key_entry.configure(state="readonly" if managed else "normal")
        self.key_hint.configure(text=f"Set by the {config.api_key_name} environment variable." if managed else "Saved on this PC. Each provider keeps its own key.")
        self.show_key.set(False)
        self.key_entry.configure(show="•")
        self.model_box.configure(values=("claude-haiku-5-5", "claude-sonnet-5-5") if self._provider == "anthropic" else ("glm-5.3",))
        self.thinking.configure(state="normal" if self._provider == "anthropic" else "disabled")
        if self._provider == "anthropic":
            self.thinking.grid()
        else:
            self.thinking.grid_remove()
        self._thinking_changed()
        self.test_status.set("")

    def _thinking_changed(self):
        allowed = ("Low", "High", "Max") if self._provider == "zai" else tuple(EFFORTS) if self.vars["postprocess.thinking"].get() else ("Low", "Medium", "High")
        self.effort_box.configure(values=allowed)
        if self.vars["postprocess.effort"].get() not in allowed:
            self.vars["postprocess.effort"].set("High")

    def _extra_changed(self, event):
        if self.extra.edit_modified():
            self.extra.edit_modified(False)
            self.changed()

    def changed(self):
        if not self.loading:
            self.dirty = True
            self.save_button.configure(state="normal")
            self.status.set("You have unsaved changes.")
            self.status_label.configure(foreground="#505050")

    def error(self, text):
        self.status.set(text)
        self.status_label.configure(foreground="#a12020")

    def draft(self) -> Config:
        values = config_values(self.snapshot.config)
        for name, variable in self.vars.items():
            value = variable.get()
            if name == "hotkey": value = self.hotkeys[value]
            elif name == "provider": value = ENGINES[value]
            elif name == "overlay": value = value.lower()
            elif name == "postprocess.provider": value = PROVIDERS[value]
            elif name == "postprocess.effort": value = EFFORTS[value]
            elif name in ("tap_threshold_ms", "server.port", "postprocess.timeout_s"):
                try:
                    value = float(value) if name.endswith("timeout_s") else int(value)
                except ValueError:
                    raise ConfigError(f"{name} must be a number.") from None
            section, dot, key = name.partition(".")
            if name == "postprocess.effort" and self._provider == "zai": key = "reasoning_effort"
            if dot: values[section][key] = value
            else: values[section] = value
        values["postprocess"]["extra_instructions"] = self.extra.get("1.0", "end-1c")
        values["app_rules"] = [{**asdict(rule), "apps": list(rule.apps)} for rule in self.rules.get_rules()]
        return parse_config(values)

    def save(self):
        try:
            config = self.draft()
            if config.model_dir != self.snapshot.config.model_dir and not (config.model_dir / "config.json").is_file():
                raise ConfigError("Choose a Parakeet model folder containing config.json.")
            self.snapshot = self.store.save(self.snapshot, config, {name: var.get() for name, var in self.keys.items()})
        except (ConfigError, OSError) as e:
            self.error(str(e))
            return False
        self.dirty = False
        self.save_button.configure(state="disabled")
        try:
            self.status.set(self.on_apply())
            self.status_label.configure(foreground="#276334")
        except OSError:
            self.error("Saved, but the app could not reload. Start Parakeet Dictate from the Desktop.")
        return True

    def reload(self):
        if self.dirty and not messagebox.askyesno("Reload settings", "Discard your unsaved changes and reload?", parent=self.root):
            return
        try:
            self.snapshot = self.store.load()
            self._load()
        except (ConfigError, OSError) as e:
            self.error(str(e))

    def _choose_model(self):
        path = filedialog.askdirectory(parent=self.root, title="Choose the Parakeet model folder")
        if path: self.vars["model_dir"].set(path)

    def test_provider(self):
        try:
            config = self.draft().postprocess
        except ConfigError as e:
            self.error(str(e))
            return
        key = self.keys[config.api_key_name].get().strip()
        if not key:
            self._test_message("Enter an API key first.")
            return
        self.test_button.configure(state="disabled")
        self._test_message(f"Testing {config.label} with a short sample…")
        results = queue.Queue()
        started = time.monotonic()

        def check():
            client = None
            try:
                client = create_polisher(config, key)
                client.polish("um hello this is a dictation test")
                result = f"{config.label} replied in {time.monotonic() - started:.1f} s."
            except PolishError as e:
                result = f"{config.label}: {e}"
            except Exception:
                result = f"Could not connect to {config.label}. Check the key and endpoint."
            finally:
                if client is not None:
                    client.close()
            results.put(result)

        def poll():
            try:
                result = results.get_nowait()
            except queue.Empty:
                if time.monotonic() - started < config.timeout_s:
                    self.later(100, poll)
                    return
                result = f"{config.label} did not reply within {config.timeout_s:g} s."
            self._test_message(result)
            self.test_button.configure(state="normal")

        threading.Thread(target=check, name="settings-test", daemon=True).start()
        self.later(100, poll)

    def _test_message(self, text):
        self.test_status.set(text)
        if self.test_label.winfo_ismapped():
            self.postprocess_page.reveal(self.test_label)

    def later(self, delay, callback):
        def call():
            self.after_ids.discard(handle)
            if not self.closed: callback()
        handle = self.root.after(delay, call)
        self.after_ids.add(handle)

    def _wheel(self, event):
        if isinstance(event.widget, (tk.Text, tk.Listbox, ttk.Combobox)):
            return
        for page in self.pages:
            if page.contains(event.widget):
                page.canvas.yview_scroll(-int(event.delta / 120) * 3, "units")
                return "break"

    def _focus(self, event):
        for page in self.pages:
            if page.contains(event.widget):
                page.reveal(event.widget)
                break

    def close(self):
        if self.dirty and not messagebox.askyesno("Unsaved changes", "Discard your unsaved changes and close?", parent=self.root):
            return
        self.closed = True
        for handle in self.after_ids: self.root.after_cancel(handle)
        self.root.destroy()


def main() -> int:
    from .app import ERROR_ALREADY_EXISTS, WAIT_OBJECT_0, kernel32, signal_event, user32
    mutex = kernel32.CreateMutexW(None, True, SETTINGS_MUTEX)
    if not mutex:
        raise ctypes.WinError(ctypes.get_last_error())
    if ctypes.get_last_error() == ERROR_ALREADY_EXISTS:
        kernel32.CloseHandle(mutex)
        signal_event(SHOW_EVENT)
        return 0
    show = kernel32.CreateEventW(None, False, False, SHOW_EVENT)
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    root = tk.Tk()
    root.withdraw()
    try:
        window = SettingsWindow(root)
        def activate():
            root.deiconify()
            root.lift()
            root.focus_force()
        def requested():
            if kernel32.WaitForSingleObject(show, 0) == WAIT_OBJECT_0: activate()
            window.later(150, requested)
        activate()
        window.later(150, requested)
        root.mainloop()
    except (ConfigError, OSError) as e:
        messagebox.showerror("Parakeet Dictate settings", str(e), parent=root)
        root.destroy()
        return 1
    finally:
        kernel32.CloseHandle(show)
        kernel32.CloseHandle(mutex)
    return 0
