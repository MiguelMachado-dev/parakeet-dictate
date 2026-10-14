import json
import os
from dataclasses import asdict, dataclass, field
from pathlib import Path

import tomlkit

from .config import ROOT, Config, ConfigError, load_api_key, parse_config
from .tray_choices import parse_choices

KEY_NAMES = ("ZAI_API_KEY", "ANTHROPIC_API_KEY")


class SettingsConflict(ConfigError):
    pass


@dataclass
class SettingsSnapshot:
    config: Config
    keys: dict[str, str] = field(repr=False)
    environment_keys: frozenset[str]
    files: dict[str, bytes | None] = field(repr=False)


def config_values(config: Config) -> dict:
    values = asdict(config)
    try:
        values["model_dir"] = str(config.model_dir.relative_to(ROOT))
    except ValueError:
        values["model_dir"] = str(config.model_dir)
    values["app_rules"] = [{**rule, "apps": list(rule["apps"])} for rule in values["app_rules"]]
    return values


class SettingsStore:
    """The window edits the same files as the app, including the choices saved by the tray."""

    def __init__(self, root: Path = ROOT):
        self.root = root

    def _files(self) -> dict[str, bytes | None]:
        result = {}
        for name in ("config.toml", ".env", "tray_choices.json"):
            try:
                result[name] = (self.root / name).read_bytes()
            except FileNotFoundError:
                result[name] = None
        return result

    def load(self) -> SettingsSnapshot:
        files = self._files()
        try:
            config = parse_config(tomlkit.parse((files["config.toml"] or b"").decode("utf-8-sig")).unwrap())
        except (ValueError, tomlkit.exceptions.ParseError) as e:
            raise ConfigError("Could not read config.toml. Check its syntax.") from e
        values = config_values(config)
        for key, value in parse_choices(files["tray_choices.json"]).items():
            if key == "overlay":
                values[key] = value
            elif key in ("postprocess", "server"):
                values[key]["enabled"] = value
        config = parse_config(values)
        environment_keys = frozenset(name for name in KEY_NAMES if os.environ.get(name, "").strip())
        keys = {name: load_api_key(self.root / ".env", name=name) or "" for name in KEY_NAMES}
        return SettingsSnapshot(config, keys, environment_keys, files)

    def save(self, snapshot: SettingsSnapshot, config: Config, keys: dict[str, str]) -> SettingsSnapshot:
        values = config_values(config)
        parse_config(values)
        current = self._files()
        if current != snapshot.files:
            raise SettingsConflict("Settings changed outside this window. Reload before saving so those changes are kept.")
        doc = tomlkit.parse((current["config.toml"] or b"").decode("utf-8-sig"))
        for name, value in values.items():
            if name == "app_rules":
                if value != doc.get(name, []):
                    rules = tomlkit.aot()
                    for rule in value:
                        table = tomlkit.table()
                        table.update(rule)
                        rules.append(table)
                    doc[name] = rules if value else []
            elif isinstance(value, dict):
                if name not in doc:
                    doc[name] = tomlkit.table()
                for key, item in value.items():
                    if doc[name].get(key) != item:
                        doc[name][key] = item
            elif doc.get(name) != value:
                doc[name] = value
        env = (current[".env"] or b"").decode("utf-8-sig")
        original_env = env
        for name in KEY_NAMES:
            value = keys.get(name, "").strip()
            if name in snapshot.environment_keys:
                continue
            if any(c in value for c in '\r\n\0\"\''):
                raise ConfigError(f"{name} must be a single key, without quotes or line breaks.")
            if value != snapshot.keys[name]:
                lines = [line for line in env.splitlines(keepends=True) if line.partition("=")[0].strip() != name]
                env = "".join(lines)
                if env and not env.endswith("\n"):
                    env += "\n"
                if value:
                    env += f"{name}={value}\n"
        choices = parse_choices(current["tray_choices.json"])
        choices.update(overlay=config.overlay, postprocess=config.postprocess.enabled, server=config.server.enabled)
        env_bytes = (current[".env"] or b"") if env == original_env else env.encode("utf-8")
        updated = {"config.toml": tomlkit.dumps(doc).encode("utf-8"), ".env": env_bytes,
                   "tray_choices.json": json.dumps(choices).encode("utf-8")}
        written = []
        try:
            for name, data in updated.items():
                if data == current[name] or (not data and current[name] is None):
                    continue
                self._write(name, data)
                written.append(name)
        except OSError:
            for name in reversed(written):
                if current[name] is None:
                    (self.root / name).unlink(missing_ok=True)
                else:
                    self._write(name, current[name])
            raise
        return self.load()

    def _write(self, name: str, data: bytes) -> None:
        path = self.root / name
        temporary = path.with_name(path.name + ".tmp")
        try:
            temporary.write_bytes(data)
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)
