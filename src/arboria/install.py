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
# `arb`; sessions/ holds the inbox sockets the notebook posts to. Other files under
# .arboria/ stay writable, notably commands/ (the notebook commands Claude writes).
PROTECTED_PATHS = ["**/.arboria/notebooks/**", "**/.arboria/ledger.db*", "**/.arboria/backup/**",
                   "**/.arboria/*.toml", "**/.arboria/sessions/**", "**/notebook.py"]
EDIT_TOOLS = ["Edit", "Write", "MultiEdit", "NotebookEdit"]
FILE_RULES = [f"{tool}({path})" for path in PROTECTED_PATHS for tool in EDIT_TOOLS]
DENY_RULES = [*CLUSTER_RULES, *APPROVAL_RULES, *FILE_RULES]
# The seed home (`arb install-claude --home`): the seed notebooks and the whole state dir
# are the human's (its commands are the global ones, outside the home), and so are the
# seed commands that write (they also refuse inside Claude Code).
SEED_RULES = ["Bash(arb seed new:*)", "Bash(arb seed set:*)", "Bash(arb seed drop:*)",
              "Bash(arb seed plant:*)"]
HOME_PROTECTED_PATHS = ["**/seeds/**", "**/.arboria/**"]
HOME_DENY_RULES = [*CLUSTER_RULES, *APPROVAL_RULES, *SEED_RULES,
                   *(f"{tool}({path})" for path in HOME_PROTECTED_PATHS for tool in EDIT_TOOLS)]
SETTINGS_NAME = "settings.json"
LOCAL_NAME = "settings.local.json"  # per machine, not committed
LOCAL_EXCLUDE = f"/.claude/{LOCAL_NAME}"
LOCAL_COMMENT = "arboria: Claude Code settings of this machine (install-claude)"


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


def merge_settings(settings: dict[str, Any], where: str = "settings.json",
                   add_dirs: list[str] | None = None,
                   deny_rules: list[str] | None = None) -> list[str]:
    """Add our entries to `settings` in place; return a list of what changed.

    `add_dirs` go into `permissions.additionalDirectories` (the global notebook-command
    library lives outside the project, see commands.global_dir). In a project these are
    written into settings.local.json instead (see `install`)."""
    changes: list[str] = []
    hooks = _section(settings, "hooks", dict, where)
    session_start = _section(hooks, "SessionStart", list, where)
    if not _has_hook(session_start):
        session_start.append({"hooks": [{"type": "command", "command": HOOK_COMMAND}]})
        changes.append(f"hooks.SessionStart += {HOOK_COMMAND}")
    perms = _section(settings, "permissions", dict, where)
    deny = _section(perms, "deny", list, where)
    for rule in DENY_RULES if deny_rules is None else deny_rules:
        if rule not in deny:
            deny.append(rule)
            changes.append(f"permissions.deny += {rule}")
    if add_dirs:
        changes += add_directories(settings, add_dirs, where)
    ask = perms.get("ask")
    if isinstance(ask, list):  # moved from ask to deny
        for rule in APPROVAL_RULES:
            while rule in ask:
                ask.remove(rule)
                changes.append(f"permissions.ask -= {rule}")
    return changes


def add_directories(settings: dict[str, Any], add_dirs: list[str],
                    where: str = "settings.json") -> list[str]:
    """Add `add_dirs` to `permissions.additionalDirectories` in place."""
    perms = _section(settings, "permissions", dict, where)
    dirs = _section(perms, "additionalDirectories", list, where)
    changes = []
    for d in add_dirs:
        if d not in dirs:
            dirs.append(d)
            changes.append(f"permissions.additionalDirectories += {d}")
    return changes


def _is_commands_dir(entry: Any, ours: list[str]) -> bool:
    """An entry an earlier `install-claude` wrote: our dir, or someone's
    `.../arboria/commands` (machine-specific, it never belonged in a committed file)."""
    if not isinstance(entry, str):
        return False
    return entry in ours or tuple(Path(entry).parts[-2:]) == ("arboria", "commands")


def remove_directories(settings: dict[str, Any], ours: list[str],
                       where: str = "settings.json") -> list[str]:
    """Take the global commands dir(s) out of a portable settings.json, in place."""
    perms = settings.get("permissions")
    if not isinstance(perms, dict) or not isinstance(perms.get("additionalDirectories"), list):
        return []
    dirs = perms["additionalDirectories"]
    gone = [d for d in dirs if _is_commands_dir(d, ours)]
    if not gone:
        return []
    perms["additionalDirectories"] = [d for d in dirs if d not in gone]
    if not perms["additionalDirectories"]:
        del perms["additionalDirectories"]
    return [f"permissions.additionalDirectories -= {d} (machine-specific; now in "
            f"{LOCAL_NAME})" for d in gone]


def default_add_dirs() -> list[str]:
    """The global commands dir (created, so Claude Code can add it as a working dir)."""
    from .commands import global_dir

    path = global_dir().expanduser().absolute()
    try:
        path.mkdir(parents=True, exist_ok=True)
    except OSError:
        pass
    return [str(path)]


def _load(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    try:
        settings = json.loads(path.read_text(encoding="utf-8") or "{}")
    except ValueError as exc:
        raise ArbError(f"{path} is not valid JSON ({exc}); fix it first") from exc
    if not isinstance(settings, dict):
        raise ArbError(f"{path} must contain a JSON object")
    return settings


def _save(path: Path, settings: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(settings, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def install(root: Path, add_dirs: list[str] | None = None, *,
            local: bool = True, deny_rules: list[str] | None = None) -> tuple[Path, list[str]]:
    """Merge into `<root>/.claude/settings.json` (created if missing).

    The global commands dir is an absolute path on this machine, so with `local` (a
    project) it goes into `.claude/settings.local.json`, which is not committed; an entry
    an older arb wrote into settings.json is moved there. Without `local` (`--user`,
    `~/.claude/settings.json` is already personal; `--home`, the home is no repo) it stays
    in settings.json."""
    if add_dirs is None:
        add_dirs = default_add_dirs()
    path = Path(root) / ".claude" / SETTINGS_NAME
    settings = _load(path)
    local_path = path.with_name(LOCAL_NAME)
    local_settings = _load(local_path) if local else {}
    changes = merge_settings(settings, str(path), None if local else add_dirs, deny_rules)
    local_changes: list[str] = []
    if local:
        changes += remove_directories(settings, add_dirs, str(path))
        local_changes = add_directories(local_settings, add_dirs, str(local_path))
    if changes or not path.exists():
        _save(path, settings)
    if local_changes:
        _save(local_path, local_settings)
        changes += [f"{LOCAL_NAME}: {c}" for c in local_changes]
    return path, changes
