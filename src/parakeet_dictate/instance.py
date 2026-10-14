"""The named kernel objects the app's processes share: the tray app's single-instance mutex and the events
it waits on, and how to start the tray app in the background."""

import ctypes
import subprocess
from ctypes import wintypes

from .config import ROOT

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.CreateMutexW.argtypes = [wintypes.LPVOID, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.CreateMutexW.restype = wintypes.HANDLE
kernel32.CreateEventW.argtypes = [wintypes.LPVOID, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.CreateEventW.restype = wintypes.HANDLE
kernel32.OpenEventW.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.OpenEventW.restype = wintypes.HANDLE
kernel32.SetEvent.argtypes = [wintypes.HANDLE]
kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel32.WaitForSingleObject.restype = wintypes.DWORD
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]

MUTEX_NAME = "Local\\parakeet-dictate"
QUIT_EVENT_NAME = "Local\\parakeet-dictate-quit"
LAUNCH_EVENT_NAME = "Local\\parakeet-dictate-launched"
RELOAD_EVENT_NAME = "Local\\parakeet-dictate-reload"
ERROR_ALREADY_EXISTS = 183
EVENT_MODIFY_STATE = 0x0002
INFINITE = 0xFFFFFFFF
WAIT_OBJECT_0 = 0


def signal_event(name: str) -> bool:
    handle = kernel32.OpenEventW(EVENT_MODIFY_STATE, False, name)
    if not handle:
        return False
    kernel32.SetEvent(handle)
    kernel32.CloseHandle(handle)
    return True


def launch_background(*arguments: str) -> None:
    subprocess.Popen(
        [str(ROOT / ".venv" / "Scripts" / "pythonw.exe"), "-m", "parakeet_dictate", *arguments],
        cwd=ROOT, creationflags=subprocess.CREATE_NO_WINDOW,
    )
