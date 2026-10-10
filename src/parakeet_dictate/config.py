import tomllib
from dataclasses import dataclass, fields
from pathlib import Path
from typing import Literal

from pynput.keyboard import Key

ROOT = Path(__file__).resolve().parents[2]
if not (ROOT / "pyproject.toml").is_file():
    raise ImportError(f"parakeet_dictate must run from its checkout, installed editable by `uv sync`; got {ROOT}")
CONFIG_PATH = ROOT / "config.toml"
LOG_PATH = ROOT / "dictate.log"


class ConfigError(ValueError):
    pass


@dataclass(frozen=True)
class Config:
    hotkey: str = "ctrl_r"
    tap_threshold_ms: int = 300
    provider: Literal["cuda", "cpu"] = "cuda"
    sounds: bool = True
    append_space: bool = True
    model_dir: Path = ROOT / "models" / "parakeet-tdt-0.6b-v3"

    @property
    def hotkey_vk(self) -> int:
        return Key[self.hotkey].value.vk


def load_config(path: Path = CONFIG_PATH) -> Config:
    if not path.exists():
        return Config()
    try:
        raw = tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as e:
        raise ConfigError(f"{path}: {e}") from e

    unknown = raw.keys() - {f.name for f in fields(Config)}
    if unknown:
        raise ConfigError(f"{path}: unknown setting(s) {sorted(unknown)}")

    values = {name: _parse(name, value) for name, value in raw.items()}
    return Config(**values)


def _parse(name: str, value: object) -> object:
    match name:
        case "hotkey":
            if not isinstance(value, str) or value not in Key.__members__:
                raise ConfigError(f"hotkey must be a pynput Key name such as 'ctrl_r', 'f13', 'pause'; got {value!r}")
            if value == "esc":
                raise ConfigError("hotkey cannot be 'esc'; Esc cancels a recording")
            # The keyboard hook reports sided modifiers only, so a generic one would never match.
            if value in ("ctrl", "shift", "alt"):
                raise ConfigError(f"hotkey must name a side, like '{value}_r'; got {value!r}")
            return value
        case "tap_threshold_ms":
            if type(value) is not int or not 0 < value <= 2000:
                raise ConfigError(f"tap_threshold_ms must be an integer in 1..2000; got {value!r}")
            return value
        case "provider":
            if value not in ("cuda", "cpu"):
                raise ConfigError(f"provider must be 'cuda' or 'cpu'; got {value!r}")
            return value
        case "sounds" | "append_space":
            if type(value) is not bool:
                raise ConfigError(f"{name} must be true or false; got {value!r}")
            return value
        case "model_dir":
            if not isinstance(value, str):
                raise ConfigError(f"model_dir must be a path string; got {value!r}")
            return ROOT / value
    raise AssertionError(name)
