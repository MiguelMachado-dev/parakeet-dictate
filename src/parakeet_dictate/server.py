"""The server for Mac: an HTTP API that lets another machine dictate through this PC's model and GLM.
docs/mac-client.md is the contract."""

import email.parser
import email.policy
import hmac
import io
import json
import logging
import os
import secrets
import socketserver
import subprocess
import threading
import time
import wave
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

import numpy as np

from .config import ServerConfig
from .polish import GlmClient, PolishError
from .recorder import Audio
from .transcriber import SharedModel

log = logging.getLogger(__name__)

HEALTH_PATH = "/v1/health"
TRANSCRIPTIONS_PATH = "/v1/audio/transcriptions"
# Ten minutes of 48 kHz 16-bit stereo, plus room for the multipart framing.
MAX_BODY_BYTES = 10 * 60 * 48_000 * 2 * 2 + 64 * 1024
EXPECTED_AUDIO = "a WAV file of 16-bit PCM samples, mono or stereo, at any sample rate"
LAN_QUERY = r"""
$up = @(Get-NetAdapter -Physical | Where-Object Status -eq Up).ifIndex
$routes = @(Get-NetRoute -AddressFamily IPv4 -DestinationPrefix 0.0.0.0/0 -ErrorAction SilentlyContinue)
ConvertTo-Json -Compress -InputObject @(Get-NetIPAddress -AddressFamily IPv4 | Where-Object InterfaceIndex -in $up | ForEach-Object {
    $index = $_.InterfaceIndex
    @{ address = $_.IPAddress; route_metric = ($routes | Where-Object ifIndex -eq $index | Measure-Object RouteMetric -Minimum).Minimum }
})
"""


class BadRequest(Exception):
    pass


class DictationServer:
    """Started and stopped from the tray. Each request runs on its own thread; transcriptions still run
    one at a time through the shared model."""

    def __init__(
        self,
        config: ServerConfig,
        *,
        model: SharedModel,
        glm: GlmClient | None,
        polish_enabled: Callable[[], bool],
        timeout_s: float,
        token_path: Path,
    ) -> None:
        self._config = config
        self._model = model
        self._glm = glm
        self._polish_enabled = polish_enabled
        self._timeout_s = timeout_s
        self._token_path = token_path
        self._httpd: _HttpServer | None = None
        self._polisher: ThreadPoolExecutor | None = None
        self.token = ""

    @property
    def running(self) -> bool:
        return self._httpd is not None

    @property
    def port(self) -> int:
        return self._httpd.server_address[1]

    def start(self) -> None:
        """Serve from now on. Raises OSError when the port cannot be bound, and stays stopped."""
        if self._httpd is not None:
            return
        self.token = read_token(self._token_path)
        httpd = _HttpServer((self._config.bind, self._config.port), self)
        # Several workers, so a GLM request still running past its timeout never delays the next one.
        self._polisher = ThreadPoolExecutor(max_workers=4, thread_name_prefix="server-polish")
        # stop() waits for the loop to poll; the default half second would stall the tray's click.
        threading.Thread(target=httpd.serve_forever, args=(0.1,), name="server", daemon=True).start()
        self._httpd = httpd
        log.info("server for Mac listening on %s:%d", self._config.bind, self.port)

    def stop(self) -> None:
        httpd, self._httpd = self._httpd, None
        if httpd is None:
            return
        httpd.shutdown()
        httpd.server_close()
        self._polisher.shutdown(wait=False, cancel_futures=True)
        log.info("server for Mac stopped")

    def connection_info(self) -> str | None:
        """The URL and the token, one per line, or None when this PC has no address on the local network."""
        address = self._config.bind if self._config.bind != "0.0.0.0" else lan_ipv4()
        return f"http://{address}:{self.port}\n{self.token}" if address else None

    def authorized(self, header: str) -> bool:
        return hmac.compare_digest(header.encode(), f"Bearer {self.token}".encode())

    @property
    def ready(self) -> bool:
        return self._model.ready

    def polish_on(self) -> bool:
        return self._glm is not None and self._polish_enabled()

    def health(self) -> dict:
        on = self.polish_on()
        if on:
            self._polisher.submit(self._glm.preconnect)
        return {"status": "ok", "postprocess": on}

    def transcribe(self, audio: Audio, postprocess: bool | None, metrics: dict[str, float]) -> dict:
        t0 = time.perf_counter()
        raw = self._model.transcribe(audio)
        metrics["asr_ms"] = (time.perf_counter() - t0) * 1000
        polish = self.polish_on() if postprocess is None else postprocess and self._glm is not None
        if not raw or not polish:
            return {"text": raw, "raw": raw, "polished": False}
        t0 = time.perf_counter()
        future = self._polisher.submit(self._glm.polish, raw)
        try:
            text = future.result(timeout=self._timeout_s)
        except TimeoutError:
            log.warning("post-processing failed (no reply within %g s); returning the raw transcript", self._timeout_s)
            text = None
        except PolishError as e:
            log.warning("post-processing failed (%s); returning the raw transcript", e)
            text = None
        except Exception:
            log.exception("post-processing crashed; returning the raw transcript")
            text = None
        metrics["polish_ms"] = (time.perf_counter() - t0) * 1000
        return {"text": text or raw, "raw": raw, "polished": text is not None}


class _HttpServer(ThreadingHTTPServer):
    # HTTPServer sets SO_REUSEADDR. On Windows two sockets that both set it share the port, so a port held
    # by another such server would bind without error and the two would split the connections.
    allow_reuse_address = False
    daemon_threads = True

    def __init__(self, address: tuple[str, int], app: DictationServer) -> None:
        self.app = app
        super().__init__(address, _Handler)

    def server_bind(self) -> None:
        # HTTPServer.server_bind resolves the host's FQDN, which can stall on a reverse DNS lookup.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


