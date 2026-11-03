"""`from arboria import lab`: the notebook API (marimo, but any Python works).

Which idea a call is about is resolved explicitly, in this order (`resolve()`):

1. `lab.use("<idea>")`, a manual override;
2. the calling notebook's file (`__file__` of the caller, e.g. a marimo cell): it must be
   `.arboria/notebooks/<idea>.py`, or the `notebook.py` symlink to it in a worktree;
3. the running marimo app's file, when marimo exposes it;
4. the current directory: an idea worktree (or a folder whose `notebook.py` links to one).

Nothing here approves anything: `run()` only creates the card, like `arb submit`.
Records written from here (todos, notes, confidence, metric) are the human's.

Any other lowercase attribute is a notebook command (`lab.svd(r, "…")`, see commands.py
and the "commands" section below): Claude writes the library, the human calls it here.
"""

from __future__ import annotations

import json
import os
import sys
import time as _time
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable, Iterable, Mapping

from . import commands as _commands
from . import ideas, notebooks, records, sessions, slurm, submission, tasks
from .config import project_config
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

    @property
    def local_out(self) -> Path:
        """The fetched ARBORIA_OUT (`.arboria/runs/<run>/out/`; may not exist yet)."""
        return submission.local_run_dir(self._target.ctx, self.id) / "out"

    @property
    def config(self) -> Any:
        """The run's resolved config (from its config.yaml), or None if not fetched yet."""
        raw = self.row()["config_json"]
        return json.loads(raw) if raw else None

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
        command: list[str] | None = None, repeat: bool = False) -> Run:
    """Create a run awaiting the human's approval (like `arb submit`); never approves.

    `from_` reuses that run's snapshot and defaults (resources, command, overrides);
    `set` adds/replaces overrides (`{"lr": 1e-3}` → `lr=0.001` or `--lr 0.001`).
    Re-running the same cell returns the same run instead of a duplicate, unless
    `repeat=True` (e.g. seed-variance repeats): then every execution is a new run with
    its own card (careful: re-executing that cell creates another one)."""
    t, ledger, idea = _open()
    with ledger:
        if idea["status"] != "open":
            raise ArbError(f"idea {idea['name']!r} is {idea['status']}")
        parent = from_.id if isinstance(from_, Run) else from_
        res, _card = submission.submit(
            t.ctx, ledger, idea, reason, command, gpus=gpus, time=time, nodes=nodes,
            partition=partition, machine=machine, from_run=parent, overrides=_pairs(set),
            repeat=repeat)
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


# ---------------------------------------------------------------- commands
#
# `lab.<name>(...)` runs the notebook command `<name>` (commands.py). Claude writes the
# command files; the human only calls them. A missing command becomes a todo for Claude
# (and, best effort, a message into the running Claude Code session), and the call
# returns a placeholder; the cell re-runs by itself once the file appears or changes.

POLL_S = 1.0  # how often a cell's watcher looks at its command file


class Ctx:
    """What a command function gets as its first argument (see `arb commands guide`)."""

    def __init__(self, target: Target, idea: dict[str, Any], found: _commands.CommandFile):
        self._target = target
        self.name = found.name
        self.scope = found.scope
        self.version = found.version
        self.idea = SimpleNamespace(name=idea["name"], hypothesis=idea["hypothesis"],
                                    metric=idea["metric"])
        self.mo = _marimo_module()

    @property
    def cache_dir(self) -> Path:
        path = _commands.cache_dir(self._target.ctx.state_dir, self.name)
        path.mkdir(parents=True, exist_ok=True)
        return path

    def run(self, run: str | Run) -> Run:
        run_id = run.id if isinstance(run, Run) else str(run)
        with Ledger(self._target.ctx.state_dir) as ledger:
            records.get_run(ledger, run_id)
        return Run(self._target, run_id)

    def runs(self) -> list[Run]:
        """This idea's runs, oldest first."""
        with Ledger(self._target.ctx.state_dir) as ledger:
            idea = ideas.get_idea(ledger, self._target.idea)
            rows = ledger.all("SELECT id FROM runs WHERE idea_id = ? ORDER BY id", (idea["id"],))
        return [Run(self._target, r["id"]) for r in rows]

    @staticmethod
    def value(x: Any) -> Any:
        """`x.value` for a marimo UI element, else `x` itself."""
        if type(x).__module__.startswith("marimo") and hasattr(x, "value"):
            return x.value
        return x

    def __repr__(self) -> str:
        return f"Ctx({self.name!r}, idea={self.idea.name!r})"


def _marimo_module() -> Any:
    if "marimo" in sys.modules:
        return sys.modules["marimo"]
    try:
        import marimo  # optional
    except ImportError:
        return None
    return marimo


def _in_marimo_notebook() -> bool:
    mo = sys.modules.get("marimo")
    try:
        return bool(mo is not None and mo.running_in_notebook())
    except Exception:
        return False


