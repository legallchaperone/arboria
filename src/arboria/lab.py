"""`from arboria import lab`: the notebook API (marimo, but any Python works).

Which idea a call is about is resolved explicitly, in this order (`resolve()`):

1. `lab.use("<idea>")`, a manual override;
2. the calling notebook's file (`__file__` of the caller, e.g. a marimo cell): it must be
   `.arboria/notebooks/<idea>.py`, or the `notebook.py` symlink to it in a worktree;
3. the running marimo app's file, when marimo exposes it;
4. the current directory: an idea worktree (or a folder whose `notebook.py` links to one).

A seed's notebook (`<home>/seeds/<name>.py`, see seeds.py) resolves to a `SeedTarget`:
a seed has no repo, so everything about runs and ideas (`run`, `fork`, `prune`, …)
refuses with a pointer to `arb idea plant`; `todo` / `todos` work on the home's ledger,
`metric` sets the seed's metric, and global notebook commands can be called.

Nothing here approves anything: `run()` only creates the card, like `arb submit`.
Records written from here (todos, notes, confidence, metric) are the human's.

Any other lowercase attribute is a notebook command (`lab.svd(r, "…")`, see commands.py
and the "commands" section below): Claude writes the library, the human calls it here.
"""

from __future__ import annotations

import collections.abc as _abc
import json
import os
import sys
import time as _time
from dataclasses import dataclass, field
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable, Iterable, Mapping

from . import commands as _commands
from . import ideas, notebooks, records, sessions, slurm, submission, tasks
from . import seeds as _seeds
from .config import project_config
from .db import Ledger
from .errors import ArbError
from .paths import Home, RepoContext, locate, seed_of_notebook

HUMAN_ONLY = ("{what} 只由人在 notebook 里做；Claude 请用 `arb todo done tNNN --result \"...\"` "
              "或 `arb run summary RUN \"...\"`")

SEED_NOT_PLANTED = "这是一个种子（{name}），还没种进仓库：先在目标仓库里运行 arb idea plant {name}"
SEED_PLANTED = ("这是种子 {name} 的 notebook，它已种进 {where}：run、fork 这些在那个 idea 的 "
                "notebook 里做")
SEED_NO_CONF = "种子没有置信度：种下（arb idea plant）之后在 idea 的 notebook 里用 lab.conf"
SEED_GLOBAL_ONLY = "种子没有仓库，只能用全局命令：去掉 scope=，或者写 scope=\"global\""

_use: tuple[str, Path | None] | None = None


@dataclass(frozen=True)
class Target:
    ctx: RepoContext
    idea: str
    source: str  # use | file | marimo | cwd

    @property
    def state_dir(self) -> Path:
        return self.ctx.state_dir


@dataclass(frozen=True)
class SeedTarget:
    """A seed's notebook in the home: no repo, no runs."""

    home: Home
    seed: str
    source: str  # file | marimo

    @property
    def state_dir(self) -> Path:
        return self.home.state_dir


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
    """The running marimo notebook's path, best effort.

    marimo patches `__file__` in cell globals, so step 2 (`_caller_file`) normally finds
    the notebook already. This is the fallback: the public `mo.notebook_dir()` when that
    folder has a `notebook.py` link (an idea worktree), else the kernel context's
    filename (private API, may move)."""
    if "marimo" not in sys.modules:
        return None
    try:
        import marimo as mo

        folder = mo.notebook_dir()
        if folder is not None and (Path(folder) / notebooks.LINK_NAME).is_symlink():
            return str(Path(folder) / notebooks.LINK_NAME)
    except Exception:
        pass
    try:
        from marimo._runtime.context import get_context  # type: ignore

        name = getattr(get_context(), "filename", None)
        return str(name) if name else None
    except Exception:
        return None


def _from_notebook(path: str | Path | None, source: str) -> Target | SeedTarget | None:
    if not path:
        return None
    seed = seed_of_notebook(path)
    if seed is not None:
        return SeedTarget(seed[0], seed[1], source)
    found = notebooks.idea_of_notebook(path)
    if found is None:
        return None
    state_dir, name = found
    return Target(locate(state_dir.parent), name, source)


