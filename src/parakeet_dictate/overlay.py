"""The on-screen overlay: a layered Win32 window that never takes focus or input.

It runs on its own thread with its own message loop. While something is on screen it draws at
FPS; otherwise the thread blocks on an event with no timer armed, so it uses no CPU.
"""

import ctypes
import logging
import queue
import threading
import time
from collections.abc import Callable
from ctypes import wintypes
from dataclasses import dataclass

import numpy as np
import numpy.typing as npt
from PIL import Image

from .overlay_frame import FPS, OverlayFrame, Renderer
from .overlay_scene import Command, Placement, Scene, command_for, place
from .reducer import Transition

log = logging.getLogger(__name__)

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
shcore = ctypes.WinDLL("shcore")

CLASS_NAME = "ParakeetDictateOverlay"
WS_POPUP = 0x80000000
WS_EX_TOPMOST = 0x00000008
WS_EX_TRANSPARENT = 0x00000020
WS_EX_TOOLWINDOW = 0x00000080
WS_EX_LAYERED = 0x00080000
WS_EX_NOACTIVATE = 0x08000000
EX_STYLE = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST
HWND_TOPMOST = -1
SWP_NOSIZE, SWP_NOMOVE, SWP_NOZORDER, SWP_NOACTIVATE = 0x0001, 0x0002, 0x0004, 0x0010
SWP_SHOWWINDOW, SWP_HIDEWINDOW = 0x0040, 0x0080
WM_MOUSEACTIVATE, WM_NCHITTEST = 0x0021, 0x0084
MA_NOACTIVATE, HTTRANSPARENT = 3, -1
ULW_ALPHA, AC_SRC_OVER, AC_SRC_ALPHA = 0x2, 0x0, 0x1
BI_RGB, DIB_RGB_COLORS = 0, 0
PM_REMOVE = 0x0001
QS_ALLINPUT = 0x04FF
MWMO_INPUTAVAILABLE = 0x0004
INFINITE = 0xFFFFFFFF
WAIT_FAILED = 0xFFFFFFFF
CREATE_WAITABLE_TIMER_HIGH_RESOLUTION = 0x2
TIMER_ALL_ACCESS = 0x1F0003
MONITOR_DEFAULTTOPRIMARY, MONITOR_DEFAULTTONEAREST = 1, 2
MDT_EFFECTIVE_DPI = 0
DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
ERROR_CLASS_ALREADY_EXISTS = 1410

LRESULT = ctypes.c_ssize_t
WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)


class WNDCLASSEXW(ctypes.Structure):
    _fields_ = [
        ("cbSize", wintypes.UINT),
        ("style", wintypes.UINT),
        ("lpfnWndProc", WNDPROC),
        ("cbClsExtra", ctypes.c_int),
        ("cbWndExtra", ctypes.c_int),
        ("hInstance", wintypes.HINSTANCE),
        ("hIcon", wintypes.HICON),
        ("hCursor", wintypes.HANDLE),
        ("hbrBackground", wintypes.HBRUSH),
        ("lpszMenuName", wintypes.LPCWSTR),
        ("lpszClassName", wintypes.LPCWSTR),
        ("hIconSm", wintypes.HICON),
    ]


class BLENDFUNCTION(ctypes.Structure):
    _fields_ = [("BlendOp", ctypes.c_ubyte), ("BlendFlags", ctypes.c_ubyte), ("SourceConstantAlpha", ctypes.c_ubyte), ("AlphaFormat", ctypes.c_ubyte)]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wintypes.DWORD),
        ("biWidth", wintypes.LONG),
        ("biHeight", wintypes.LONG),
        ("biPlanes", wintypes.WORD),
        ("biBitCount", wintypes.WORD),
        ("biCompression", wintypes.DWORD),
        ("biSizeImage", wintypes.DWORD),
        ("biXPelsPerMeter", wintypes.LONG),
        ("biYPelsPerMeter", wintypes.LONG),
        ("biClrUsed", wintypes.DWORD),
        ("biClrImportant", wintypes.DWORD),
    ]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", wintypes.DWORD * 1)]


class MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT), ("rcWork", wintypes.RECT), ("dwFlags", wintypes.DWORD)]


