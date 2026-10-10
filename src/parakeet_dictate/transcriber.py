import logging
import re
import time
from pathlib import Path
from typing import get_args

import numpy as np
import numpy.typing as npt
import onnx_asr
import onnxruntime
from onnx_asr.adapters import TextResultsAsrAdapter
from onnx_asr.utils import SampleRates

from .recorder import Audio

log = logging.getLogger(__name__)

MODEL_NAME = "nemo-parakeet-tdt-0.6b-v3"
CUDA = "CUDAExecutionProvider"
MIN_SECONDS = 0.1
WARMUP_SECONDS = (1.0, 4.0)


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


def resample(samples: npt.NDArray[np.float32], rate: int, target: int) -> npt.NDArray[np.float32]:
    n_out = round(len(samples) * target / rate)
    spectrum = np.fft.rfft(samples)[: n_out // 2 + 1]
    return (np.fft.irfft(spectrum, n_out) * (n_out / len(samples))).astype(np.float32)


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
    if cuda:
        onnxruntime.preload_dlls()
        cuda_options = {"cudnn_conv_algo_search": cudnn_conv_algo_search} if cudnn_conv_algo_search else {}
        providers = [(CUDA, cuda_options), "CPUExecutionProvider"]
        quantization = None
    else:
        providers = ["CPUExecutionProvider"]
        quantization = "int8"
    model = onnx_asr.load_model(MODEL_NAME, model_dir, quantization=quantization, providers=providers)
    transcriber = Transcriber(model, model.asr._encoder.get_providers()[0], quantization)
    log.info("loaded %s on %s in %.1f s", MODEL_NAME, transcriber.label, time.perf_counter() - t0)
    return transcriber