def resolve(file: str | Path | None = None,
            cwd: str | Path | None = None) -> Target | SeedTarget:
    """Which repo and idea (or which seed) a lab call is about (see the module docstring)."""
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


def _seed_error(t: SeedTarget) -> ArbError:
    try:
        with Ledger(t.state_dir) as ledger:
            seed = _seeds.get_seed(ledger, t.seed)
            done = _seeds.plantings(ledger, seed["id"])
    except Exception:
        done = []
    if done:
        where = "、".join(f"{Path(p['repo']).name} · {p['idea']}" for p in done)
        return ArbError(SEED_PLANTED.format(name=t.seed, where=where))
    return ArbError(SEED_NOT_PLANTED.format(name=t.seed))


def _open_seed(t: SeedTarget) -> tuple[Ledger, dict[str, Any]]:
    ledger = Ledger(t.state_dir)
    try:
        seed = _seeds.get_seed(ledger, t.seed)
    except BaseException:
        ledger.close()
        raise
    return ledger, seed


def _open(target: Target | SeedTarget | None = None) -> tuple[Target, Ledger, dict[str, Any]]:
    t = target or resolve()
    if isinstance(t, SeedTarget):
        raise _seed_error(t)
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


def status_label(status: str) -> str:
    """The Chinese label of a run status (the dashboard's)."""
    from .ui.views import RUN_STATUS

    return RUN_STATUS.get(status, (status, ""))[0]


def job_state_label(state: str) -> str:
    from .ui.views import JOB_STATE

    return JOB_STATE.get(state, state)


def _short(seconds: int | float | None) -> str:
    """9h42m, 25m, 40s (no seconds once it is over a minute)."""
    s = int(seconds or 0)
    return slurm.human_duration(s - s % 60 if s >= 60 else s)


def _clip(text: str, n: int) -> str:
    text = " ".join(text.split())
    return text if len(text) <= n else text[: n - 1] + "…"


def _esc(text: Any) -> str:
    import html

    return html.escape("" if text is None else str(text), quote=True)


STATUS_COLOR = {"awaiting_approval": "#b7791f", "failed": "#c53030", "succeeded": "#2f855a"}
MUTED = "opacity:.62"
CODE = "padding:1px 6px;border-radius:4px;background:rgba(127,127,127,.12)"
JOB_HEAD = ("段", "作业", "状态", "时长", "SU")


