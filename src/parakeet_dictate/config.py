import os
import tomllib
from dataclasses import dataclass, fields
from pathlib import Path
from typing import Literal

from pynput.keyboard import Key

from .styles import DEFAULT_STYLE, STYLES

ROOT = Path(__file__).resolve().parents[2]
if not (ROOT / "pyproject.toml").is_file():
    raise ImportError(f"parakeet_dictate must run from its checkout, installed editable by `uv sync`; got {ROOT}")
CONFIG_PATH = ROOT / "config.toml"
LOG_PATH = ROOT / "dictate.log"
ENV_PATH = ROOT / ".env"
TRAY_CHOICES_PATH = ROOT / "tray_choices.json"
OVERLAY_CHOICES = (*STYLES, "off")


class ConfigError(ValueError):
    pass


@dataclass(frozen=True)
class PostprocessConfig:
    enabled: bool = True
    model: str = "glm-5.3"
    endpoint: str = "https://api.z.ai/api/coding/paas/v4/chat/completions"
    reasoning_effort: Literal["low", "high", "max"] = "low"
    timeout_s: float = 8.0
    extra_instructions: str = ""


@dataclass(frozen=True)
class Config:
    hotkey: str = "ctrl_r"
    tap_threshold_ms: int = 300
    provider: Literal["cuda", "cpu"] = "cuda"
    sounds: bool = True
    append_space: bool = True
    # One of OVERLAY_CHOICES: a style name from styles.STYLES, or "off".
    overlay: str = DEFAULT_STYLE
    model_dir: Path = ROOT / "models" / "parakeet-tdt-0.6b-v3"
    postprocess: PostprocessConfig = PostprocessConfig()

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


def load_api_key(env_path: Path = ENV_PATH) -> str | None:
    if key := os.environ.get("ZAI_API_KEY", "").strip():
        return key
    if not env_path.exists():
        return None
    for line in env_path.read_text(encoding="utf-8-sig").splitlines():
        name, _, value = line.partition("=")
        if name.strip() == "ZAI_API_KEY":
            return value.strip().strip("\"'") or None
    return None


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
        case "overlay":
            if value not in OVERLAY_CHOICES:
                raise ConfigError(f"overlay must be one of {', '.join(map(repr, OVERLAY_CHOICES))}; got {value!r}")
            return value
        case "sounds" | "append_space":
            if type(value) is not bool:
                raise ConfigError(f"{name} must be true or false; got {value!r}")
            return value
        case "model_dir":
            if not isinstance(value, str):
                raise ConfigError(f"model_dir must be a path string; got {value!r}")
            return ROOT / value
        case "postprocess":
            if not isinstance(value, dict):
                raise ConfigError(f"postprocess must be a table, [postprocess]; got {value!r}")
            unknown = value.keys() - {f.name for f in fields(PostprocessConfig)}
            if unknown:
                raise ConfigError(f"unknown postprocess setting(s) {sorted(unknown)}")
            return PostprocessConfig(**{key: _parse_postprocess(key, item) for key, item in value.items()})
    raise AssertionError(name)


def _parse_postprocess(name: str, value: object) -> object:
    match name:
        case "enabled":
            if type(value) is not bool:
                raise ConfigError(f"postprocess.enabled must be true or false; got {value!r}")
            return value
        case "model" | "endpoint":
            if not isinstance(value, str) or not value.strip():
                raise ConfigError(f"postprocess.{name} must be a non-empty string; got {value!r}")
            if name == "endpoint" and not value.startswith(("https://", "http://")):
                raise ConfigError(f"postprocess.endpoint must be an http(s) URL; got {value!r}")
            return value
        case "reasoning_effort":
            if value not in ("low", "high", "max"):
                raise ConfigError(f"postprocess.reasoning_effort must be 'low', 'high', or 'max'; got {value!r}")
            return value
        case "timeout_s":
            if type(value) not in (int, float) or not 0 < value <= 60:
                raise ConfigError(f"postprocess.timeout_s must be a number of seconds in (0, 60]; got {value!r}")
            return float(value)
        case "extra_instructions":
            if not isinstance(value, str):
                raise ConfigError(f"postprocess.extra_instructions must be a string; got {value!r}")
            return value
    raise AssertionError(name)
