import json
import logging
import os
from collections.abc import Callable
from pathlib import Path

from .config import OVERLAY_CHOICES, Config

log = logging.getLogger("parakeet_dictate")

_VALID: dict[str, Callable[[object], bool]] = {
    "postprocess": lambda value: type(value) is bool,
    "overlay": lambda value: isinstance(value, str) and value in OVERLAY_CHOICES,
    "server": lambda value: type(value) is bool,
}


def parse_choices(data: bytes | None, name: str = "tray_choices.json") -> dict[str, object]:
    """Invalid saved choices fall back to config.toml, both in the app and in Settings."""
    if data is None:
        return {}
    try:
        saved = json.loads(data.decode("utf-8-sig"))
    except ValueError:
        saved = None
    if not isinstance(saved, dict):
        log.warning("ignoring %s: expected a JSON object", name)
        return {}
    chosen = {}
    for key, value in saved.items():
        if key in _VALID and _VALID[key](value):
            chosen[key] = value
        else:
            log.warning("ignoring %r in %s: %r", key, name, value)
    return chosen


def read_choices(path: Path) -> dict[str, object]:
    try:
        return parse_choices(path.read_bytes(), path.name)
    except FileNotFoundError:
        return {}


class TrayChoices:
    """Tray choices override config.toml and survive restarts. Unchosen keys use the config."""

    def __init__(self, config: Config, path: Path) -> None:
        self._path = path
        self._config = {"postprocess": config.postprocess.enabled, "overlay": config.overlay, "server": config.server.enabled}
        try:
            self._chosen = read_choices(path)
        except OSError:
            log.warning("could not read %s; using config.toml", path.name, exc_info=True)
            self._chosen = {}

    @property
    def postprocess(self) -> bool:
        return self._chosen.get("postprocess", self._config["postprocess"])

    @property
    def overlay(self) -> str:
        """A style name or 'off'."""
        return self._chosen.get("overlay", self._config["overlay"])

    @property
    def server(self) -> bool:
        return self._chosen.get("server", self._config["server"])

    def choose(self, key: str, value: object) -> None:
        self._chosen[key] = value
        temporary = self._path.with_name(self._path.name + ".tray.tmp")
        try:
            # Settings can save while this process waits for an active dictation to finish.
            # Only persist this menu action; never replay this process's older choices.
            saved = read_choices(self._path)
            saved[key] = value
            temporary.write_text(json.dumps(saved), encoding="utf-8")
            os.replace(temporary, self._path)
        except OSError:
            log.exception("could not save the tray choices")
        finally:
            temporary.unlink(missing_ok=True)