@dataclass
class _RunView:
    """What a Run shows in a notebook; rendered as HTML (marimo, Jupyter) or Markdown."""

    run_id: str
    label: str
    status: str
    reason: str
    origin: str | None
    line: list[tuple[str, ...]] = field(default_factory=list)  # (kind, text[, url])
    detail_title: str | None = None
    pre: str | None = None
    table: list[list[str]] = field(default_factory=list)
    text: str | None = None
    foot: str | None = None

    def _join(self, parts: list[str]) -> str:
        """Parts joined with ' · ', except right after a lead-in ending in '：'."""
        out = parts[0]
        for prev, part in zip(self.line, parts[1:]):
            out += ("" if prev[1].endswith("：") else " · ") + part
        return out

    def html(self) -> str:
        color = STATUS_COLOR.get(self.status)
        label = (f'<span style="color:{color}">{_esc(self.label)}</span>' if color
                 else _esc(self.label))
        head = f"<strong>{_esc(self.run_id)}</strong> · {label} · {_esc(self.reason)}"
        if self.origin:
            head += f' · <span style="{MUTED}">{_esc(self.origin)}</span>'
        out = [f"<div>{head}</div>"]
        if self.line:
            parts = []
            for kind, text, *url in self.line:
                if kind == "code":
                    parts.append(f'<code style="{CODE}">{_esc(text)}</code>')
                elif kind == "link" and url and url[0].startswith(("https://", "http://")):
                    parts.append(f'<a href="{_esc(url[0])}" target="_blank" '
                                 f'rel="noopener">{_esc(text)}</a>')
                elif kind == "warn":
                    parts.append(f'<span style="color:#c53030">{_esc(text)}</span>')
                else:
                    parts.append(_esc(text))
            style = "opacity:.85" if self.status == "awaiting_approval" else MUTED
            out.append(f'<div style="margin-top:2px;{style}">{self._join(parts)}</div>')
        if self.detail_title:
            inner = []
            if self.text:
                inner.append(f'<div style="white-space:pre-wrap;margin:6px 0">{_esc(self.text)}</div>')
            if self.table:
                cells = "".join(f'<th style="text-align:left;padding:2px 14px 2px 0;'
                                f'font-weight:500">{h}</th>' for h in JOB_HEAD)
                rows = "".join("<tr>" + "".join(f'<td style="padding:2px 14px 2px 0">{_esc(c)}</td>'
                                                for c in r) + "</tr>" for r in self.table)
                inner.append(f'<table style="font-size:.9em;border-collapse:collapse;margin:4px 0">'
                             f"<tr>{cells}</tr>{rows}</table>")
            if self.pre:
                inner.append('<pre style="font-size:12px;line-height:1.5;margin:6px 0 0;'
                             'padding:10px 12px;border-radius:6px;white-space:pre-wrap;'
                             f'background:rgba(127,127,127,.08)">{_esc(self.pre)}</pre>')
            if self.foot:
                inner.append(f'<div style="{MUTED};font-size:.85em;margin-top:4px">'
                             f"{_esc(self.foot)}</div>")
            out.append(f'<details style="margin-top:6px"><summary style="cursor:pointer;'
                       f'{MUTED};font-size:.9em">{_esc(self.detail_title)}</summary>'
                       + "".join(inner) + "</details>")
        return '<div class="arb-run" style="line-height:1.7">' + "".join(out) + "</div>"

    def markdown(self) -> str:
        def md(text: Any) -> str:
            import html

            return html.escape("" if text is None else str(text), quote=False)

        head = f"**{md(self.run_id)}** · {md(self.label)} · {md(self.reason)}"
        if self.origin:
            head += f" · {md(self.origin)}"
        blocks = [head]
        if self.line:
            parts = []
            for kind, text, *url in self.line:
                if kind == "code":
                    parts.append(f"`{text}`")
                elif kind == "link" and url:
                    parts.append(f"[{md(text)}]({url[0]})")
                else:
                    parts.append(md(text))
            blocks.append(self._join(parts))
        if self.text:
            blocks.append(md(self.text))
        if self.table:
            rows = ["| " + " | ".join(JOB_HEAD) + " |", "|" + " --- |" * len(JOB_HEAD)]
            rows += ["| " + " | ".join(md(c) for c in r) + " |" for r in self.table]
            blocks.append("\n".join(rows))
        if self.pre:
            fence = "`" * max(3, max((len(r) for r in _backtick_runs(self.pre)), default=0) + 1)
            blocks.append(f"{fence}text\n{self.pre}\n{fence}")
        if self.foot:
            blocks.append(f"_{md(self.foot)}_")
        return "\n\n".join(blocks)




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

    # -- display: one status line, one short line that depends on the status, and the
    # rest (approval card, jobs, summary) collapsed. Ledger text is escaped (some of it
    # is written by an LLM).

    def _view(self) -> _RunView:
        with self._ledger() as ledger:
            run = records.get_run(ledger, self.id)
            jobs = ledger.all("SELECT segment, job_id, state, elapsed_s, billed_su FROM jobs"
                              " WHERE run_id = ? ORDER BY segment", (self.id,))
            final = ledger.one("SELECT payload_json FROM events WHERE subject = ? AND kind IN"
                               " ('run_final', 'run_reject') ORDER BY id DESC LIMIT 1",
                               (self.id,))
            payload = json.loads(final["payload_json"] or "{}") if final else {}
            note = None
            if payload.get("note"):
                note = ledger.one("SELECT text FROM notes WHERE id = ?", (payload["note"],))
            card_text, card_ok = None, True
            if run["status"] == "awaiting_approval":
                try:
                    card, card_ok = submission.card_for(self._target.ctx, ledger, self.id)
                    card_text = card.text()
                except ArbError as exc:
                    card_text, card_ok = f"审核单算不出来：{exc}", False
        status = run["status"]
        v = _RunView(self.id, status_label(status), status, run["change_reason"],
                     f"来自 {run['parent_run']}" if run["parent_run"] else None)
        cost = run["actual_cost"]
        su = [("text", f"{cost:.2f} SU")] if cost else []
        if status == "awaiting_approval":
            v.line = ([("text", "在你自己的终端里批准："), ("code", f"arb approve {self.id}")]
                      if card_ok else [("warn", f"⚠ {submission.CARD_CHANGED}")])
            v.detail_title = "审核单"
            v.pre = card_text
        elif status == "queued":
            ids = ", ".join(j["job_id"] for j in jobs if j["job_id"])
            v.line = [("text", "排队中" + (f"，作业 {ids}" if ids else ""))]
        elif status == "running":
            elapsed = sum(j["elapsed_s"] or 0 for j in jobs)
            done = sum(1 for j in jobs if j["state"] in ("completed",))
            parts = [f"已运行 {_short(elapsed)}"]
            if len(jobs) > 1:
                parts.append(f"第 {min(done + 1, len(jobs))}/{len(jobs)} 段")
            v.line = [("text", " · ".join(parts))]
        elif status == "succeeded":
            elapsed = sum(j["elapsed_s"] or 0 for j in jobs)
            v.line = ([("text", f"用时 {_short(elapsed)}")] if elapsed else []) + su
            if run["wandb_url"]:
                v.line.append(("link", "wandb", run["wandb_url"]))
        elif status == "failed":
            reason = payload.get("reason")
            if not reason:
                bad = [f"s{j['segment']} {job_state_label(j['state'])}" for j in jobs
                       if j["state"] in ("failed", "timeout", "cancelled")]
                reason = "，".join(bad) or None
            v.line = ([("text", _clip(reason, 160))] if reason else []) + su
            if run["wandb_url"]:
                v.line.append(("link", "wandb", run["wandb_url"]))
        elif status == "cancelled":
            v.line = su
        elif status == "rejected" and note:
            v.line = [("text", _clip(note["text"], 160))]
        if jobs and status != "awaiting_approval":
            v.detail_title = "详情"
            v.table = [[f"s{j['segment']}", j["job_id"] or "", job_state_label(j["state"]),
                        _short(j["elapsed_s"]) if j["elapsed_s"] else "",
                        f"{j['billed_su']:.2f}" if j["billed_su"] is not None else ""]
                       for j in jobs]
        if run["summary"]:
            v.detail_title = v.detail_title or "详情"
            v.text = run["summary"]
        if self.existed:
            v.detail_title = v.detail_title or "详情"
            v.foot = "这个 cell 重新运行时复用了已有的 run，没有重复提交"
        return v

    def _repr_html_(self) -> str:
        return self._view().html()

    def _repr_markdown_(self) -> str:
        return self._view().markdown()

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


