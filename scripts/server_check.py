"""Run the server for Mac on 127.0.0.1 with the real model and post the TTS fixtures through curl.exe.

Checks the raw transcript, then the selected post-processor when its key is found, then a 44.1 kHz stereo copy
of the English fixture. Prints timings and the server's own log lines; never the key or the token.
"""

import argparse
import json
import logging
import subprocess
import sys
import tempfile
import time
import wave
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_model import word_error_rate, words  # noqa: E402

from parakeet_dictate.config import ENV_PATH, Config, ServerConfig, load_api_key, load_config  # noqa: E402
from parakeet_dictate.polish import create_polisher  # noqa: E402
from parakeet_dictate.server import DictationServer  # noqa: E402
from parakeet_dictate.transcriber import SharedModel, load_transcriber, resample  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests" / "fixtures"
MAX_WER = 0.15


def stereo_44k(source: Path, target: Path) -> None:
    with wave.open(str(source)) as f:
        rate = f.getframerate()
        mono = np.frombuffer(f.readframes(f.getnframes()), "<i2").astype(np.float32)
    up = resample(mono, rate, 44_100)
    # Unequal channels, so a server that dropped a channel instead of averaging would hear a quieter copy.
    stereo = np.column_stack([up * 0.9, up * 0.5]).clip(-32768, 32767).astype("<i2")
    with wave.open(str(target), "wb") as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(44_100)
        f.writeframes(stereo.tobytes())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--provider", choices=["cpu", "cuda"], default="cpu")
    parser.add_argument("--model-dir", type=Path, default=Config().model_dir)
    parser.add_argument("--env-file", type=Path, default=ENV_PATH)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="  log: %(message)s", stream=sys.stdout)
    logging.getLogger("httpx2").setLevel(logging.WARNING)

    t0 = time.perf_counter()
    transcriber = load_transcriber(args.model_dir, args.provider)
    transcriber.warm_up(16_000)
    print(f"model on {transcriber.label}, loaded and warmed in {time.perf_counter() - t0:.1f} s")
    model = SharedModel()
    model.load(transcriber)
    config = load_config()
    api_key = load_api_key(args.env_file, name=config.postprocess.api_key_name)
    print(f"{config.postprocess.api_key_name} {'found' if api_key else 'not found; skipping post-processing'}")
    polisher = create_polisher(config.postprocess, api_key) if api_key else None

    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        server = DictationServer(
            ServerConfig(bind="127.0.0.1", port=0), model=model, polisher=polisher, polish_enabled=lambda: True,
            timeout_s=config.postprocess.timeout_s, token_path=scratch / "server_token.txt",
        )
        server.start()
        # A header file keeps the token out of curl's command line.
        headers = scratch / "headers.txt"
        headers.write_text(f"Authorization: Bearer {server.token}\n", encoding="utf-8")
        base = f"http://127.0.0.1:{server.port}"

        def curl(*arguments: str) -> tuple[int, dict, float]:
            t = time.perf_counter()
            result = subprocess.run(
                ["curl.exe", "-sS", "-H", f"@{headers}", "-w", "\n%{http_code}", *arguments],
                capture_output=True, text=True, encoding="utf-8", check=True,
            )
            body, status = result.stdout.rsplit("\n", 1)
            return int(status), json.loads(body), (time.perf_counter() - t) * 1000

        stereo = scratch / "en-44k-stereo.wav"
        stereo_44k(FIXTURES / "en.wav", stereo)
        cases = [(f"{name} raw", FIXTURES / f"{name}.wav", FIXTURES / f"{name}.txt", "false") for name in ("en", "pt")]
        if polisher is not None:
            cases += [(f"{name} {config.postprocess.label}", FIXTURES / f"{name}.wav", FIXTURES / f"{name}.txt", "true") for name in ("en", "pt")]
        cases.append(("en 44.1 kHz stereo raw", stereo, FIXTURES / "en.txt", "false"))

        ok = True
        try:
            status, reply, ms = curl(f"{base}/v1/health")
            print(f"[health] {status} {reply} in {ms:.0f} ms")
            ok &= status == 200 and reply["status"] == "ok"
            for label, audio, expected, postprocess in cases:
                status, reply, ms = curl("-F", f"file=@{audio};type=audio/wav", "-F", f"postprocess={postprocess}",
                                         f"{base}/v1/audio/transcriptions")
                want = expected.read_text(encoding="utf-8").strip()
                wer = word_error_rate(words(want), words(reply.get("text", "")))
                passed = status == 200 and wer <= MAX_WER and reply["polished"] == (postprocess == "true")
                ok &= passed
                print(f"[{label}] {'PASS' if passed else 'FAIL'} {status} in {ms:.0f} ms, WER {wer:.2f}, "
                      f"polished {reply.get('polished')}\n  want: {want}\n  raw:  {reply.get('raw')}\n  text: {reply.get('text')}")
        finally:
            server.stop()
            if polisher is not None:
                polisher.close()
    print("PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