user32.DefWindowProcW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.DefWindowProcW.restype = LRESULT
user32.RegisterClassExW.argtypes = [ctypes.POINTER(WNDCLASSEXW)]
user32.RegisterClassExW.restype = wintypes.ATOM
user32.UnregisterClassW.argtypes = [wintypes.LPCWSTR, wintypes.HINSTANCE]
user32.CreateWindowExW.argtypes = [
    wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_int, ctypes.c_int, ctypes.c_int,
    ctypes.c_int, wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID,
]
user32.CreateWindowExW.restype = wintypes.HWND
user32.DestroyWindow.argtypes = [wintypes.HWND]
user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
user32.UpdateLayeredWindow.argtypes = [
    wintypes.HWND, wintypes.HDC, ctypes.POINTER(wintypes.POINT), ctypes.POINTER(wintypes.SIZE), wintypes.HDC,
    ctypes.POINTER(wintypes.POINT), wintypes.COLORREF, ctypes.POINTER(BLENDFUNCTION), wintypes.DWORD,
]
user32.PeekMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND, wintypes.UINT, wintypes.UINT, wintypes.UINT]
user32.TranslateMessage.argtypes = [ctypes.POINTER(wintypes.MSG)]
user32.DispatchMessageW.argtypes = [ctypes.POINTER(wintypes.MSG)]
user32.MsgWaitForMultipleObjectsEx.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE), wintypes.DWORD, wintypes.DWORD, wintypes.DWORD]
user32.MsgWaitForMultipleObjectsEx.restype = wintypes.DWORD
user32.GetForegroundWindow.restype = wintypes.HWND
user32.MonitorFromWindow.argtypes = [wintypes.HWND, wintypes.DWORD]
user32.MonitorFromWindow.restype = wintypes.HMONITOR
user32.MonitorFromPoint.argtypes = [wintypes.POINT, wintypes.DWORD]
user32.MonitorFromPoint.restype = wintypes.HMONITOR
user32.GetMonitorInfoW.argtypes = [wintypes.HMONITOR, ctypes.POINTER(MONITORINFO)]
user32.SetThreadDpiAwarenessContext.argtypes = [ctypes.c_void_p]
user32.SetThreadDpiAwarenessContext.restype = ctypes.c_void_p
shcore.GetDpiForMonitor.argtypes = [wintypes.HMONITOR, ctypes.c_int, ctypes.POINTER(wintypes.UINT), ctypes.POINTER(wintypes.UINT)]
shcore.GetDpiForMonitor.restype = ctypes.c_long
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.CreateDIBSection.argtypes = [wintypes.HDC, ctypes.POINTER(BITMAPINFO), wintypes.UINT, ctypes.POINTER(ctypes.c_void_p), wintypes.HANDLE, wintypes.DWORD]
gdi32.CreateDIBSection.restype = wintypes.HBITMAP
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi32.SelectObject.restype = wintypes.HGDIOBJ
gdi32.DeleteObject.argtypes = [wintypes.HGDIOBJ]
gdi32.DeleteDC.argtypes = [wintypes.HDC]
kernel32.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]
kernel32.GetModuleHandleW.restype = wintypes.HMODULE
kernel32.CreateEventW.argtypes = [wintypes.LPVOID, wintypes.BOOL, wintypes.BOOL, wintypes.LPCWSTR]
kernel32.CreateEventW.restype = wintypes.HANDLE
kernel32.SetEvent.argtypes = [wintypes.HANDLE]
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
kernel32.CreateWaitableTimerExW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD]
kernel32.CreateWaitableTimerExW.restype = wintypes.HANDLE
kernel32.SetWaitableTimer.argtypes = [
    wintypes.HANDLE, ctypes.POINTER(wintypes.LARGE_INTEGER), wintypes.LONG, wintypes.LPVOID, wintypes.LPVOID, wintypes.BOOL,
]
kernel32.CancelWaitableTimer.argtypes = [wintypes.HANDLE]


def premultiplied_bgra(rgba: npt.NDArray[np.uint8]) -> npt.NDArray[np.uint8]:
    """Straight-alpha RGBA to the premultiplied BGRA that UpdateLayeredWindow blends with AC_SRC_ALPHA."""
    alpha = rgba[..., 3:4].astype(np.uint16)
    out = np.empty_like(rgba)
    out[..., :3] = (rgba[..., 2::-1].astype(np.uint16) * alpha + 127) // 255
    out[..., 3] = rgba[..., 3]
    return out


def _wndproc(hwnd: int, msg: int, wparam: int, lparam: int) -> int:
    if msg == WM_NCHITTEST:
        return HTTRANSPARENT
    if msg == WM_MOUSEACTIVATE:
        return MA_NOACTIVATE
    return user32.DefWindowProcW(hwnd, msg, wparam, lparam)


