import ctypes
import threading
from ctypes import wintypes

import numpy as np

from parakeet_dictate.recorder import Audio
from parakeet_dictate.transcriber import Transcriber

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.GetCurrentThread.restype = wintypes.HANDLE
kernel32.GetThreadInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
THREAD_POWER_THROTTLING = 3


class PowerThrottling(ctypes.Structure):
    _fields_ = [("Version", wintypes.ULONG), ("ControlMask", wintypes.ULONG), ("StateMask", wintypes.ULONG)]


class Model:
    def recognize(self, samples, sample_rate: int) -> str:
        return "hello"


def power_throttling_after(transcribe) -> tuple[int, int]:
    """(ControlMask, StateMask) of a new thread's power throttling after it runs transcribe."""
    seen = {}

    def run() -> None:
        transcribe()
        state = PowerThrottling(Version=1)
        ok = kernel32.GetThreadInformation(kernel32.GetCurrentThread(), THREAD_POWER_THROTTLING, ctypes.byref(state), ctypes.sizeof(state))
        seen["state"] = (state.ControlMask, state.StateMask) if ok else ctypes.WinError(ctypes.get_last_error())

    thread = threading.Thread(target=run)
    thread.start()
    thread.join()
    return seen["state"]


def test_a_transcribing_thread_asks_for_full_speed():
    transcriber = Transcriber(Model(), "CUDAExecutionProvider", None)

    assert power_throttling_after(lambda: transcriber.transcribe(Audio(np.zeros(16_000, np.float32), 16_000))) == (1, 0)


def test_a_thread_left_alone_keeps_the_system_default():
    assert power_throttling_after(lambda: None) == (0, 0)
