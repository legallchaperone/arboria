"""`from arboria import lab`: the notebook API (marimo, but any Python works).

Which idea a call is about is resolved explicitly, in this order (`resolve()`):

1. `lab.use("<idea>")`, a manual override;
2. the calling notebook's file (`__file__` of the caller, e.g. a marimo cell): it must be
   `.arboria/notebooks/<idea>.py`, or the `notebook.py` symlink to it in a worktree;
3. the running marimo app's file, when marimo exposes it;
4. the current directory: an idea worktree (or a folder whose `notebook.py` links to one).

Nothing here approves anything: `run()` only creates the card, like `arb submit`.
Records written from here (todos, notes, confidence, metric) are the human's.
"""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping

from . import ideas, notebooks, records, slurm, submission, tasks
from .db import Ledger
from .errors import ArbError
from .paths import RepoContext, locate

HUMAN_ONLY = ("{what} 只由人在 notebook 里做；Claude 请用 `arb todo done tNNN --result \"...\"` "
              "或 `arb run summary RUN \"...\"`")
AWAITING = "等你批准：在你自己的终端运行 `arb approve {run}`"

_use: tuple[str, Path | None] | None = None


@dataclass(frozen=True)
class Target:
    ctx: RepoContext
    idea: str
    source: str  # use | file | marimo | cwd


def use(idea: str | None, repo: str | Path | None = None) -> None:
    """Pin the idea (and optionally the repo) for later calls; `use(None)` clears it."""
    global _use
    _use = None if idea is None else (idea, Path(repo) if repo is not None else None)


def _caller_file() -> str | None:
    """`__file__` of the nearest frame outside this module (e.g. the marimo cell)."""
    frame = sys._getframe(1)
    while frame is not None:
        g = frame.f_globals
        if g.get("__name__") != __name__ and isinstance(g.get("__file__"), str):
            return g["__file__"]
        frame = frame.f_back
    return None


def _marimo_file() -> str | None:
    """The running marimo notebook's path, best effort (private API, may move)."""
    if "marimo" not in sys.modules:
        return None
    try:
        from marimo._runtime.context import get_context  # type: ignore

        name = getattr(get_context(), "filename", None)
        return str(name) if name else None
    except Exception:
        return None


def _from_notebook(path: str | Path | None, source: str) -> Target | None:
    if not path:
        return None
    found = notebooks.idea_of_notebook(path)
    if found is None:
        return None
    state_dir, name = found
    return Target(locate(state_dir.parent), name, source)


def resolve(file: str | Path | None = None, cwd: str | Path | None = None) -> Target:
    """Which repo and idea a lab call is about (see the module docstring)."""
    if _use is not None:
        name, repo = _use
        return Target(locate(repo if repo is not None else cwd), name, "use")
    for path, source in ((file, "file"), (None if file else _caller_file(), "file"),
                         (_marimo_file(), "marimo")):
        target = _from_notebook(path, source)
        if target is not None:
            return target
    cwd = Path(cwd if cwd is not None else os.getcwd())
    target = _from_notebook(cwd / notebooks.LINK_NAME, "cwd")
    if target is not None:
        return target
    ctx = locate(cwd).require_initialized()
    with Ledger(ctx.state_dir) as ledger:
        idea = ideas.current_idea(ctx, ledger)
    if idea is None:
        raise ArbError("lab cannot tell which idea this is: open the idea's notebook "
                       "(.arboria/notebooks/<idea>.py or <worktree>/notebook.py), run from an "
                       "idea worktree, or call lab.use(\"<idea>\")")
    return Target(ctx, idea["name"], "cwd")


def _open(target: Target | None = None) -> tuple[Target, Ledger, dict[str, Any]]:
    t = target or resolve()
    t.ctx.require_initialized()
    ledger = Ledger(t.ctx.state_dir)
    try:
        idea = ideas.get_idea(ledger, t.idea)
    except BaseException:
        ledger.close()
        raise
    return t, ledger, idea


def _human_only(what: str) -> None:
    from .human import in_claude_code

    if in_claude_code():
        raise ArbError(HUMAN_ONLY.format(what=what))


def _table(rows: list[dict[str, Any]]) -> Any:
    """A pandas DataFrame when pandas is installed, else the list of dicts."""
    try:
        import pandas as pd  # optional
    except ImportError:
        return rows
    return pd.DataFrame(rows)


