"""Runs the Windows app's server (src/parakeet_dictate/server.py) on the Mac with a stand-in model and GLM,
so the Mac client can be checked against the real request handling without the PC:

    uv run --no-project --python 3.12 --with numpy --with httpx2 --with pillow \
        python mac/scripts/fake_pc_server.py

The stand-in model replies with what it received ("16000 Hz, 6.6 s"); the stand-in GLM upper-cases it.
It prints the two lines of "Copy Mac connection info" and serves until Ctrl+C.
"""

import enum
import logging
import sys
import tempfile
import time
import types
from pathlib import Path
from typing import Literal

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))


def stub(name: str, **attributes: object) -> None:
    module = types.ModuleType(name)
    module.__dict__.update(attributes)
    sys.modules[name] = module


# Windows-only and model packages that server.py imports through config, recorder, and transcriber.
stub("pynput")
stub("pynput.keyboard", Key=enum.Enum("Key", "ctrl_r"))
stub("sounddevice")
stub("onnxruntime")
stub("onnx_asr")
stub("onnx_asr.adapters", TextResultsAsrAdapter=object)
stub("onnx_asr.utils", SampleRates=Literal[16000])

from parakeet_dictate.config import ServerConfig  # noqa: E402
from parakeet_dictate.server import DictationServer  # noqa: E402
from parakeet_dictate.transcriber import SharedModel  # noqa: E402


class EchoModel:
    def transcribe(self, audio) -> str:
        return f"{audio.rate} Hz, {audio.seconds:.1f} s"


class ShoutingGlm:
    def preconnect(self) -> None:
        pass

    def polish(self, raw: str) -> str:
        return raw.upper()


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    model = SharedModel()
    model.load(EchoModel())
    server = DictationServer(
        ServerConfig(bind="127.0.0.1", port=8799), model=model, glm=ShoutingGlm(), polish_enabled=lambda: True,
        timeout_s=2.0, token_path=Path(tempfile.mkdtemp()) / "server_token.txt",
    )
    server.start()
    print(f"http://127.0.0.1:{server.port}\n{server.token}", flush=True)
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        server.stop()


if __name__ == "__main__":
    main()