def _local_short(ts: str | None) -> str:
    """'2026-10-31T14:02:11Z' -> '10-31 22:02' in local time."""
    if not ts:
        return ""
    from datetime import datetime

    try:
        return datetime.fromisoformat(ts.replace("Z", "+00:00")).astimezone().strftime(
            "%m-%d %H:%M")
    except ValueError:
        return ts[:16]


def _su(row: dict[str, Any]) -> str:
    if row["actual_cost"] is not None:
        return f"{row['actual_cost']:.2f}"
    return f"≤ {row['est_cost']:g}" if row["est_cost"] is not None else ""


def run_rows(rows: list[dict[str, Any]]) -> list[dict[str, str]]:
    """The compact table `lab.runs()` shows (all strings; empty, never NaN)."""
    return [{"run": r["id"], "状态": status_label(r["status"]), "父 run": r["parent_run"] or "",
             "原因": r["change_reason"] or "",
             "资源": f"{r['gpus']} GPU × {r['walltime']}" if r["gpus"] and r["walltime"] else "",
             "SU": _su(r), "创建": _local_short(r["created_at"])} for r in rows]


def runs(raw: bool = False) -> Any:
    """This idea's runs as a compact table: run, 状态, 父 run, 原因, 资源, SU (actual, or
    `≤ estimate` before it is known), 创建. `raw=True` gives every column of the ledger's
    runs table instead. A DataFrame if pandas is installed, else a list of dicts."""
    t, ledger, idea = _open()
    with ledger:
        rows = ledger.all("SELECT * FROM runs WHERE idea_id = ? ORDER BY id", (idea["id"],))
    return _table(rows if raw else run_rows(rows))