class Result:
    """A command's return value plus where it came from (shown under it in marimo)."""

    def __init__(self, value: Any, found: _commands.CommandFile):
        self.value = value
        self.name, self.version, self.scope = found.name, found.version, found.scope

    @property
    def provenance(self) -> str:
        return f"命令 {self.name} · v{self.version} · {_commands.SCOPE_LABELS[self.scope]}"

    def _display_(self) -> Any:
        mo = sys.modules["marimo"]
        footer = mo.md(f"<small>{self.provenance}</small>")
        return footer if self.value is None else mo.vstack([self.value, footer])

    def __repr__(self) -> str:
        return f"{self.value!r}\n({self.provenance})"


class Pending:
    """What a call to a missing command returns: never raises, renders as a short note."""

    def __init__(self, name: str, kind: str, task_id: str | None, *, created: bool = False,
                 pushed: bool = False, watching: bool = False, error: str | None = None):
        self.name, self.kind, self.task_id = name, kind, task_id
        self.created, self.pushed, self.watching, self.error = created, pushed, watching, error

    def text(self) -> str:
        if self.error:
            return f"命令 {self.name} 还不存在，没能交给 Claude：{self.error}"
        if self.kind == "command_revise":
            head = f"已请 Claude 修改命令 {self.name}（{self.task_id}）。改好后用到它的 cell 会自动重跑。"
        else:
            head = f"命令 {self.name} 还不存在，已交给 Claude（{self.task_id}）。" + (
                "写好后这个 cell 会自动重跑。" if self.watching else "写好后重新运行这个 cell。")
        return head + ("" if self.pushed else "在 Claude 面板里说'处理待办'即可。")

    def _repr_markdown_(self) -> str:
        return self.text()

    def __repr__(self) -> str:
        return self.text()

    def __bool__(self) -> bool:
        return False


class Source:
    """A command's file text (`lab.<name>.source()`), rendered as a code block."""

    def __init__(self, found: _commands.CommandFile, text: str):
        self.path, self.version, self.scope, self.text = found.path, found.version, \
            found.scope, text

    def _repr_markdown_(self) -> str:
        fence = "`" * max(3, max((len(r) for r in _backtick_runs(self.text)), default=0) + 1)
        return (f"`{self.path}`（v{self.version}，{_commands.SCOPE_LABELS[self.scope]}）\n\n"
                f"{fence}python\n{self.text.rstrip()}\n{fence}")

    def __str__(self) -> str:
        return self.text

    def __repr__(self) -> str:
        return f"Source({str(self.path)!r}, v{self.version})"


def _backtick_runs(text: str) -> list[str]:
    import re

    return re.findall(r"`+", text)


def _cell_rerunner() -> Callable[[], None] | None:
    """A function that re-runs the marimo cell executing right now, or None outside a
    running marimo kernel.

    marimo's public reactivity (`mo.state`, `mo.watch.file`/`directory`, `mo.ui.refresh`)
    only re-runs cells that *reference a global variable* bound to the state object or
    UI element (marimo 0.25.2 `CellRunner.resolve_state_updates`: "It has among its refs
    the state object"). A cell like `lab.svd(r, "…")` references only `lab` and `r`, so
    a watch object created inside the call can never trigger it. Instead this marks
    exactly the calling cell stale and asks the kernel to run stale cells, which is what
    marimo itself does for a state update from a `mo.Thread` (private API: the kernel
    context's `_kernel`, `graph.set_stale`, `_execute_stale_cells_callback`). If any of it
    is missing (another marimo version), the placeholder says to re-run the cell."""
    if not _in_marimo_notebook():
        return None
    try:
        from marimo._runtime.context import get_context  # type: ignore

        ctx = get_context()
        cell_id = ctx.execution_context.cell_id
        kernel = ctx._kernel
        graph, run_stale, lazy = kernel.graph, kernel._execute_stale_cells_callback, kernel.lazy
        graph.set_stale  # noqa: B018  (fail now, not in the watcher thread)
    except Exception:
        return None

    def rerun() -> None:
        graph.set_stale({cell_id}, prune_imports=True)
        if not lazy():  # lazy mode: marimo shows the cell as stale instead
            run_stale()

    return rerun


def _watch_loop(name: str, state_dir: Path, seen: tuple[str, str] | None,
                rerun: Callable[[], None], should_exit: Callable[[], bool],
                interval: float = POLL_S) -> bool:
    """Poll until what `lab.<name>` resolves to differs from `seen` and has stayed the same
    for one more interval (so a half-written file is not picked up), then `rerun()`.
    Returns True if it re-ran."""
    pending = None
    while not should_exit():
        _time.sleep(interval)
        try:
            now = _commands.fingerprint(name, state_dir)
        except Exception:
            continue
        if now == seen:
            pending = None
            continue
        if now != pending:  # changed: wait one interval for it to settle
            pending = now
            continue
        if should_exit():
            return False
        try:
            rerun()
        except Exception:
            return False
        return True
    return False


def _watch(name: str, state_dir: Path, seen: tuple[str, str] | None) -> bool:
    """Re-run the calling cell when the command file is created or changed (marimo only).
    The watcher is a `mo.Thread`, so marimo stops it when the cell re-runs or is deleted."""
    rerun = _cell_rerunner()
    if rerun is None:
        return False
    try:
        mo = sys.modules["marimo"]

        def target() -> None:
            me = mo.current_thread()
            _watch_loop(name, state_dir, seen, rerun, lambda: me.should_exit)

        mo.Thread(target=target, daemon=True).start()
        return True
    except Exception:
        return False


