from dataclasses import dataclass, field


@dataclass(frozen=True)
class AppRule:
    name: str
    instructions: str = field(repr=False)
    apps: tuple[str, ...] = ()
    enabled: bool = True


def combined_instructions(rules: tuple[AppRule, ...], executable: str | None) -> str:
    """Enabled, non-empty rules for this executable, in config order."""
    if executable is None:
        return ""
    return "\n\n".join(
        rule.instructions.strip() for rule in rules
        if rule.enabled and rule.instructions.strip()
        and any(app.casefold() == executable.casefold() for app in rule.apps)
    )