def compare(run_ids: Iterable[str | Run], key: str | Iterable[str]) -> Any:
    """wandb history of `key` for several runs, long format with a `run` column."""
    rows: list[dict[str, Any]] = []
    for r in run_ids:
        handle = r if isinstance(r, Run) else get(r)
        rows += [{"run": handle.id, **row} for row in handle.history(key)]
    return _table(rows)


# ---------------------------------------------------------------- the human's records


TODO_STATUS = {"open": "等 Claude", "done": "已完成", "dropped": "已放弃"}
TODO_CLIP = 160  # one line in the notebook; the full text is in the tooltip / `.text`


def _todo_age(ts: str | None, now: Any) -> str:
    from .ui.views import age

    return age(ts, now)


def _todo_kind(row: Mapping[str, Any]) -> str:
    from .ui.views import todo_kind

    return todo_kind(dict(row))


def _md_escape(text: str) -> str:
    """Escaped for marimo's Markdown: HTML first, then the inline Markdown marks."""
    import html
    import re

    return re.sub(r"([\\`*_\[\]#|~])", r"\\\1", html.escape(text, quote=False))


class Todo(_abc.Mapping):
    """What `lab.todo` returns: the todo row, shown as one calm line.

    It reads like the row it wraps (`t["id"]`, `t.get("status")`, `dict(t)`, `t.data`),
    plus `t.id`, `t.text`, `t.status`, `t.result`; `t.existed` is True when the call found
    the same open todo instead of writing a new one (the cell re-ran)."""

    def __init__(self, row: Mapping[str, Any], *, existed: bool = False):
        self.data: dict[str, Any] = dict(row)
        self.existed = existed

    def __getitem__(self, key: str) -> Any:
        return self.data[key]

    def __iter__(self) -> Any:
        return iter(self.data)

    def __len__(self) -> int:
        return len(self.data)

    @property
    def id(self) -> str:
        return self.data["id"]

    @property
    def text(self) -> str:
        return self.data["text"]

    @property
    def status(self) -> str:
        return self.data["status"]

    @property
    def result(self) -> str | None:
        return self.data.get("result")

    def lead(self) -> str:
        if self.status == "done":
            label = "已完成"
        elif self.status == "dropped":
            label = "已放弃"
        else:
            label = "已在待办里" if self.existed else "已交给 Claude"
        return f"{label}（{self.id}）："

    def _tail(self) -> str | None:
        return self.result if self.status != "open" and self.result else None

    def __str__(self) -> str:
        tail = self._tail()
        return self.lead() + _clip(self.text, TODO_CLIP) + (
            f" → {_clip(tail, TODO_CLIP)}" if tail else "")

    __repr__ = __str__

    def _repr_html_(self) -> str:
        tail = self._tail()
        out = (f'<span style="{MUTED}">{_esc(self.lead())}</span>'
               f'<span title="{_esc(self.text)}">{_esc(_clip(self.text, TODO_CLIP))}</span>')
        if tail:
            out += (f'<span style="{MUTED}"> → </span>'
                    f'<span title="{_esc(tail)}">{_esc(_clip(tail, TODO_CLIP))}</span>')
        return f'<div class="arb-todo" style="line-height:1.7">{out}</div>'

    def _repr_markdown_(self) -> str:
        return _md_escape(str(self))


