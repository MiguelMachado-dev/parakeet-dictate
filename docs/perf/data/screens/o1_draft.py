# Draft for transcriber.py (not applied yet)
from importlib.resources import files

import onnx_asr.preprocessors
import onnxruntime
from onnx_asr.loader import Manager
from onnx_asr.onnx import TensorRtOptions
from onnx_asr.preprocessors.resampler import Resampler


class LazyResampler(Resampler):
    """onnx_asr's Resampler opens a session for each of its seven input rates when the model loads. A
    microphone records at one rate, so open each session the first time its rate arrives."""

    def __init__(self, sample_rate, onnx_options):
        self._target_sample_rate = sample_rate
        self._onnx_options = onnx_options
        self._preprocessors = _Sessions(self._open)

    def _open(self, rate: int) -> onnxruntime.InferenceSession:
        model = files(onnx_asr.preprocessors).joinpath("data", f"resample_{rate // 1000}_{self._target_sample_rate // 1000}.onnx")
        return onnxruntime.InferenceSession(
            model.read_bytes(), **TensorRtOptions.add_profile(self._onnx_options, self._preprocessor_shapes)
        )


class _Sessions(dict):
    def __init__(self, open_session):
        super().__init__()
        self._open_session = open_session

    def __missing__(self, rate):
        session = self[rate] = self._open_session(rate)
        return session


class _Manager(Manager):
    def _create_resampler(self, sample_rate):
        return LazyResampler(sample_rate, self.resampler_config)
