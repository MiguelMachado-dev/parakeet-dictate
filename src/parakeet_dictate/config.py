import ipaddress
import os
import tomllib
from dataclasses import dataclass, fields
from pathlib import Path
from typing import Literal

from pynput.keyboard import Key

from .app_rules import AppRule
from .styles import DEFAULT_STYLE, STYLES

ROOT = Path(__file__).resolve().parents[2]
if not (ROOT / "pyproject.toml").is_file():
    raise ImportError(f"parakeet_dictate must run from its checkout, installed editable by `uv sync`; got {ROOT}")
CONFIG_PATH = ROOT / "config.toml"
LOG_PATH = ROOT / "dictate.log"
ENV_PATH = ROOT / ".env"
TRAY_CHOICES_PATH = ROOT / "tray_choices.json"
SERVER_TOKEN_PATH = ROOT / "server_token.txt"
OVERLAY_CHOICES = (*STYLES, "off")
HOTKEY_LABELS = {
    "ctrl_l": "Left Ctrl",
    "ctrl_r": "Right Ctrl",
    "shift_l": "Left Shift",
    "shift_r": "Right Shift",
    "alt_l": "Left Alt",
    "alt_r": "Right Alt",
    "alt_gr": "AltGr",
    "cmd_l": "Left Windows",
    "cmd_r": "Right Windows",
}


class ConfigError(ValueError):
    pass


@dataclass(frozen=True)
class PostprocessConfig:
    enabled: bool = True
    provider: Literal["zai", "anthropic"] = "zai"
    model: str = ""
    endpoint: str = ""
    reasoning_effort: Literal["low", "high", "max"] = "low"
    thinking: bool = False
    effort: Literal["low", "medium", "high", "xhigh", "max"] = "low"
    timeout_s: float = 8.0
    extra_instructions: str = ""

    def __post_init__(self) -> None:
        anthropic = self.provider == "anthropic"
        if not self.model:
            object.__setattr__(self, "model", "claude-haiku-5-5" if anthropic else "glm-5.3")
        if not self.endpoint:
            object.__setattr__(self, "endpoint", "https://api.anthropic.com/v1/messages" if anthropic else
                               "https://api.z.ai/api/coding/paas/v4/chat/completions")
        if anthropic and not self.thinking and self.effort in ("xhigh", "max"):
            raise ConfigError("postprocess.effort above 'high' requires thinking = true for Anthropic")

    @property
    def label(self) -> str:
        return "Claude" if self.provider == "anthropic" else "GLM"

    @property
    def api_key_name(self) -> str:
        return "ANTHROPIC_API_KEY" if self.provider == "anthropic" else "ZAI_API_KEY"


@dataclass(frozen=True)
class ServerConfig:
    enabled: bool = False
    port: int = 8765
    bind: str = "0.0.0.0"


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
    server: ServerConfig = ServerConfig()
    app_rules: tuple[AppRule, ...] = ()

    @property
    def hotkey_vk(self) -> int:
        return Key[self.hotkey].value.vk

    @property
    def hotkey_label(self) -> str:
        """The hotkey as people name it: "Right Ctrl", "F13", "Scroll Lock"."""
        return HOTKEY_LABELS.get(self.hotkey, self.hotkey.replace("_", " ").title())


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


def load_api_key(env_path: Path = ENV_PATH, *, name: str = "ZAI_API_KEY") -> str | None:
    if key := os.environ.get(name, "").strip():
        return key
    if not env_path.exists():
        return None
    for line in env_path.read_text(encoding="utf-8-sig").splitlines():
        entry, _, value = line.partition("=")
        if entry.strip() == name:
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
        case "server":
            if not isinstance(value, dict):
                raise ConfigError(f"server must be a table, [server]; got {value!r}")
            unknown = value.keys() - {f.name for f in fields(ServerConfig)}
            if unknown:
                raise ConfigError(f"unknown server setting(s) {sorted(unknown)}")
            return ServerConfig(**{key: _parse_server(key, item) for key, item in value.items()})
        case "app_rules":
            if not isinstance(value, list):
                raise ConfigError("app_rules must be an array of tables, [[app_rules]]")
            return tuple(_parse_app_rule(rule, index) for index, rule in enumerate(value, 1))
    raise AssertionError(name)


def _parse_app_rule(value: object, index: int) -> AppRule:
    label = f"app_rules[{index}]"
    if not isinstance(value, dict):
        raise ConfigError(f"{label} must be a table")
    unknown = value.keys() - {f.name for f in fields(AppRule)}
    if unknown:
        raise ConfigError(f"unknown {label} setting(s) {sorted(unknown)}")
    name, instructions, apps, enabled = (value.get("name"), value.get("instructions"), value.get("apps", []), value.get("enabled", True))
    if not isinstance(name, str) or not name.strip():
        raise ConfigError(f"{label}.name must be a non-empty string")
    if not isinstance(instructions, str):
        raise ConfigError(f"{label}.instructions must be a string")
    if type(enabled) is not bool:
        raise ConfigError(f"{label}.enabled must be true or false")
    if not isinstance(apps, list) or any(
        not isinstance(app, str) or app != app.strip() or len(app) <= 4
        or not app.casefold().endswith(".exe") or any(c in app for c in '\\/:*?"<>|\0') for app in apps
    ):
        raise ConfigError(f"{label}.apps must be executable names such as ['Slack.exe'], without paths or wildcards")
    return AppRule(name=name, instructions=instructions, apps=tuple(apps), enabled=enabled)


def _parse_server(name: str, value: object) -> object:
    match name:
        case "enabled":
            if type(value) is not bool:
                raise ConfigError(f"server.enabled must be true or false; got {value!r}")
            return value
        case "port":
            if type(value) is not int or not 1 <= value <= 65535:
                raise ConfigError(f"server.port must be an integer in 1..65535; got {value!r}")
            return value
        case "bind":
            try:
                # IPv4Address also takes an integer, which is no address to bind.
                ipaddress.IPv4Address(value if isinstance(value, str) else "")
            except ValueError:
                raise ConfigError(f"server.bind must be an IPv4 address such as '0.0.0.0'; got {value!r}") from None
            return value
    raise AssertionError(name)


def _parse_postprocess(name: str, value: object) -> object:
    match name:
        case "enabled" | "thinking":
            if type(value) is not bool:
                raise ConfigError(f"postprocess.{name} must be true or false; got {value!r}")
            return value
        case "provider":
            if value not in ("zai", "anthropic"):
                raise ConfigError(f"postprocess.provider must be 'zai' or 'anthropic'; got {value!r}")
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
        case "effort":
            if value not in ("low", "medium", "high", "xhigh", "max"):
                raise ConfigError(f"postprocess.effort must be 'low', 'medium', 'high', 'xhigh', or 'max'; got {value!r}")
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