class TodoList(_abc.Sequence):
    """What `lab.todos()` returns: the todos (each a `Todo`), oldest first, shown as a
    compact list: id · 状态 · text · age. `.data` is the plain rows (for a DataFrame)."""

    def __init__(self, rows: Iterable[Mapping[str, Any]], *, include_closed: bool = False):
        from datetime import datetime, timezone

        self.items = [Todo(r) for r in rows]
        self.include_closed = include_closed
        self.loaded_at = datetime.now(timezone.utc)

    def __getitem__(self, i: Any) -> Any:
        return self.items[i]

    def __len__(self) -> int:
        return len(self.items)

    @property
    def data(self) -> list[dict[str, Any]]:
        return [t.data for t in self.items]

    def empty_text(self) -> str:
        return "没有待办" if self.include_closed else "没有未完成的待办"

    def _cells(self, t: Todo) -> tuple[str, str, str, str]:
        kind = _todo_kind(t)
        text = (f"[{kind}] " if kind else "") + _clip(t.text, TODO_CLIP)
        return (t.id, TODO_STATUS.get(t.status, t.status), text,
                _todo_age(t.get("created_at"), self.loaded_at))

    def __str__(self) -> str:
        if not self.items:
            return self.empty_text()
        lines = []
        for t in self.items:
            tid, label, text, when = self._cells(t)
            tail = t._tail()
            text += f" → {_clip(tail, 80)}" if tail else ""
            lines.append(f"{tid} · {label} · {text} · {when}")
        return "\n".join(lines)

    __repr__ = __str__

    def _repr_html_(self) -> str:
        if not self.items:
            return f'<div class="arb-todos" style="{MUTED}">{_esc(self.empty_text())}</div>'
        rows = []
        for t in self.items:
            tid, label, text, when = self._cells(t)
            color = "#2f855a" if t.status == "done" else None
            style = f' style="color:{color}"' if color else (
                f' style="{MUTED}"' if t.status == "dropped" else "")
            tail = t._tail()
            rows.append(
                f'<div><code style="{CODE}">{_esc(tid)}</code>'
                f' · <span{style}>{_esc(label)}</span>'
                f' · <span title="{_esc(t.text)}">{_esc(text)}</span>'
                + (f'<span style="{MUTED}" title="{_esc(tail)}"> → {_esc(_clip(tail, 80))}</span>'
                   if tail else "")
                + f' · <span style="{MUTED}">{_esc(when)}</span></div>')
        return '<div class="arb-todos" style="line-height:1.8">' + "".join(rows) + "</div>"

    def _repr_markdown_(self) -> str:
        if not self.items:
            return self.empty_text()
        return "\n".join(f"- {_md_escape(line)}" for line in str(self).splitlines())


def todo(text: str) -> Todo:
    """Leave a todo for Claude on this idea (shown by the SessionStart hook).

    An open todo with the same text is returned instead of a duplicate: the cell
    re-runs every time the notebook opens. Shown as one line in the notebook."""
    _human_only("写待办")
    from .db import validate_text

    text = validate_text("todo", text)
    t = resolve()
    if isinstance(t, SeedTarget):  # a todo for a Claude Code session opened in the home
        ledger, owner = _open_seed(t)
        seed = True
    else:
        t, ledger, owner = _open(t)
        seed = False
    with ledger:
        existing = tasks.open_plain_task(ledger, owner["id"], text, seed=seed)
        if existing is not None:
            return Todo(existing, existed=True)
        return Todo(tasks.add(ledger, owner, text))


def todos(include_closed: bool = False) -> TodoList:
    """This idea's (or seed's) todos, oldest first: open ones, or all with include_closed."""
    t = resolve()
    if isinstance(t, SeedTarget):
        ledger, seed = _open_seed(t)
        with ledger:
            return TodoList(tasks.list_tasks(ledger, seed_id=seed["id"],
                                             include_closed=include_closed),
                            include_closed=include_closed)
    t, ledger, idea = _open(t)
    with ledger:
        return TodoList(tasks.list_tasks(ledger, idea["id"], include_closed=include_closed),
                        include_closed=include_closed)


