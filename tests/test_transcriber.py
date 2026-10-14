import ctypes
import threading
from ctypes import wintypes

import numpy as np
import onnxruntime
import pytest
from onnx_asr.loader import Manager
from onnx_asr.preprocessors.resampler import Resampler

from parakeet_dictate.recorder import Audio
from parakeet_dictate.transcriber import LazyResampler, Transcriber

CPU = {"providers": ["CPUExecutionProvider"]}

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


def waveform(rate: int) -> tuple[np.ndarray, np.ndarray]:
    return np.random.default_rng(rate).normal(0, 0.1, (1, rate)).astype(np.float32), np.array([rate], np.int64)


def test_the_resampler_opens_a_session_only_for_rates_it_receives(monkeypatch):
    opened = []
    session = onnxruntime.InferenceSession
    monkeypatch.setattr(onnxruntime, "InferenceSession", lambda *args, **kwargs: opened.append(1) or session(*args, **kwargs))

    resampler = LazyResampler(16_000, CPU)
    assert opened == []
    resampler(*waveform(16_000), 16_000)
    assert opened == []
    for _ in range(2):
        resampler(*waveform(44_100), 44_100)
    assert len(opened) == 1


@pytest.mark.parametrize("rate", [44_100, 48_000])
def test_the_resampler_matches_onnx_asrs(rate):
    want = Resampler(16_000, CPU)(*waveform(rate), rate)
    got = LazyResampler(16_000, CPU)(*waveform(rate), rate)

    np.testing.assert_array_equal(got[0], want[0])
    np.testing.assert_array_equal(got[1], want[1])


def test_onnx_asr_still_builds_resamplers_through_the_method_the_app_overrides():
    assert "_create_resampler" in vars(Manager)