class _Handler(BaseHTTPRequestHandler):
    # HTTP/1.1 answers "Expect: 100-continue", which curl sends for bodies over 1 MB; every reply still
    # closes the connection, so no idle keep-alive connection outlives a stop from the tray.
    protocol_version = "HTTP/1.1"
    server: _HttpServer

    def do_GET(self) -> None:
        app = self.server.app
        if not app.authorized(self.headers.get("Authorization", "")):
            self._error(401, "missing or wrong bearer token")
        elif self._path() != HEALTH_PATH:
            self._error(404, f"no such endpoint; GET {HEALTH_PATH} or POST {TRANSCRIPTIONS_PATH}")
        else:
            self._reply(200, app.health())

    def do_POST(self) -> None:
        app = self.server.app
        try:
            length = int(self.headers["Content-Length"])
        except (TypeError, ValueError):
            self.close_connection = True
            return self._error(411, "send a Content-Length header")
        if length > MAX_BODY_BYTES:
            self.close_connection = True
            return self._error(413, f"the body is over {MAX_BODY_BYTES} bytes, about ten minutes of 48 kHz stereo")
        # Read the body even to refuse it: closing a socket with unread data resets the connection on
        # Windows, and the client would see the reset instead of the status.
        body = self.rfile.read(length)
        if not app.authorized(self.headers.get("Authorization", "")):
            return self._error(401, "missing or wrong bearer token")
        if self._path() != TRANSCRIPTIONS_PATH:
            return self._error(404, f"no such endpoint; GET {HEALTH_PATH} or POST {TRANSCRIPTIONS_PATH}")
        if not app.ready:
            return self._error(503, "the model is still loading; try again in a few seconds")
        try:
            fields = parse_form(self.headers.get("Content-Type", ""), body)
            if "file" not in fields:
                raise BadRequest("the form has no field named file")
            audio = parse_wav(fields["file"])
            postprocess = parse_flag(fields.get("postprocess"))
        except BadRequest as e:
            return self._error(400, str(e))
        metrics = {"audio_s": audio.seconds}
        self._reply(200, app.transcribe(audio, postprocess, metrics), metrics)

    def _path(self) -> str:
        return urlsplit(self.path).path

    def _error(self, status: int, message: str) -> None:
        self._reply(status, {"error": {"message": message}})

    def _reply(self, status: int, payload: dict, metrics: dict[str, float] | None = None) -> None:
        body = json.dumps(payload).encode()
        self.close_connection = True
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        metrics = metrics or {}
        shown = lambda key, digits: f"{metrics[key]:.{digits}f}" if key in metrics else "-"
        log.info(
            "%s %s -> %d, audio %s s, ASR %s ms, polish %s ms",
            self.command, self._path(), status, shown("audio_s", 1), shown("asr_ms", 0), shown("polish_ms", 0),
        )

    def log_request(self, code: object = "-", size: object = "-") -> None:
        pass

    def log_message(self, format: str, *args: object) -> None:
        log.warning("server for Mac: " + format, *args)


def parse_form(content_type: str, body: bytes) -> dict[str, bytes]:
    head = f"Content-Type: {content_type}\r\n\r\n".encode("latin-1")
    message = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(head + body)
    if message.get_content_type() != "multipart/form-data" or not message.is_multipart():
        raise BadRequest(f"send multipart/form-data with the audio in a field named file, {EXPECTED_AUDIO}")
    return {
        part.get_param("name", header="content-disposition"): part.get_payload(decode=True)
        for part in message.iter_parts()
    }


def parse_wav(data: bytes) -> Audio:
    try:
        with wave.open(io.BytesIO(data)) as wav:
            width, channels, rate = wav.getsampwidth(), wav.getnchannels(), wav.getframerate()
            frames = wav.readframes(wav.getnframes())
    except (wave.Error, EOFError) as e:
        raise BadRequest(f"the file is not {EXPECTED_AUDIO} ({e})") from None
    if width != 2:
        raise BadRequest(f"the file has {8 * width}-bit samples; send {EXPECTED_AUDIO}")
    if rate <= 0:
        raise BadRequest(f"the file has a sample rate of {rate}; send {EXPECTED_AUDIO}")
    samples = np.frombuffer(frames, "<i2")
    samples = samples[: len(samples) - len(samples) % channels].reshape(-1, channels)
    return Audio((samples.mean(axis=1) / 32768).astype(np.float32), rate)


def parse_flag(value: bytes | None) -> bool | None:
    match value:
        case None:
            return None
        case b"true":
            return True
        case b"false":
            return False
    raise BadRequest(f"postprocess must be true or false; got {value[:20]!r}")


def read_token(path: Path) -> str:
    try:
        token = path.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        token = ""
    if not token:
        token = secrets.token_urlsafe(32)
        temporary = path.with_name(path.name + ".tmp")
        temporary.write_text(token, encoding="utf-8")
        os.replace(temporary, path)
        log.info("created a new token in %s", path.name)
    return token


def lan_ipv4() -> str | None:
    """This PC's IPv4 address on the local network. Only physical adapters count, which leaves out VPN
    tunnels such as NordLynx; among those, the one with the default route wins."""
    try:
        result = subprocess.run(
            ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", LAN_QUERY],
            capture_output=True, text=True, timeout=20, creationflags=subprocess.CREATE_NO_WINDOW,
        )
        candidates = json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, ValueError):
        log.exception("could not list the network adapters")
        return None
    usable = [c for c in candidates if not c["address"].startswith("169.254.")]
    usable.sort(key=lambda c: (c["route_metric"] is None, c["route_metric"] or 0))
    return usable[0]["address"] if usable else None