def _arg_repr(value: Any) -> str | None:
    return f"Run({value.id!r})" if isinstance(value, Run) else None


def _push(t: Target, idea: dict[str, Any], task: dict[str, Any]) -> bool:
    try:
        if not project_config(t.ctx).claude.push:
            return False
        return sessions.notify(t.ctx.state_dir, idea["worktree_path"],
                               _commands.push_text(task))
    except Exception:
        return False


def _request(t: Target, kind: str, name: str, scope: str, description: str | None,
             args: tuple = (), kwargs: Mapping[str, Any] | None = None, *,
             watching: bool = False, path: str | None = None) -> Pending:
    try:
        _human_only("写命令待办")
        a = [_commands.safe_repr(x, _arg_repr) for x in args]
        kw = {k: _commands.safe_repr(v, _arg_repr) for k, v in (kwargs or {}).items()}
        with Ledger(t.ctx.state_dir) as ledger:
            idea = ideas.get_idea(ledger, t.idea)
            task, created = _commands.request(ledger, idea, kind, name, scope, description,
                                              a, kw, path)
        pushed = _push(t, idea, task) if created else False
        return Pending(name, kind, task["id"], created=created, pushed=pushed,
                       watching=watching)
    except Exception as exc:  # a placeholder never raises
        return Pending(name, kind, None, watching=watching, error=str(exc))


def _log_call(t: Target, found: _commands.CommandFile, ok: bool, seconds: float) -> None:
    """A lightweight event (no backup commit)."""
    try:
        with Ledger(t.ctx.state_dir) as ledger, ledger.write():
            ledger.event("command_call", found.name, {
                "idea": t.idea, "scope": found.scope, "version": found.version, "ok": ok,
                "seconds": round(seconds, 3)})
    except Exception:
        pass


def _invoke(name: str, args: tuple, kwargs: dict[str, Any]) -> Any:
    scope = kwargs.pop("scope", None)
    if scope is not None:
        _commands.validate_scope(scope)
    t = resolve()
    state_dir = t.ctx.require_initialized().state_dir
    found = _commands.find(name, state_dir)
    seen = None if found is None else (str(found.path), found.sha256)
    watching = _watch(name, state_dir, seen)  # armed first: a broken file is fixed later
    if found is None:
        return _request(t, "command_new", name, scope or "project",
                        _commands.description_of(args, kwargs), args, kwargs,
                        watching=watching)
    func = _commands.load(found)
    with Ledger(state_dir) as ledger:
        idea = ideas.get_idea(ledger, t.idea)
    from .human import running_command

    started, ok = _time.monotonic(), False
    try:
        with running_command(found.name):
            value = func(Ctx(t, idea, found), *args, **kwargs)
        ok = True
    finally:
        _log_call(t, found, ok, _time.monotonic() - started)
    return Result(value, found) if _in_marimo_notebook() else value


class Command:
    """`lab.<name>`: call it to run the command; `.revise(...)`, `.source()`, `.info()`."""

    def __init__(self, name: str):
        self.name = _commands.validate_name(name)

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        return _invoke(self.name, args, kwargs)

    def _found(self) -> tuple[Target, _commands.CommandFile | None]:
        t = resolve()
        return t, _commands.find(self.name, t.ctx.require_initialized().state_dir)

    def revise(self, text: str) -> Pending:
        """Ask Claude to change this command (a command_revise todo)."""
        try:
            t, found = self._found()
        except Exception as exc:
            return Pending(self.name, "command_revise", None, error=str(exc))
        if found is None:
            return _request(t, "command_new", self.name, "project", text)
        return _request(t, "command_revise", self.name, found.scope, text, path=str(found.path))

    def source(self) -> Source | str:
        _, found = self._found()
        if found is None:
            return f"命令 {self.name} 还不存在"
        return Source(found, found.path.read_text(encoding="utf-8"))

    def info(self) -> dict[str, Any] | None:
        t, found = self._found()
        if found is None:
            return None
        doc, problem = _commands.describe(found.path, found.name)
        return {"name": found.name, "scope": found.scope, "version": found.version,
                "path": str(found.path), "description": doc, "problem": problem}

    def __repr__(self) -> str:
        return f"lab.{self.name}"


def commands() -> Any:
    """The command library: name, scope, version, description (DataFrame with pandas)."""
    try:
        state_dir: Path | None = resolve().ctx.require_initialized().state_dir
    except ArbError:
        state_dir = None
    rows = [r for r in _commands.discover(state_dir) if r["active"]]
    return _table([{k: r[k] for k in ("name", "scope", "version", "description", "problem")}
                   for r in rows])


def __getattr__(name: str) -> Command:
    """Any other lowercase name is a notebook command (`lab.svd(...)`)."""
    if not _commands.NAME_RE.match(name):
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    return Command(name)