def _wandb_api() -> Any:
    try:
        import wandb  # optional
    except ImportError as exc:
        raise ArbError("读 wandb 曲线需要 wandb：uv add wandb（或 pip install wandb）") from exc
    return wandb.Api()


def _wandb_path(url: str | None, run_id: str) -> str:
    """`entity/project/id` from a run's wandb_url."""
    prefix = "https://wandb.ai/"
    if not url or not url.startswith(prefix) or "/runs/" not in url:
        raise ArbError(f"run {run_id} has no wandb link: set [wandb] entity and project in "
                       "arboria.toml before submitting")
    entity_project, _, wid = url[len(prefix):].partition("/runs/")
    return f"{entity_project}/{wid}"


# ---------------------------------------------------------------- runs


class Run:
    """A handle on one run; every accessor reads the ledger afresh."""

    def __init__(self, target: Target, run_id: str, existed: bool = False):
        self._target = target
        self.id = run_id
        self.existed = existed

    def _ledger(self) -> Ledger:
        return Ledger(self._target.ctx.state_dir)

    def row(self) -> dict[str, Any]:
        with self._ledger() as ledger:
            return records.get_run(ledger, self.id)

    def status(self) -> str:
        return self.row()["status"]

    def card(self) -> str:
        with self._ledger() as ledger:
            card, matches = submission.card_for(self._target.ctx, ledger, self.id)
        return card.text() + ("" if matches else f"\n⚠ {submission.CARD_CHANGED}")

    def jobs(self) -> list[dict[str, Any]]:
        with self._ledger() as ledger:
            return ledger.all("SELECT segment, job_name, job_id, state, elapsed_s, billed_su"
                              " FROM jobs WHERE run_id = ? ORDER BY segment", (self.id,))

    @property
    def wandb_url(self) -> str | None:
        return self.row()["wandb_url"]

    def history(self, keys: Iterable[str] | str) -> list[dict[str, Any]]:
        """wandb history rows for `keys` (plus `_step`) as a list of dicts."""
        keys = [keys] if isinstance(keys, str) else list(keys)
        path = _wandb_path(self.wandb_url, self.id)
        run = _wandb_api().run(path)
        return [dict(r) for r in run.scan_history(keys=keys)]

    def metrics(self, keys: Iterable[str] | str) -> Any:
        """wandb metrics: a DataFrame if pandas is installed, else a list of dicts."""
        return _table(self.history(keys))

    def _repr_markdown_(self) -> str:
        run = self.row()
        lines = [f"**{self.id}** · {run['status']} · {run['change_reason']}"
                 + ("（已存在，没有重复提交）" if self.existed else "")]
        if run["parent_run"]:
            lines.append(f"来自 {run['parent_run']}（同一快照）")
        if run["status"] == "awaiting_approval":
            lines += ["", AWAITING.format(run=self.id), "", "```text", self.card(), "```"]
            return "\n".join(lines)
        jobs = self.jobs()
        if jobs:
            lines += ["", "| 段 | 作业 | 状态 | 时长 | SU |", "| --- | --- | --- | --- | --- |"]
            for j in jobs:
                elapsed = slurm.human_duration(j["elapsed_s"]) if j["elapsed_s"] else "-"
                su = f"{j['billed_su']:.2f}" if j["billed_su"] is not None else "-"
                lines.append(f"| s{j['segment']} | {j['job_id'] or '-'} | {j['state']} | "
                             f"{elapsed} | {su} |")
        if run["est_cost"] is not None:
            cost = run["actual_cost"] or 0
            lines += ["", f"花费 {cost:.2f} SU（估计上限 {run['est_cost']:.2f}）"]
        if run["wandb_url"]:
            lines.append(f"[wandb]({run['wandb_url']})")
        if run["summary"]:
            lines += ["", run["summary"]]
        return "\n".join(lines)

    def __repr__(self) -> str:
        return f"Run({self.id!r})"


def _pairs(set_: Mapping[str, Any] | Iterable[tuple[str, Any]] | None) -> list[tuple[str, str]]:
    if set_ is None:
        return []
    items = set_.items() if isinstance(set_, Mapping) else set_
    return [slurm.check_override(k, v) for k, v in items]


