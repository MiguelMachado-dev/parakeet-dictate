import ctypes
import sys
from ctypes import wintypes
from pathlib import Path

import pytest

from parakeet_dictate import foreground


@pytest.fixture
def window(monkeypatch):
    """A real, hidden window owned by this Python process; never changes the user's focus."""
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.CreateWindowExW.argtypes = [
        wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD,
        ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
        wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID,
    ]
    user32.CreateWindowExW.restype = wintypes.HWND
    user32.DestroyWindow.argtypes = [wintypes.HWND]
    hwnd = user32.CreateWindowExW(0, "STATIC", "Parakeet app rules test", 0, 0, 0, 1, 1, None, None, None, None)
    assert hwnd, ctypes.get_last_error()
    monkeypatch.setattr(foreground.user32, "GetForegroundWindow", lambda: hwnd)
    try:
        yield hwnd
    finally:
        user32.DestroyWindow(hwnd)


def test_window_owner_is_resolved_through_real_win32_process_apis(window):
    assert foreground.foreground_executable().casefold() == Path(sys.executable).name.casefold()


def test_no_foreground_window_means_no_app(monkeypatch):
    monkeypatch.setattr(foreground.user32, "GetForegroundWindow", lambda: None)
    assert foreground.foreground_executable() is None


def test_vanished_window_means_no_app(monkeypatch):
    monkeypatch.setattr(foreground.user32, "GetForegroundWindow", lambda: 0x12345678)
    assert foreground.foreground_executable() is None


def test_a_process_that_cannot_be_opened_means_no_app(window, monkeypatch):
    monkeypatch.setattr(foreground.kernel32, "OpenProcess", lambda *args: None)
    assert foreground.foreground_executable() is None


@pytest.mark.parametrize("readable", [True, False])
def test_the_process_handle_is_closed_even_when_its_name_cannot_be_read(window, monkeypatch, readable):
    closed = []
    close = foreground.kernel32.CloseHandle

    def tracked_close(handle):
        closed.append(handle)
        return close(handle)

    monkeypatch.setattr(foreground.kernel32, "CloseHandle", tracked_close)
    if not readable:
        monkeypatch.setattr(foreground.kernel32, "QueryFullProcessImageNameW", lambda *args: False)
    assert bool(foreground.foreground_executable()) == readable
    assert len(closed) == 1
