import ctypes
from ctypes import wintypes
from pathlib import PureWindowsPath

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

user32.GetForegroundWindow.argtypes = []
user32.GetForegroundWindow.restype = wintypes.HWND
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
kernel32.QueryFullProcessImageNameW.restype = wintypes.BOOL
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
kernel32.CloseHandle.restype = wintypes.BOOL

PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.EnumWindows.argtypes = [WNDENUMPROC, wintypes.LPARAM]
user32.EnumWindows.restype = wintypes.BOOL
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.IsWindowVisible.restype = wintypes.BOOL


def foreground_executable() -> str | None:
    """The foreground window's executable name, or None if it vanished or cannot be read."""
    return _window_executable(user32.GetForegroundWindow())


def running_executables() -> list[str]:
    """Executable names of apps with visible windows, for the rule editor."""
    names = {}

    @WNDENUMPROC
    def collect(hwnd, unused):
        if user32.IsWindowVisible(hwnd):
            name = _window_executable(hwnd)
            if name:
                names[name.casefold()] = name
        return True

    user32.EnumWindows(collect, 0)
    return sorted(names.values(), key=str.casefold)


def _window_executable(hwnd: int | None) -> str | None:
    pid = wintypes.DWORD()
    if not hwnd or not user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid)) or not pid.value:
        return None
    process = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid.value)
    if not process:
        return None
    try:
        path = ctypes.create_unicode_buffer(32768)
        size = wintypes.DWORD(len(path))
        if not kernel32.QueryFullProcessImageNameW(process, 0, path, ctypes.byref(size)):
            return None
        return PureWindowsPath(path.value).name or None
    finally:
        kernel32.CloseHandle(process)
