"""Merge arboria's hook and permission rules into a Claude Code settings.json."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .errors import ArbError

HOOK_COMMAND = "arb hook session-start"
ASK_RULES = ["Bash(arb approve:*)", "Bash(arb lesson accept:*)"]
DENY_RULES = ["Bash(sbatch:*)", "Bash(ssh:* sbatch*)"]


def _has_hook(entries: list[Any]) -> bool:
    for entry in entries:
        if isinstance(entry, dict):
            for hook in entry.get("hooks") or []:
                if isinstance(hook, dict) and hook.get("command") == HOOK_COMMAND:
                    return True
    return False


def _section(parent: dict[str, Any], key: str, kind: type, where: str) -> Any:
    value = parent.setdefault(key, kind())
    if not isinstance(value, kind):
        raise ArbError(f"{where}: `{key}` must be a {kind.__name__}, found {type(value).__name__}")
    return value


def merge_settings(settings: dict[str, Any], where: str = "settings.json") -> list[str]:
    """Add our entries to `settings` in place; return a list of what changed."""
    changes: list[str] = []
    hooks = _section(settings, "hooks", dict, where)
    session_start = _section(hooks, "SessionStart", list, where)
    if not _has_hook(session_start):
        session_start.append({"hooks": [{"type": "command", "command": HOOK_COMMAND}]})
        changes.append(f"hooks.SessionStart += {HOOK_COMMAND}")
    perms = _section(settings, "permissions", dict, where)
    for key, rules in (("ask", ASK_RULES), ("deny", DENY_RULES)):
        current = _section(perms, key, list, where)
        for rule in rules:
            if rule not in current:
                current.append(rule)
                changes.append(f"permissions.{key} += {rule}")
    return changes


def install(root: Path) -> tuple[Path, list[str]]:
    """Merge into `<root>/.claude/settings.json` (created if missing)."""
    path = Path(root) / ".claude" / "settings.json"
    settings: dict[str, Any] = {}
    if path.exists():
        try:
            settings = json.loads(path.read_text(encoding="utf-8") or "{}")
        except ValueError as exc:
            raise ArbError(f"{path} is not valid JSON ({exc}); fix it first") from exc
        if not isinstance(settings, dict):
            raise ArbError(f"{path} must contain a JSON object")
    changes = merge_settings(settings, str(path))
    if changes or not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(settings, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return path, changes
