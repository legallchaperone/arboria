"""Reading the project's arboria.toml (project id, default machine, budget, watch)."""

from __future__ import annotations

import hashlib
import re
import shlex
import tomllib
from dataclasses import dataclass
from typing import Any

from .errors import ArbError
from .paths import RepoContext
from .slurm import PROJECT_RE as PROJECT_ID_RE
_SLUG_MAX = 26  # + "-" + 4 hex chars = 31, the longest id PROJECT_ID_RE allows

DEFAULT_OBSERVATION_MINUTES = 10
DEFAULT_FAIL_PATTERNS = (
    r"(?i)\bloss\b[^\n]{0,40}\bnan\b",
    r"(?i)\bnan\b[^\n]{0,40}\bloss\b",
    r"CUDA out of memory",
    r"torch\.OutOfMemoryError",
)


@dataclass(frozen=True)
class WatchConfig:
    observation_minutes: int
    fail_patterns: tuple[re.Pattern[str], ...]


OVERRIDE_STYLES = ("hydra", "argparse", "tyro")
DEFAULT_RUN_COMMAND = "python train.py"
DEFAULT_OVERRIDE_STYLE = "hydra"
_WANDB_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


@dataclass(frozen=True)
class WandbConfig:
    entity: str  # "" = not configured
    project: str

    @property
    def configured(self) -> bool:
        return bool(self.entity and self.project)

    def run_url(self, wandb_id: str) -> str | None:
        if not self.configured:
            return None
        return f"https://wandb.ai/{self.entity}/{self.project}/runs/{wandb_id}"


@dataclass(frozen=True)
class RunConfig:
    command: tuple[str, ...]  # () = not configured
    override_style: str


@dataclass(frozen=True)
class ClaudeConfig:
    push: bool = True  # tell the running Claude Code session about new command todos


@dataclass(frozen=True)
class ProjectConfig:
    default_machine: str
    total_su: float  # 0 = no budget set
    project_id: str | None  # None = arboria.toml predates project ids (run `arb init`)
    watch: WatchConfig
    wandb: WandbConfig = WandbConfig("", "")
    run: RunConfig = RunConfig((), DEFAULT_OVERRIDE_STYLE)
    claude: ClaudeConfig = ClaudeConfig()


def load_toml(path, what: str) -> dict[str, Any]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as exc:
        raise ArbError(f"{path} is not valid TOML ({exc}); fix the {what} first") from exc


def validate_project_id(value: Any, where: str = "project id") -> str:
    if not isinstance(value, str) or not PROJECT_ID_RE.match(value):
        raise ArbError(f"{where} must match {PROJECT_ID_RE.pattern} "
                       f"(lowercase letters, digits, '-', at most 31 chars); got {value!r}")
    return value


def derive_project_id(ctx: RepoContext) -> str:
    """Slug of the main worktree's folder name + 4 hex chars of sha256(absolute main path)."""
    main = ctx.main.resolve()
    slug = re.sub(r"[^a-z0-9]+", "-", main.name.lower()).strip("-")[:_SLUG_MAX].rstrip("-")
    digest = hashlib.sha256(str(main).encode("utf-8")).hexdigest()[:4]
    return validate_project_id(f"{slug or 'proj'}-{digest}")


def _project_id(cfg: dict[str, Any], path) -> str | None:
    if "project" not in cfg:
        return None
    table = cfg["project"]
    if not isinstance(table, dict):
        raise ArbError(f"{path}: [project] must be a table")
    if "id" not in table:
        return None
    return validate_project_id(table["id"], f"{path}: [project] id")


def _watch(cfg: dict[str, Any], path) -> WatchConfig:
    table = cfg.get("watch", {})
    if not isinstance(table, dict):
        raise ArbError(f"{path}: [watch] must be a table")
    minutes = table.get("observation_minutes", DEFAULT_OBSERVATION_MINUTES)
    if isinstance(minutes, bool) or not isinstance(minutes, int) or minutes < 1:
        raise ArbError(f"{path}: [watch] observation_minutes must be an integer >= 1")
    raw = table.get("fail_patterns", list(DEFAULT_FAIL_PATTERNS))
    if not isinstance(raw, list):
        raise ArbError(f"{path}: [watch] fail_patterns must be a list of regular expressions")
    patterns = []
    for i, p in enumerate(raw):
        if not isinstance(p, str) or not p:
            raise ArbError(f"{path}: [watch] fail_patterns[{i}] must be a non-empty string")
        try:
            patterns.append(re.compile(p))
        except re.error as exc:
            raise ArbError(f"{path}: [watch] fail_patterns[{i}] {p!r} is not a valid "
                           f"regular expression: {exc}") from exc
    return WatchConfig(observation_minutes=minutes, fail_patterns=tuple(patterns))