class _Window:
    """The layered window and the one DIB section it is drawn from, reallocated only on a size change."""

    def __init__(self) -> None:
        self._instance = kernel32.GetModuleHandleW(None)
        self._wndproc = WNDPROC(_wndproc)
        wc = WNDCLASSEXW(cbSize=ctypes.sizeof(WNDCLASSEXW), lpfnWndProc=self._wndproc, hInstance=self._instance, lpszClassName=CLASS_NAME)
        if not user32.RegisterClassExW(ctypes.byref(wc)) and ctypes.get_last_error() != ERROR_CLASS_ALREADY_EXISTS:
            raise ctypes.WinError(ctypes.get_last_error())
        self.hwnd = user32.CreateWindowExW(EX_STYLE, CLASS_NAME, "Parakeet Dictate overlay", WS_POPUP, 0, 0, 0, 0, None, None, self._instance, None)
        if not self.hwnd:
            raise ctypes.WinError(ctypes.get_last_error())
        self._blend = BLENDFUNCTION(AC_SRC_OVER, 0, 255, AC_SRC_ALPHA)
        self._dc: int | None = None
        self._bitmap: int | None = None
        self._previous_bitmap: int | None = None
        self._pixels: npt.NDArray[np.uint8] | None = None
        self._size = (0, 0)
        self._visible = False

    def draw(self, image: Image.Image, placement: Placement) -> None:
        size = (placement.width, placement.height)
        if size != self._size:
            self._allocate(size)
        self._pixels[...] = premultiplied_bgra(np.asarray(image))
        if not user32.UpdateLayeredWindow(
            self.hwnd, None, ctypes.byref(wintypes.POINT(placement.x, placement.y)), ctypes.byref(wintypes.SIZE(*size)),
            self._dc, ctypes.byref(wintypes.POINT(0, 0)), 0, ctypes.byref(self._blend), ULW_ALPHA,
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        if not self._visible:
            # SetWindowPos rather than ShowWindow: a process's first ShowWindow call can take its
            # show command from STARTUPINFO (pythonw from a shortcut), which would activate the window.
            user32.SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW)
            self._visible = True

    def hide(self) -> None:
        if self._visible:
            user32.SetWindowPos(self.hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW)
            self._visible = False

    def destroy(self) -> None:
        self._free()
        user32.DestroyWindow(self.hwnd)
        user32.UnregisterClassW(CLASS_NAME, self._instance)

    def _allocate(self, size: tuple[int, int]) -> None:
        self._free()
        width, height = size
        info = BITMAPINFO()
        # A negative height makes the rows top-down, the order numpy and Pillow use.
        info.bmiHeader = BITMAPINFOHEADER(ctypes.sizeof(BITMAPINFOHEADER), width, -height, 1, 32, BI_RGB)
        bits = ctypes.c_void_p()
        self._dc = gdi32.CreateCompatibleDC(None)
        self._bitmap = gdi32.CreateDIBSection(self._dc, ctypes.byref(info), DIB_RGB_COLORS, ctypes.byref(bits), None, 0)
        if not self._dc or not self._bitmap:
            raise ctypes.WinError(ctypes.get_last_error())
        self._previous_bitmap = gdi32.SelectObject(self._dc, self._bitmap)
        buffer = (ctypes.c_uint8 * (width * height * 4)).from_address(bits.value)
        self._pixels = np.ctypeslib.as_array(buffer).reshape(height, width, 4)
        self._size = size

    def _free(self) -> None:
        self._pixels = None
        if self._dc:
            gdi32.SelectObject(self._dc, self._previous_bitmap)
            gdi32.DeleteObject(self._bitmap)
            gdi32.DeleteDC(self._dc)
        self._dc = self._bitmap = self._previous_bitmap = None
        self._size = (0, 0)


def _placement_for(foreground: int | None) -> Placement:
    """Where a dictation shows: the monitor of the window that had focus when it started."""
    monitor = (
        user32.MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST)
        if foreground
        else user32.MonitorFromPoint(wintypes.POINT(0, 0), MONITOR_DEFAULTTOPRIMARY)
    )
    info = MONITORINFO(cbSize=ctypes.sizeof(MONITORINFO))
    user32.GetMonitorInfoW(monitor, ctypes.byref(info))
    dpi_x, dpi_y = wintypes.UINT(96), wintypes.UINT(96)
    shcore.GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, ctypes.byref(dpi_x), ctypes.byref(dpi_y))
    work = info.rcWork
    return place((work.left, work.top, work.right, work.bottom), dpi_x.value)


