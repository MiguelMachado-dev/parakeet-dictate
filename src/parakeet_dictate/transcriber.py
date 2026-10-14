import ctypes
import logging
import re
import threading
import time
from collections.abc import Callable
from ctypes import wintypes
from importlib.resources import files
from pathlib import Path
from typing import Literal, get_args

import numpy as np
import numpy.typing as npt
import onnx_asr.preprocessors
import onnxruntime
from onnx_asr.adapters import TextResultsAsrAdapter
from onnx_asr.loader import Manager
from onnx_asr.onnx import OnnxSessionOptions, TensorRtOptions
from onnx_asr.preprocessors.resampler import Resampler
from onnx_asr.utils import SampleRates

from .recorder import Audio

log = logging.getLogger(__name__)

MODEL_NAME = "nemo-parakeet-tdt-0.6b-v3"
CUDA = "CUDAExecutionProvider"
MIN_SECONDS = 0.1
WARMUP_SECONDS = (1.0, 4.0)
THREAD_POWER_THROTTLING = 3
THREAD_POWER_THROTTLING_EXECUTION_SPEED = 0x1


class ThreadPowerThrottlingState(ctypes.Structure):
    _fields_ = [("Version", wintypes.ULONG), ("ControlMask", wintypes.ULONG), ("StateMask", wintypes.ULONG)]


kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.GetCurrentThread.restype = wintypes.HANDLE
kernel32.SetThreadInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]


def prefer_performance_cores() -> None:
    """Tag the calling thread high QoS. Untagged, Windows can run it on an efficiency core, where a
    transcription took two to four times as long in scripts/bench_resources.py runs."""
    state = ThreadPowerThrottlingState(1, THREAD_POWER_THROTTLING_EXECUTION_SPEED, 0)
    kernel32.SetThreadInformation(kernel32.GetCurrentThread(), THREAD_POWER_THROTTLING, ctypes.byref(state), ctypes.sizeof(state))


class Transcriber:
    def __init__(self, model: TextResultsAsrAdapter, provider: str, quantization: str | None):
        self._model = model
        self.provider = provider
        self.quantization = quantization

    @property
    def label(self) -> str:
        return "CUDA" if self.provider == CUDA else f"CPU {self.quantization or 'fp32'}"

    def transcribe(self, audio: Audio) -> str:
        if audio.seconds < MIN_SECONDS:
            return ""
        prefer_performance_cores()
        samples, rate = audio.samples, audio.rate
        if rate not in get_args(SampleRates):
            samples, rate = resample(samples, rate, 16_000), 16_000
        text = self._model.recognize(samples, sample_rate=rate).strip()
        return text if re.search(r"\w", text) else ""

    def warm_up(self, rate: int = 44_100) -> None:
        noise = np.random.default_rng(0).normal(0, 0.01, int(max(WARMUP_SECONDS) * rate)).astype(np.float32)
        for seconds in WARMUP_SECONDS:
            t0 = time.perf_counter()
            self.transcribe(Audio(noise[: int(seconds * rate)], rate))
            log.info("warm-up %.1f s @ %d Hz: %.0f ms", seconds, rate, (time.perf_counter() - t0) * 1000)


class SharedModel:
    """The one loaded model, shared by local dictation and the server for Mac. It runs one transcription
    at a time: a caller that arrives during another's waits for it."""

    def __init__(self) -> None:
        self._transcriber: Transcriber | None = None
        self._lock = threading.Lock()

    @property
    def ready(self) -> bool:
        return self._transcriber is not None

    def load(self, transcriber: Transcriber) -> None:
        self._transcriber = transcriber

    def transcribe(self, audio: Audio) -> str:
        with self._lock:
            return self._transcriber.transcribe(audio)


def resample(samples: npt.NDArray[np.float32], rate: int, target: int) -> npt.NDArray[np.float32]:
    n_out = round(len(samples) * target / rate)
    spectrum = np.fft.rfft(samples)[: n_out // 2 + 1]
    return (np.fft.irfft(spectrum, n_out) * (n_out / len(samples))).astype(np.float32)


class LazyResampler(Resampler):
    """onnx_asr's Resampler opens a session for each of its seven input rates as the model loads, about
    70 MB of GPU memory each. A microphone records at one rate, so open each the first time it arrives."""

    def __init__(self, sample_rate: Literal[8_000, 16_000], onnx_options: OnnxSessionOptions):
        self._target_sample_rate = sample_rate
        self._preprocessors = OpenOnFirstUse(lambda rate: onnxruntime.InferenceSession(
            files(onnx_asr.preprocessors).joinpath("data", f"resample_{rate // 1000}_{sample_rate // 1000}.onnx").read_bytes(),
            **TensorRtOptions.add_profile(onnx_options, self._preprocessor_shapes),
        ))


class OpenOnFirstUse(dict):
    def __init__(self, open_session: Callable[[int], onnxruntime.InferenceSession]):
        super().__init__()
        self._open_session = open_session

    def __missing__(self, rate: int) -> onnxruntime.InferenceSession:
        session = self[rate] = self._open_session(rate)
        return session


class LazyResamplerManager(Manager):
    def _create_resampler(self, sample_rate: Literal[8_000, 16_000]) -> Resampler:
        return LazyResampler(sample_rate, self.resampler_config)


def load_transcriber(model_dir: Path, provider: str, *, cudnn_conv_algo_search: str | None = None) -> Transcriber:
    if not (model_dir / "config.json").exists():
        raise FileNotFoundError(f"no model in {model_dir}; run scripts/download_model.py")
    onnxruntime.set_default_logger_severity(3)
    if provider == "cuda":
        try:
            transcriber = _load(model_dir, cuda=True, cudnn_conv_algo_search=cudnn_conv_algo_search)
        except Exception:
            log.exception("CUDA load failed; falling back to CPU int8")
        else:
            if transcriber.provider == CUDA:
                return transcriber
            log.warning("CUDA requested but ONNX Runtime chose %s; falling back to CPU int8", transcriber.provider)
            del transcriber
    return _load(model_dir, cuda=False)


def _load(model_dir: Path, *, cuda: bool, cudnn_conv_algo_search: str | None = None) -> Transcriber:
    t0 = time.perf_counter()
    options = None
    if cuda:
        onnxruntime.preload_dlls()
        cuda_options = {"cudnn_conv_algo_search": cudnn_conv_algo_search} if cudnn_conv_algo_search else {}
        providers = [(CUDA, cuda_options), "CPUExecutionProvider"]
        quantization = None
        # The GPU runs the graphs and the CPU only does shape arithmetic, so the default pool of a thread per
        # core in every session would only spin after each run.
        options = onnxruntime.SessionOptions()
        options.intra_op_num_threads = 1
    else:
        providers = ["CPUExecutionProvider"]
        quantization = "int8"
    model = LazyResamplerManager(options, providers).create_asr(MODEL_NAME, model_dir, quantization=quantization)
    transcriber = Transcriber(model, model.asr._encoder.get_providers()[0], quantization)
    log.info("loaded %s on %s in %.1f s", MODEL_NAME, transcriber.label, time.perf_counter() - t0)
    return transcriber