def conf(level: str) -> dict[str, Any]:
    """Set the idea's confidence: guess | prelim | confirmed | rejected (no-op if unchanged)."""
    _human_only("改置信度")
    t = resolve()
    if isinstance(t, SeedTarget):
        raise ArbError(SEED_NO_CONF)
    t, ledger, idea = _open(t)
    with ledger:
        return records.set_confidence(ledger, idea, level)


def metric(text: str) -> dict[str, Any]:
    """Replace the idea's (or the seed's) success metric (≤200 chars; no-op if unchanged)."""
    _human_only("改指标")
    t = resolve()
    if isinstance(t, SeedTarget):
        ledger, _seed = _open_seed(t)
        with ledger:
            return _seeds.set_fields(ledger, t.seed, metric=text)
    t, ledger, idea = _open(t)
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
    """What a command function gets as its first argument (see `arb commands guide`).
    In a seed's notebook `idea` is None and `seed` is set (no runs there)."""

    def __init__(self, target: Target | SeedTarget, owner: dict[str, Any],
                 found: _commands.CommandFile):
        self._target = target
        self.name = found.name
        self.scope = found.scope
        self.version = found.version
        self.idea = self.seed = None
        info = SimpleNamespace(name=owner["name"], hypothesis=owner["hypothesis"],
                               metric=owner["metric"])
        if isinstance(target, SeedTarget):
            info.gist = owner["gist"]
            info.notebook = target.home.notebook(owner["name"])
            self.seed = info
        else:
            self.idea = info
        self.mo = _marimo_module()

    @property
    def cache_dir(self) -> Path:
        path = _commands.cache_dir(self._target.state_dir, self.name)
        path.mkdir(parents=True, exist_ok=True)
        return path

    def run(self, run: str | Run) -> Run:
        if isinstance(self._target, SeedTarget):
            raise _seed_error(self._target)
        run_id = run.id if isinstance(run, Run) else str(run)
        with Ledger(self._target.ctx.state_dir) as ledger:
            records.get_run(ledger, run_id)
        return Run(self._target, run_id)

    def runs(self) -> list[Run]:
        """This idea's runs, oldest first."""
        if isinstance(self._target, SeedTarget):
            raise _seed_error(self._target)
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
        if self.seed is not None:
            return f"Ctx({self.name!r}, seed={self.seed.name!r})"
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