@dataclass(frozen=True)
class _Restyle:
    renderer: Renderer | None


class Overlay:
    def __init__(
        self, renderer: Renderer, levels: Callable[[], tuple[npt.NDArray[np.float32], npt.NDArray[np.float32]]]
    ) -> None:
        """levels returns the current meter's (levels, spectrum); the overlay thread calls it each frame."""
        self._renderer = renderer
        self._levels = levels
        self._commands: queue.SimpleQueue[tuple[float, Command, int | None] | _Restyle] = queue.SimpleQueue()
        self._wake = kernel32.CreateEventW(None, False, False, None)
        self._closing = False
        self.hwnd: int | None = None
        self.thread_id = 0
        self.frames = 0
        self.last_frame: OverlayFrame | None = None
        self._failure: BaseException | None = None
        ready = threading.Event()
        self._thread = threading.Thread(target=self._run, args=(ready,), name="overlay", daemon=True)
        self._thread.start()
        ready.wait()
        if self._failure is not None:
            raise self._failure

    def notify(self, transition: Transition) -> None:
        """Called on the controller thread: queue the command and wake the overlay thread."""
        command = command_for(transition)
        if command is not None:
            self._commands.put((time.perf_counter(), command, user32.GetForegroundWindow()))
            kernel32.SetEvent(self._wake)

    def restyle(self, renderer: Renderer | None) -> None:
        """Draw with renderer from the next frame. None turns the overlay off: it hides at once, drops
        the dictation on screen, and ignores transitions until a renderer is set again."""
        self._commands.put(_Restyle(renderer))
        kernel32.SetEvent(self._wake)

    def close(self) -> None:
        self._closing = True
        kernel32.SetEvent(self._wake)
        self._thread.join(timeout=5)
        kernel32.CloseHandle(self._wake)

    def _run(self, ready: threading.Event) -> None:
        # Per thread, so the tray and the paste keep the process's default DPI behavior.
        if not user32.SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2):
            log.warning("overlay: per-monitor DPI awareness unavailable; sizes assume 100%% scale")
        try:
            window = _Window()
        except OSError as e:
            self._failure = e
            ready.set()
            return
        self.hwnd, self.thread_id = window.hwnd, threading.get_native_id()
        ready.set()
        timer = kernel32.CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS)
        handles = (wintypes.HANDLE * 2)(self._wake, timer)
        try:
            self._loop(window, timer, handles)
        except Exception:
            log.exception("overlay stopped")
        finally:
            window.destroy()
            kernel32.CloseHandle(timer)

    def _loop(self, window: _Window, timer: int, handles: ctypes.Array) -> None:
        renderer: Renderer | None = self._renderer
        scene = Scene()
        msg = wintypes.MSG()
        deadline: float | None = None
        while not self._closing:
            while user32.PeekMessageW(ctypes.byref(msg), None, 0, 0, PM_REMOVE):
                user32.TranslateMessage(ctypes.byref(msg))
                user32.DispatchMessageW(ctypes.byref(msg))
            while True:
                try:
                    item = self._commands.get_nowait()
                except queue.Empty:
                    break
                if isinstance(item, _Restyle):
                    renderer = item.renderer
                    if renderer is None:
                        scene = Scene()
                elif renderer is not None:
                    t, command, foreground = item
                    scene.apply(command, t, _placement_for(foreground))

            now = time.perf_counter()
            shown = scene.frame(now, *self._levels())
            if shown is None:
                window.hide()
                self.last_frame, deadline = None, None
                due = scene.next_show()
            else:
                frame, placement = shown
                window.draw(renderer(frame), placement)
                self.frames += 1
                self.last_frame = frame
                deadline = now + 1 / FPS if deadline is None or deadline + 1 / FPS < now else deadline + 1 / FPS
                due = deadline

            if due is None:
                kernel32.CancelWaitableTimer(timer)
            else:
                relative = wintypes.LARGE_INTEGER(-max(1, round((due - time.perf_counter()) * 1e7)))
                kernel32.SetWaitableTimer(timer, ctypes.byref(relative), 0, None, None, False)
            if user32.MsgWaitForMultipleObjectsEx(2, handles, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) == WAIT_FAILED:
                raise ctypes.WinError(ctypes.get_last_error())
