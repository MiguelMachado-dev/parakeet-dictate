import ctypes
import json
import os
import queue
import sys
import threading
import tkinter as tk
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32.GetForegroundWindow.restype = wintypes.HWND
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
user32.AttachThreadInput.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.BOOL]
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
user32.BringWindowToTop.argtypes = [wintypes.HWND]
user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
SW_RESTORE = 9

root = tk.Tk()
root.title("parakeet-dictate e2e target")
root.geometry("640x240+200+200")
root.attributes("-topmost", True)
box = tk.Text(root, font=("Segoe UI", 12))
box.pack(fill="both", expand=True)
commands: queue.Queue[str] = queue.Queue()


def read_stdin() -> None:
    for line in sys.stdin:
        commands.put(line.strip())
    commands.put("quit")


def bring_to_front() -> None:
    # Windows refuses SetForegroundWindow from a background process unless its input is
    # attached to the current foreground thread.
    hwnd = int(root.wm_frame(), 16)
    foreground_thread = user32.GetWindowThreadProcessId(user32.GetForegroundWindow(), None)
    me = kernel32.GetCurrentThreadId()
    attached = foreground_thread not in (0, me) and user32.AttachThreadInput(me, foreground_thread, True)
    try:
        user32.ShowWindow(hwnd, SW_RESTORE)
        user32.BringWindowToTop(hwnd)
        user32.SetForegroundWindow(hwnd)
    finally:
        if attached:
            user32.AttachThreadInput(me, foreground_thread, False)
    root.focus_force()
    box.focus_set()


def reply(message: dict) -> None:
    print(json.dumps(message), flush=True)


def placement() -> dict:
    root.update()
    return {
        "origin": [root.winfo_x(), root.winfo_y()],
        "center": [root.winfo_rootx() + root.winfo_width() // 2, root.winfo_rooty() + root.winfo_height() // 2],
    }


def poll() -> None:
    while not commands.empty():
        match commands.get().split():
            case ["focus"]:
                bring_to_front()
                reply({"ok": True} | placement())
            case ["move", x, y]:
                root.geometry(f"+{int(x)}+{int(y)}")
                reply({"ok": True} | placement())
            case ["text"]:
                reply({"text": box.get("1.0", "end-1c")})
            case ["clear"]:
                box.delete("1.0", "end")
                reply({"ok": True})
            case ["quit"]:
                root.destroy()
                return
    root.after(5, poll)


threading.Thread(target=read_stdin, daemon=True).start()
root.update()
reply({"pid": os.getpid()})
root.after(5, poll)
root.mainloop()
