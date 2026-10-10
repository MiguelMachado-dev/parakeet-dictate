"""Put text into the focused window: swap it onto the clipboard, send Ctrl+V, restore the clipboard."""

import ctypes
import time
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

CF_UNICODETEXT = 13
GMEM_MOVEABLE = 0x0002
INPUT_KEYBOARD = 1
KEYEVENTF_EXTENDEDKEY = 0x0001
KEYEVENTF_KEYUP = 0x0002
MAPVK_VK_TO_VSC = 0
VK_CONTROL = 0x11
VK_ESCAPE = 0x1B
VK_RCONTROL = 0xA3
VK_RMENU = 0xA5
VK_V = 0x56
EXTENDED_VKS = {VK_RCONTROL, VK_RMENU}

# Stamped into dwExtraInfo of our own synthetic keys so the hotkey listener can skip them.
INJECTED_TAG = 0x5044
# How long the target app gets to read the clipboard before the snapshot goes back.
SETTLE_S = 0.2


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [
        ("wVk", wintypes.WORD),
        ("wScan", wintypes.WORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.c_size_t),
    ]


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", wintypes.LONG),
        ("dy", wintypes.LONG),
        ("mouseData", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.c_size_t),
    ]


class _INPUTUNION(ctypes.Union):
    # MOUSEINPUT is the largest member; it sets sizeof(INPUT), which SendInput checks.
    _fields_ = [("ki", KEYBDINPUT), ("mi", MOUSEINPUT)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("u", _INPUTUNION)]


user32.SendInput.argtypes = [wintypes.UINT, ctypes.POINTER(INPUT), ctypes.c_int]
user32.SendInput.restype = wintypes.UINT
user32.MapVirtualKeyW.argtypes = [wintypes.UINT, wintypes.UINT]
user32.MapVirtualKeyW.restype = wintypes.UINT
user32.OpenClipboard.argtypes = [wintypes.HWND]
user32.OpenClipboard.restype = wintypes.BOOL
user32.CloseClipboard.restype = wintypes.BOOL
user32.EmptyClipboard.restype = wintypes.BOOL
user32.GetClipboardData.argtypes = [wintypes.UINT]
user32.GetClipboardData.restype = wintypes.HANDLE
user32.SetClipboardData.argtypes = [wintypes.UINT, wintypes.HANDLE]
user32.SetClipboardData.restype = wintypes.HANDLE
user32.GetClipboardSequenceNumber.restype = wintypes.DWORD
user32.RegisterClipboardFormatW.argtypes = [wintypes.LPCWSTR]
user32.RegisterClipboardFormatW.restype = wintypes.UINT
kernel32.GlobalAlloc.argtypes = [wintypes.UINT, ctypes.c_size_t]
kernel32.GlobalAlloc.restype = wintypes.HGLOBAL
kernel32.GlobalLock.argtypes = [wintypes.HGLOBAL]
kernel32.GlobalLock.restype = wintypes.LPVOID
kernel32.GlobalUnlock.argtypes = [wintypes.HGLOBAL]
kernel32.GlobalUnlock.restype = wintypes.BOOL
kernel32.GlobalFree.argtypes = [wintypes.HGLOBAL]
kernel32.GlobalFree.restype = wintypes.HGLOBAL

# Clipboard history (Win+V) and clipboard managers skip content carrying this format.
CF_EXCLUDE_FROM_HISTORY = user32.RegisterClipboardFormatW("ExcludeClipboardContentFromMonitorProcessing")


def paste(text: str) -> None:
    """Paste text into the focused window, then put the previous clipboard text back.

    Only text survives the round trip: an image or file list on the clipboard is lost.
    """
    previous = get_text()
    set_text(text)
    ours = user32.GetClipboardSequenceNumber()
    send_keys([(VK_CONTROL, False), (VK_V, False), (VK_V, True), (VK_CONTROL, True)])
    time.sleep(SETTLE_S)
    # If something else wrote the clipboard meanwhile (the user copied), keep that instead.
    if user32.GetClipboardSequenceNumber() == ours:
        set_text(previous, keep_out_of_history=True)


def send_keys(strokes: Sequence[tuple[int, bool]], *, tag: int = INJECTED_TAG) -> None:
    """Send (virtual key, is_key_up) strokes as one atomic SendInput batch."""
    inputs = (INPUT * len(strokes))()
    for item, (vk, up) in zip(inputs, strokes):
        item.type = INPUT_KEYBOARD
        item.u.ki = KEYBDINPUT(
            wVk=vk,
            wScan=user32.MapVirtualKeyW(vk, MAPVK_VK_TO_VSC),
            dwFlags=(KEYEVENTF_KEYUP if up else 0) | (KEYEVENTF_EXTENDEDKEY if vk in EXTENDED_VKS else 0),
            time=0,
            dwExtraInfo=tag,
        )
    if user32.SendInput(len(inputs), inputs, ctypes.sizeof(INPUT)) != len(inputs):
        raise ctypes.WinError(ctypes.get_last_error())


def get_text() -> str | None:
    with _clipboard():
        handle = user32.GetClipboardData(CF_UNICODETEXT)
        if not handle:
            return None
        pointer = kernel32.GlobalLock(handle)
        if not pointer:
            return None
        try:
            return ctypes.wstring_at(pointer)
        finally:
            kernel32.GlobalUnlock(handle)


def set_text(text: str | None, *, keep_out_of_history: bool = False) -> None:
    """Replace the clipboard with text, or leave it empty for None."""
    with _clipboard():
        if not user32.EmptyClipboard():
            raise ctypes.WinError(ctypes.get_last_error())
        if text is None:
            return
        _set_data(CF_UNICODETEXT, (text + "\0").encode("utf-16-le"))
        if keep_out_of_history and CF_EXCLUDE_FROM_HISTORY:
            _set_data(CF_EXCLUDE_FROM_HISTORY, b"\0")


@contextmanager
def _clipboard() -> Iterator[None]:
    # Other apps hold the clipboard open for brief moments; retry instead of failing the paste.
    for _ in range(50):
        if user32.OpenClipboard(None):
            break
        time.sleep(0.01)
    else:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        yield
    finally:
        user32.CloseClipboard()


def _set_data(fmt: int, data: bytes) -> None:
    handle = kernel32.GlobalAlloc(GMEM_MOVEABLE, len(data))
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    pointer = kernel32.GlobalLock(handle)
    ctypes.memmove(pointer, data, len(data))
    kernel32.GlobalUnlock(handle)
    if not user32.SetClipboardData(fmt, handle):
        kernel32.GlobalFree(handle)
        raise ctypes.WinError(ctypes.get_last_error())