def _table(cfg: dict[str, Any], name: str, path) -> dict[str, Any]:
    table = cfg.get(name, {})
    if not isinstance(table, dict):
        raise ArbError(f"{path}: [{name}] must be a table")
    return table


def _wandb(cfg: dict[str, Any], path) -> WandbConfig:
    table = _table(cfg, "wandb", path)
    vals = {}
    for key in ("entity", "project"):
        v = table.get(key, "")
        if not isinstance(v, str) or (v.strip() and not _WANDB_NAME_RE.match(v.strip())):
            raise ArbError(f"{path}: [wandb] {key} must be a wandb name (letters, digits, "
                           f"'.', '_', '-'); got {v!r}")
        vals[key] = v.strip()
    return WandbConfig(**vals)


def _run(cfg: dict[str, Any], path) -> RunConfig:
    table = _table(cfg, "run", path)
    raw = table.get("command", "")
    if not isinstance(raw, str):
        raise ArbError(f"{path}: [run] command must be a string, e.g. \"python train.py\"")
    try:
        command = tuple(shlex.split(raw))
    except ValueError as exc:
        raise ArbError(f"{path}: [run] command {raw!r} cannot be split into arguments: {exc}") \
            from exc
    style = table.get("override_style", DEFAULT_OVERRIDE_STYLE)
    if style not in OVERRIDE_STYLES:
        raise ArbError(f"{path}: [run] override_style must be one of: {', '.join(OVERRIDE_STYLES)}"
                       f" (hydra → k=v, argparse/tyro → --k v); got {style!r}")
    return RunConfig(command=command, override_style=style)


def _claude(cfg: dict[str, Any], path) -> ClaudeConfig:
    table = _table(cfg, "claude", path)
    push = table.get("push", True)
    if not isinstance(push, bool):
        raise ArbError(f"{path}: [claude] push must be true or false")
    return ClaudeConfig(push=push)


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
    return ProjectConfig(default_machine=machine.strip(), total_su=float(total),
                         project_id=_project_id(cfg, path), watch=_watch(cfg, path),
                         wandb=_wandb(cfg, path), run=_run(cfg, path),
                         claude=_claude(cfg, path))


def require_project_id(ctx: RepoContext) -> str:
    """The project id that namespaces job names and remote dirs; refuse without one."""
    pid = project_config(ctx).project_id
    if pid is None:
        raise ArbError(f"{ctx.config_path} has no [project] id (it names this repo's jobs and "
                       "remote dirs so repos sharing a machine cannot collide); run `arb init` "
                       "to add it (the rest of the file is left as is)")
    return pid


# ---------------------------------------------------------------- arb init


_PROJECT_HEADER_RE = re.compile(r"^[ \t]*\[[ \t]*project[ \t]*\][ \t]*(?:#[^\n]*)?$", re.M)
PROJECT_COMMENT = ("# 项目 id：作业名和集群上的目录都带上它，几个仓库共用一个机器配置也不会互相覆盖。"
                   "由 arb init 生成；已有 run 提交后不要改。")


def project_table(project_id: str) -> str:
    return f"[project]\n{PROJECT_COMMENT}\nid = \"{project_id}\"\n"


def add_project_id(text: str, project_id: str) -> str:
    """`text` with `[project] id` added; every existing byte is kept as is."""
    validate_project_id(project_id)
    headers = list(_PROJECT_HEADER_RE.finditer(text))
    if not headers:
        sep = "" if not text or text.endswith("\n\n") else "\n" if text.endswith("\n") else "\n\n"
        new = text + sep + project_table(project_id)
    elif len(headers) == 1:
        end = headers[0].end()
        line = f"\n{PROJECT_COMMENT}\nid = \"{project_id}\""
        new = text[:end] + line + text[end:]
    else:
        new = text  # invalid TOML anyway; the check below reports it
    try:
        parsed = tomllib.loads(new)
    except tomllib.TOMLDecodeError:
        parsed = {}
    table = parsed.get("project")
    if not isinstance(table, dict) or table.get("id") != project_id:
        raise ArbError("could not add [project] id to arboria.toml automatically; add\n"
                       f"{project_table(project_id)}by hand")
    return new
