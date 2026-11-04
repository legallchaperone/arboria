"""Seeds (种子): ideas that do not belong to any repo yet, kept in the global home.

A seed has a name (same slug rules as ideas), a one-line gist, optionally a hypothesis
and a metric, and its own marimo notebook `<home>/seeds/<name>.py`. Everything is the
human's: seeds are written through `arb seed ...` (or `lab.metric` in the seed notebook),
Claude may read the notebooks but never edits them. When the idea is ready, the human
plants it into a repo: `arb seed plant` creates a normal idea (branch, worktree,
notebook) there through the same code path as `arb idea new`.

The home (paths.Home, default `~/arboria/`) is not a git repo. Its `.arboria/` is a
state dir like a project's: the same `db.Ledger` (locked writes, events, schema), the
same backup repo (`init_backup_repo`, backup commits after seed events). Only the
RepoContext side (worktrees, snapshots, arboria.toml) does not apply, and nothing here
needs it. The seed notebooks are versioned by copying them into `.arboria/seeds/` before
each backup commit (db.mirror_seeds), so the visible files stay plain files that any
editor can save.
"""

from __future__ import annotations

import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

from . import ideas, notebooks
from .db import SEED_STATUS, Ledger, init_backup_repo, now, validate_name, validate_text
from .errors import ArbError
from .paths import Home, RepoContext, home

FIELDS = ("gist", "hypothesis", "metric")
STATUS_LABELS = {"thinking": "思考中", "planted": "已种下", "dropped": "已放弃"}
HUMAN_ONLY = "{command} 只由人在自己的终端里做（会{what}）；Claude 不能运行它"
PLANT_NEEDS = ("种下需要假设和指标：种子 {name} 还没有{missing}。"
               "用 --hypothesis / --metric 给出，或先 arb seed set {name} --hypothesis …")


# ---------------------------------------------------------------- the home


def init_home(h: Home) -> list[str]:
    """Idempotently create the home: dirs, backup repo, ledger. Returns the actions."""
    actions: list[str] = []
    if not h.root.exists():
        actions.append(f"created {h.root}")
    h.seeds_dir.mkdir(parents=True, exist_ok=True)
    h.mirror_dir.mkdir(parents=True, exist_ok=True)  # also marks the state dir as a home's
    had_db = h.db_path.exists()
    init_backup_repo(h.state_dir)
    with Ledger(h.state_dir) as ledger:
        if not had_db:
            actions.append(f"created ledger {h.db_path}")
        ledger.backup("init home")
    return actions


def ensure_home(echo: Callable[[str], None] | None = None, h: Home | None = None) -> Home:
    """The home, created on first use (one line through `echo` says where)."""
    h = h or home()
    if not h.initialized:
        init_home(h)
        if echo:
            echo(f"种子园建在 {h.root}（ARBORIA_HOME 可以改位置）")
    return h


def open_home(echo: Callable[[str], None] | None = None) -> tuple[Home, Ledger]:
    h = ensure_home(echo)
    return h, Ledger(h.state_dir)


# ---------------------------------------------------------------- reads


