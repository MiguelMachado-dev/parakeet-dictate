"""Drive the real overlay window through scripted dictations and measure what it costs.

Prints the process's GDI and USER handle counts across show/hide cycles and a long recording,
the overlay thread's CPU while hidden and while drawing, the frame rate, and render time. It sends
no keys and installs no keyboard hook; the window shows above the taskbar while it runs.
"""

import ctypes
import sys
import time
from ctypes import wintypes

import numpy as np

from e2e_check import FixtureRecorder, load_fixture
from parakeet_dictate import render
from parakeet_dictate.overlay import Overlay
from parakeet_dictate.reducer import Event, HotkeyDown, HotkeyUp, Idle, State, TranscriptReady, Transition, step

CYCLES = 10
LONG_FRAMES = 300
IDLE_S = 10.0
GR_GDIOBJECTS, GR_USEROBJECTS = 0, 1
THREAD_QUERY_LIMITED_INFORMATION = 0x0800

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32.GetGuiResources.argtypes = [wintypes.HANDLE, wintypes.DWORD]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
kernel32.GetCurrentProcess.restype = wintypes.HANDLE
kernel32.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.OpenThread.restype = wintypes.HANDLE
kernel32.GetThreadTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
kernel32.QueryThreadCycleTime.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_ulonglong)]


def handles() -> tuple[int, int]:
    process = kernel32.GetCurrentProcess()
    return user32.GetGuiResources(process, GR_GDIOBJECTS), user32.GetGuiResources(process, GR_USEROBJECTS)


class ThreadClock:
    def __init__(self, thread_id: int) -> None:
        self._handle = kernel32.OpenThread(THREAD_QUERY_LIMITED_INFORMATION, False, thread_id)

    def read(self) -> tuple[float, int]:
        """(kernel + user CPU seconds, CPU cycles)."""
        times = [wintypes.FILETIME() for _ in range(4)]
        kernel32.GetThreadTimes(self._handle, *map(ctypes.byref, times))
        cpu = sum((t.dwHighDateTime << 32 | t.dwLowDateTime) for t in times[2:]) / 1e7
        cycles = ctypes.c_ulonglong()
        kernel32.QueryThreadCycleTime(self._handle, ctypes.byref(cycles))
        return cpu, cycles.value


class Driver:
    """Feeds reducer transitions to the overlay as Dictation would."""

    def __init__(self, overlay: Overlay, recorder: FixtureRecorder) -> None:
        self.overlay, self.recorder = overlay, recorder
        self.state: State = Idle()

    def post(self, event: Event) -> None:
        previous = self.state
        self.state, effects = step(previous, event)
        if self.state != previous:
            self.overlay.notify(Transition(previous, self.state, event, tuple(effects)))
        if isinstance(event, HotkeyDown) and isinstance(previous, Idle):
            self.recorder.start()
        elif isinstance(event, HotkeyUp):
            self.recorder.stop()


def wait_for(probe, what: str, timeout_s: float = 5.0) -> None:
    deadline = time.perf_counter() + timeout_s
    while not probe():
        if time.perf_counter() > deadline:
            raise AssertionError(f"timed out waiting for {what}")
        time.sleep(0.005)


def main() -> int:
    render_ms: list[float] = []
    draw = render.render

    def timed_render(frame):
        t0 = time.perf_counter()
        image = draw(frame)
        render_ms.append((time.perf_counter() - t0) * 1000)
        return image

    render.render = timed_render
    recorder = FixtureRecorder()
    recorder.audio = load_fixture("en")[0]
    overlay = Overlay(lambda: recorder.meter.snapshot())
    clock = ThreadClock(overlay.thread_id)
    driver = Driver(overlay, recorder)
    visible = lambda: bool(user32.IsWindowVisible(overlay.hwnd))
    failures: list[str] = []

    def check(ok: bool, message: str) -> None:
        print(f"  {'ok  ' if ok else 'FAIL'} {message}")
        if not ok:
            failures.append(message)

    try:
        before = handles()
        print(f"handles before any frame: GDI {before[0]}, USER {before[1]}")
        cycle_handles = []
        for _ in range(CYCLES):
            t = time.perf_counter()
            driver.post(HotkeyDown(t))
            wait_for(visible, "the window to show")
            time.sleep(0.4)
            driver.post(HotkeyUp(time.perf_counter()))
            driver.post(TranscriptReady("text", 1))
            wait_for(lambda: not visible(), "the window to hide after the beat")
            cycle_handles.append(handles())
        print(f"after each of {CYCLES} show/hide cycles: GDI {[g for g, _ in cycle_handles]}, USER {[u for _, u in cycle_handles]}")
        check(all(h == cycle_handles[0] for h in cycle_handles), f"handle counts flat across {CYCLES} cycles")

        driver.post(HotkeyDown(time.perf_counter()))
        wait_for(visible, "the window to show")
        start_frames, start_cpu, t0 = overlay.frames, clock.read(), time.perf_counter()
        samples = []
        while overlay.frames - start_frames < LONG_FRAMES:
            if (overlay.frames - start_frames) // 100 >= len(samples):
                samples.append((overlay.frames - start_frames, handles()))
            time.sleep(0.01)
        samples.append((overlay.frames - start_frames, handles()))
        drawing_s, drawing_cpu = time.perf_counter() - t0, clock.read()
        levels_seen = overlay.last_frame.levels.max() if overlay.last_frame else 0.0
        driver.post(HotkeyUp(time.perf_counter()))
        driver.post(TranscriptReady("text", 1))
        wait_for(lambda: not visible(), "the window to hide after the beat")
        print("during one recording: " + ", ".join(f"frame {n}: GDI {g} USER {u}" for n, (g, u) in samples))
        check(len({h for _, h in samples}) == 1, f"handle counts flat across {LONG_FRAMES} frames")
        fps = LONG_FRAMES / drawing_s
        print(f"drawing: {fps:.1f} fps, overlay thread CPU {(drawing_cpu[0] - start_cpu[0]) / drawing_s * 100:.1f}% of a core,"
              f" {(drawing_cpu[1] - start_cpu[1]) / LONG_FRAMES / 1e6:.2f} M cycles per frame")
        check(fps > 55, "about 60 fps while visible")
        check(levels_seen > 0, f"frames carry live levels (max {levels_seen:.2f})")
        ms = np.array(render_ms)
        print(f"render ms over {len(ms)} frames: p50 {np.percentile(ms, 50):.2f}, p95 {np.percentile(ms, 95):.2f}, max {ms.max():.2f}")

        idle_from = clock.read()
        time.sleep(IDLE_S)
        idle_to = clock.read()
        print(f"hidden for {IDLE_S:.0f} s: overlay thread CPU {idle_to[0] - idle_from[0]:.4f} s, {idle_to[1] - idle_from[1]} cycles")
        check(idle_to[1] == idle_from[1], "overlay thread did not run at all while hidden")
        after = handles()
        print(f"handles at the end: GDI {after[0]}, USER {after[1]}")
    finally:
        overlay.close()
    print("PASS" if not failures else f"{len(failures)} FAILED")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