def _watch_loop(name: str, state_dir: Path | None, seen: tuple[str, str] | None,
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


def _watch(name: str, state_dir: Path | None, seen: tuple[str, str] | None) -> bool:
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


def _push(t: Target | SeedTarget, owner: dict[str, Any], task: dict[str, Any]) -> bool:
    """To the Claude Code session of the idea's worktree, or of the home for a seed."""
    try:
        if isinstance(t, SeedTarget):
            return sessions.notify(t.state_dir, t.home.root, _commands.push_text(task))
        if not project_config(t.ctx).claude.push:
            return False
        return sessions.notify(t.ctx.state_dir, owner["worktree_path"],
                               _commands.push_text(task))
    except Exception:
        return False


def _owner(t: Target | SeedTarget, ledger: Ledger) -> dict[str, Any]:
    if isinstance(t, SeedTarget):
        return _seeds.get_seed(ledger, t.seed)
    return ideas.get_idea(ledger, t.idea)


def _request(t: Target | SeedTarget, kind: str, name: str, scope: str,
             description: str | None, args: tuple = (),
             kwargs: Mapping[str, Any] | None = None, *,
             watching: bool = False, path: str | None = None) -> Pending:
    try:
        _human_only("写命令待办")
        a = [_commands.safe_repr(x, _arg_repr) for x in args]
        kw = {k: _commands.safe_repr(v, _arg_repr) for k, v in (kwargs or {}).items()}
        with Ledger(t.state_dir) as ledger:
            owner = _owner(t, ledger)
            task, created = _commands.request(ledger, owner, kind, name, scope, description,
                                              a, kw, path)
        pushed = _push(t, owner, task) if created else False
        return Pending(name, kind, task["id"], created=created, pushed=pushed,
                       watching=watching)
    except Exception as exc:  # a placeholder never raises
        return Pending(name, kind, None, watching=watching, error=str(exc))


def _log_call(t: Target | SeedTarget, found: _commands.CommandFile, ok: bool,
              seconds: float) -> None:
    """A lightweight event (no backup commit)."""
    owner = {"seed": t.seed} if isinstance(t, SeedTarget) else {"idea": t.idea}
    try:
        with Ledger(t.state_dir) as ledger, ledger.write():
            ledger.event("command_call", found.name, owner | {
                "scope": found.scope, "version": found.version, "ok": ok,
                "seconds": round(seconds, 3)})
    except Exception:
        pass


def _project_dir(t: Target | SeedTarget) -> Path | None:
    """The state dir whose project commands apply: None for a seed (global only)."""
    if isinstance(t, SeedTarget):
        return None
    return t.ctx.require_initialized().state_dir


def _invoke(name: str, args: tuple, kwargs: dict[str, Any]) -> Any:
    scope = kwargs.pop("scope", None)
    if scope is not None:
        _commands.validate_scope(scope)
    t = resolve()
    seed = isinstance(t, SeedTarget)
    if seed and scope == "project":
        raise ArbError(SEED_GLOBAL_ONLY)
    state_dir = _project_dir(t)
    found = _commands.find(name, state_dir)
    seen = None if found is None else (str(found.path), found.sha256)
    watching = _watch(name, state_dir, seen)  # armed first: a broken file is fixed later
    if found is None:
        return _request(t, "command_new", name, scope or ("global" if seed else "project"),
                        _commands.description_of(args, kwargs), args, kwargs,
                        watching=watching)
    func = _commands.load(found)
    with Ledger(t.state_dir) as ledger:
        idea = _owner(t, ledger)
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

    def _found(self) -> tuple[Target | SeedTarget, _commands.CommandFile | None]:
        t = resolve()
        return t, _commands.find(self.name, _project_dir(t))

    def revise(self, text: str) -> Pending:
        """Ask Claude to change this command (a command_revise todo)."""
        try:
            t, found = self._found()
        except Exception as exc:
            return Pending(self.name, "command_revise", None, error=str(exc))
        if found is None:
            scope = "global" if isinstance(t, SeedTarget) else "project"
            return _request(t, "command_new", self.name, scope, text)
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


class CommandList(list):
    """`lab.commands()` outside marimo: the rows, printed one command per line."""

    def __repr__(self) -> str:
        if not self:
            return "还没有命令（在 notebook 里调用 lab.<名字>(...) 就会交给 Claude 写）"
        width = max(len(r["name"]) for r in self)
        lines = []
        for r in self:
            line = (f"{r['name']:<{width}}  {_commands.SCOPE_LABELS[r['scope']]} · "
                    f"v{r['version']}" + (f" · {r['description']}" if r["description"] else ""))
            lines.append(line + (f"  ⚠ {r['problem']}" if r["problem"] else ""))
        return "\n".join(lines)

    __str__ = __repr__


def commands() -> Any:
    """The command library: name, scope, version, description. In marimo a table, else
    a list of dicts that prints one command per line."""
    try:
        state_dir: Path | None = _project_dir(resolve())
    except ArbError:
        state_dir = None
    rows = [{k: r[k] for k in ("name", "scope", "version", "description", "problem")}
            for r in _commands.discover(state_dir) if r["active"]]
    if not _in_marimo_notebook():
        return CommandList(rows)
    problems = any(r["problem"] for r in rows)
    return _table([{"命令": r["name"], "范围": _commands.SCOPE_LABELS[r["scope"]],
                    "版本": r["version"], "说明": r["description"]}
                   | ({"问题": r["problem"] or ""} if problems else {}) for r in rows])


def __getattr__(name: str) -> Command:
    """Any other lowercase name is a notebook command (`lab.svd(...)`)."""
    if not _commands.NAME_RE.match(name):
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    return Command(name)
