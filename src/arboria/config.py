"""Reading the project's arboria.toml (default machine, budget)."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from typing import Any

from .errors import ArbError
from .paths import RepoContext


@dataclass(frozen=True)
class ProjectConfig:
    default_machine: str
    total_su: float  # 0 = no budget set


def load_toml(path, what: str) -> dict[str, Any]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as exc:
        raise ArbError(f"{path} is not valid TOML ({exc}); fix the {what} first") from exc


def project_config(ctx: RepoContext) -> ProjectConfig:
    path = ctx.config_path
    cfg = load_toml(path, "project config") if path.is_file() else {}
    machine = cfg.get("default_machine", "")
    if not isinstance(machine, str):
        raise ArbError(f"{path}: default_machine must be a string")
    budget = cfg.get("budget", {})
    if not isinstance(budget, dict):
        raise ArbError(f"{path}: [budget] must be a table")
    total = budget.get("total_su", 0)
    if isinstance(total, bool) or not isinstance(total, (int, float)) or total < 0:
        raise ArbError(f"{path}: [budget] total_su must be a number >= 0")
    return ProjectConfig(default_machine=machine.strip(), total_su=float(total))