def run(*, reason: str, from_: str | Run | None = None,
        set: Mapping[str, Any] | Iterable[tuple[str, Any]] | None = None,  # noqa: A002
        gpus: int | None = None, time: str | None = None, partition: str | None = None,
        nodes: int | None = None, machine: str | None = None,
        command: list[str] | None = None) -> Run:
    """Create a run awaiting the human's approval (like `arb submit`); never approves.

    `from_` reuses that run's snapshot and defaults (resources, command, overrides);
    `set` adds/replaces overrides (`{"lr": 1e-3}` → `lr=0.001` or `--lr 0.001`).
    Re-running the same cell returns the same run instead of a duplicate."""
    t, ledger, idea = _open()
    with ledger:
        if idea["status"] != "open":
            raise ArbError(f"idea {idea['name']!r} is {idea['status']}")
        parent = from_.id if isinstance(from_, Run) else from_
        res, _card = submission.submit(
            t.ctx, ledger, idea, reason, command, gpus=gpus, time=time, nodes=nodes,
            partition=partition, machine=machine, from_run=parent, overrides=_pairs(set))
    return Run(t, res["id"], existed=res["existed"])


def get(run_id: str) -> Run:
    t, ledger, idea = _open()
    with ledger:
        records.get_run(ledger, run_id)
    return Run(t, run_id)


RUN_COLUMNS = ("id", "status", "change_reason", "parent_run", "machine", "gpus", "walltime",
               "est_cost", "actual_cost", "wandb_url", "summary", "created_at")


def runs() -> Any:
    """This idea's runs as a table (DataFrame if pandas is installed, else list of dicts)."""
    t, ledger, idea = _open()
    with ledger:
        rows = ledger.all(f"SELECT {', '.join(RUN_COLUMNS)} FROM runs WHERE idea_id = ?"
                          " ORDER BY id", (idea["id"],))
    return _table(rows)


def compare(run_ids: Iterable[str | Run], key: str | Iterable[str]) -> Any:
    """wandb history of `key` for several runs, long format with a `run` column."""
    rows: list[dict[str, Any]] = []
    for r in run_ids:
        handle = r if isinstance(r, Run) else get(r)
        rows += [{"run": handle.id, **row} for row in handle.history(key)]
    return _table(rows)


# ---------------------------------------------------------------- the human's records


def todo(text: str) -> dict[str, Any]:
    """Leave a todo for Claude on this idea (shown by the SessionStart hook)."""
    _human_only("写待办")
    t, ledger, idea = _open()
    with ledger:
        return tasks.add(ledger, idea, text)


def todos(include_closed: bool = False) -> Any:
    t, ledger, idea = _open()
    with ledger:
        return _table(tasks.list_tasks(ledger, idea["id"], include_closed=include_closed))


def conf(level: str) -> dict[str, Any]:
    """Set the idea's confidence: guess | prelim | confirmed | rejected."""
    _human_only("改置信度")
    t, ledger, idea = _open()
    with ledger:
        return records.set_confidence(ledger, idea, level)


def metric(text: str) -> dict[str, Any]:
    """Replace the idea's success metric (≤200 chars)."""
    _human_only("改指标")
    t, ledger, idea = _open()
    with ledger:
        return records.set_metric(ledger, idea, text)


def fork(name: str, from_: str | Run | None = None, *, hypothesis: str,
         metric: str) -> dict[str, Any]:
    """A new idea branching off this one (at `from_`'s snapshot if given), with its own
    worktree and notebook whose header links back here. Returns the new idea."""
    t, ledger, idea = _open()
    with ledger:
        run_row = None
        if from_ is not None:
            run_row = records.get_run(ledger, from_.id if isinstance(from_, Run) else from_)
            if run_row["idea_id"] != idea["id"]:
                raise ArbError(f"run {run_row['id']} belongs to another idea")
        return ideas.new_idea(t.ctx, ledger, name, hypothesis, metric,
                              parent=idea["name"], from_run=run_row)


def prune(reason: str) -> dict[str, Any]:
    """Drop this idea (= `arb idea close --drop`); `reason` is kept as a note."""
    _human_only("关闭 idea")
    t, ledger, idea = _open()
    with ledger:
        from .db import validate_text

        reason = validate_text("note", reason)
        closed = ideas.close_idea(t.ctx, ledger, idea["name"], "drop")
        records.add_note(ledger, closed, f"prune: {reason}")
        return closed