def notebook_mtime(h: Home, name: str) -> str | None:
    try:
        st = h.notebook(name).stat()
    except OSError:
        return None
    return datetime.fromtimestamp(st.st_mtime, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def plantings(ledger: Ledger, seed_id: int) -> list[dict[str, Any]]:
    return ledger.all("SELECT repo, project_id, idea, created_at FROM plantings"
                      " WHERE seed_id = ? ORDER BY id", (seed_id,))


def _decorate(h: Home, ledger: Ledger, row: dict[str, Any]) -> dict[str, Any]:
    """The row plus its notebook, last edit (ledger or notebook file, whichever is later)
    and where it was planted."""
    edited = max(filter(None, (row["updated_at"], notebook_mtime(h, row["name"]))))
    return row | {"notebook": str(h.notebook(row["name"])), "last_edited": edited,
                  "planted_to": plantings(ledger, row["id"])}


def get_seed(ledger: Ledger, name: str) -> dict[str, Any]:
    row = ledger.one("SELECT * FROM seeds WHERE name = ?", (name,))
    if row is None:
        raise ArbError(f"no seed named {name!r} (see `arb seed list`)")
    return row


def show(h: Home, ledger: Ledger, name: str) -> dict[str, Any]:
    return _decorate(h, ledger, get_seed(ledger, name))


def list_seeds(h: Home, ledger: Ledger, include_dropped: bool = False) -> list[dict[str, Any]]:
    """Seeds, most recently edited first; dropped ones only with `include_dropped`."""
    rows = [_decorate(h, ledger, r) for r in ledger.all("SELECT * FROM seeds ORDER BY id")]
    if not include_dropped:
        rows = [r for r in rows if r["status"] != "dropped"]
    return sorted(rows, key=lambda r: (r["last_edited"], r["id"]), reverse=True)


# ---------------------------------------------------------------- writes


def _fields(gist: str | None, hypothesis: str | None, metric: str | None) -> dict[str, Any]:
    return {"gist": validate_text("gist", gist, required=False),
            "hypothesis": validate_text("hypothesis", hypothesis, required=False),
            "metric": validate_text("metric", metric, required=False)}


def new_seed(h: Home, ledger: Ledger, name: str, gist: str | None = None,
             hypothesis: str | None = None, metric: str | None = None) -> dict[str, Any]:
    """Record the seed and write its notebook (an existing file is left as is)."""
    validate_name(name, "seed")
    values = _fields(gist, hypothesis, metric)
    if ledger.one("SELECT id FROM seeds WHERE name = ?", (name,)):
        raise ArbError(f"a seed named {name!r} already exists (see `arb seed show {name}`)")
    path = h.notebook(name)
    existed = path.exists()
    ts = now()
    with ledger.write() as conn:
        cur = conn.execute(
            "INSERT INTO seeds (name, gist, hypothesis, metric, created_at, updated_at)"
            " VALUES (?, ?, ?, ?, ?, ?)",
            (name, values["gist"], values["hypothesis"], values["metric"], ts, ts))
        row = ledger.one("SELECT * FROM seeds WHERE id = ?", (cur.lastrowid,))
        if not existed:
            notebooks.write_new(path, notebooks.render_seed(row))
        ledger.event("seed_new", name, {"notebook": str(path.relative_to(h.root)),
                                        "kept_existing_notebook": existed})
    return show(h, ledger, name) | {"kept_existing_notebook": existed}


def set_fields(ledger: Ledger, name: str, **changes: str | None) -> dict[str, Any]:
    """Change gist / hypothesis / metric (None: leave as is; "" clears gist/hypothesis/metric).
    A no-op when nothing changes."""
    seed = get_seed(ledger, name)
    unknown = set(changes) - set(FIELDS)
    if unknown:
        raise ArbError(f"unknown seed field(s): {', '.join(sorted(unknown))}")
    new = {}
    for field, value in changes.items():
        if value is None:
            continue
        value = validate_text(field, value, required=False)
        if value != seed[field]:
            new[field] = value
    if not new:
        return seed
    with ledger.write() as conn:
        sets = ", ".join(f"{k} = ?" for k in new)
        conn.execute(f"UPDATE seeds SET {sets}, updated_at = ? WHERE id = ?",
                     (*new.values(), now(), seed["id"]))
        ledger.event("seed_set", name, {"fields": sorted(new)})
    return get_seed(ledger, name)


def require_human(command: str, what: str) -> None:
    from .human import in_claude_code

    if in_claude_code():
        raise ArbError(HUMAN_ONLY.format(command=command, what=what))


def drop(ledger: Ledger, name: str) -> dict[str, Any]:
    """Mark a seed dropped (its notebook stays)."""
    seed = get_seed(ledger, name)
    if seed["status"] == "dropped":
        raise ArbError(f"seed {name!r} is already dropped")
    with ledger.write() as conn:
        conn.execute("UPDATE seeds SET status = 'dropped', updated_at = ? WHERE id = ?",
                     (now(), seed["id"]))
        ledger.event("seed_drop", name, {"was": seed["status"]})
    return get_seed(ledger, name)


# ---------------------------------------------------------------- plant


def plant(h: Home, home_ledger: Ledger, name: str, ctx: RepoContext, *,
          idea_name: str | None = None, hypothesis: str | None = None,
          metric: str | None = None, again: bool = False,
          warn: Callable[[str], None] | None = None) -> dict[str, Any]:
    """Create an idea in the repo `ctx` from the seed (as `arb idea new` does); record
    the planting in both ledgers. Returns the new idea (+ `notebook`)."""
    seed = get_seed(home_ledger, name)
    if seed["status"] == "dropped":
        raise ArbError(f"seed {name!r} is dropped; plant another one (`arb seed new`)")
    done = plantings(home_ledger, seed["id"])
    if done and not again:
        where = "、".join(f"{p['repo']} · {p['idea']}" for p in done)
        raise ArbError(f"seed {name!r} is already planted ({where}); "
                       "add --again to plant it into another repo too")
    if not ctx.initialized:
        raise ArbError(f"the repo {ctx.main} is not set up for arboria yet; "
                       f"run `arb init` there first")
    hyp = hypothesis if hypothesis is not None else seed["hypothesis"]
    met = metric if metric is not None else seed["metric"]
    missing = [label for label, v in (("假设", hyp), ("指标", met)) if not (v or "").strip()]
    if missing:
        raise ArbError(PLANT_NEEDS.format(name=name, missing="和".join(missing)))
    target = idea_name or name
    path = h.notebook(name)
    try:
        markdown = notebooks.seed_markdown(path.read_text(encoding="utf-8"), name)
    except (OSError, UnicodeDecodeError):
        markdown = []
    origin = {"name": name, "notebook": str(path), "markdown": markdown, "home": str(h.root)}
    from .config import project_config

    project_id = project_config(ctx).project_id
    with Ledger(ctx.state_dir) as ledger:
        idea = ideas.new_idea(ctx, ledger, target, hyp, met, warn=warn, seed=origin)
    try:
        with home_ledger.write() as conn:
            conn.execute("INSERT INTO plantings (seed_id, repo, project_id, idea, created_at)"
                         " VALUES (?, ?, ?, ?, ?)",
                         (seed["id"], str(ctx.main), project_id, target, now()))
            conn.execute("UPDATE seeds SET status = 'planted', updated_at = ? WHERE id = ?",
                         (now(), seed["id"]))
            home_ledger.event("seed_plant", name, {"repo": str(ctx.main),
                                                   "project": project_id, "idea": target,
                                                   "markdown_cells": len(markdown)})
    except Exception as exc:  # the idea exists; the home record is what failed
        if warn:
            warn(f"idea {target} was created, but recording the planting in {h.state_dir} "
                 f"failed: {exc}")
    return idea | {"seed": name, "markdown_cells": len(markdown)}


def counts(ledger: Ledger) -> dict[str, int]:
    rows = ledger.all("SELECT status, COUNT(*) AS n FROM seeds GROUP BY status")
    out = {s: 0 for s in SEED_STATUS}
    out.update({r["status"]: r["n"] for r in rows})
    return out


def home_of(cwd: Path | str | None = None) -> Home | None:
    """The home if `cwd` (default: the process cwd) is inside it."""
    h = home()
    return h if h.contains(cwd if cwd is not None else os.getcwd()) else None
