"""Merge arboria's hook and permission rules into a Claude Code settings.json."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .errors import ArbError

HOOK_COMMAND = "arb hook session-start"
# Approvals happen only in the human's own terminal (`arb approve` / `arb lesson accept`
# also refuse inside Claude Code); earlier versions put these under "ask".
APPROVAL_RULES = ["Bash(arb approve:*)", "Bash(arb lesson accept:*)"]
# The real guarantee is that only `arb approve` submits (after a human Decision);
# these deny rules are an extra layer that keeps Claude off the cluster directly.
CLUSTER_RULES = ["Bash(sbatch:*)", "Bash(srun:*)", "Bash(salloc:*)", "Bash(scancel:*)",
                 "Bash(ssh:*)", "Bash(scp:*)", "Bash(rsync:*)"]
# Notebooks are the human's (Claude may read them) and the ledger is written only by
# `arb`. Other files under .arboria/ (e.g. a later commands/ dir) stay writable.
PROTECTED_PATHS = ["**/.arboria/notebooks/**", "**/.arboria/ledger.db*", "**/.arboria/backup/**",
                   "**/.arboria/*.toml", "**/notebook.py"]
EDIT_TOOLS = ["Edit", "Write", "MultiEdit", "NotebookEdit"]
FILE_RULES = [f"{tool}({path})" for path in PROTECTED_PATHS for tool in EDIT_TOOLS]
DENY_RULES = [*CLUSTER_RULES, *APPROVAL_RULES, *FILE_RULES]


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
    deny = _section(perms, "deny", list, where)
    for rule in DENY_RULES:
        if rule not in deny:
            deny.append(rule)
            changes.append(f"permissions.deny += {rule}")
    ask = perms.get("ask")
    if isinstance(ask, list):  # moved from ask to deny
        for rule in APPROVAL_RULES:
            while rule in ask:
                ask.remove(rule)
                changes.append(f"permissions.ask -= {rule}")
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
