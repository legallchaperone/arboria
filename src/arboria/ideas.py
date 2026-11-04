"""Ideas: one git branch + worktree folder per research idea."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable

from . import gitutil, notebooks, snapshot
from .db import CONFIDENCE, Ledger, now, validate_choice, validate_name, validate_text
from .errors import ArbError
from .paths import RepoContext


def branch_for(name: str) -> str:
    return f"idea/{name}"


def worktree_for(ctx: RepoContext, name: str) -> Path:
    return ctx.main.parent / f"{ctx.main.name}-{name}"


def get_idea(ledger: Ledger, name: str) -> dict[str, Any]:
    idea = ledger.one("SELECT * FROM ideas WHERE name = ?", (name,))
    if idea is None:
        raise ArbError(f"no idea named {name!r} (see `arb idea list`)")
    return idea


def current_idea(ctx: RepoContext, ledger: Ledger) -> dict[str, Any] | None:
    """The idea whose worktree contains the current directory, if any."""
    if ctx.toplevel is None:
        return None
    return ledger.one(
        "SELECT * FROM ideas WHERE worktree_path = ? ORDER BY id DESC LIMIT 1",
        (str(ctx.toplevel),),
    )


def require_current_idea(ctx: RepoContext, ledger: Ledger) -> dict[str, Any]:
    idea = current_idea(ctx, ledger)
    if idea is None:
        raise ArbError(
            "the current directory is not an idea worktree; cd into one "
            "(see `arb idea list`) or create one with `arb idea new`"
        )
    if idea["status"] != "open":
        raise ArbError(f"idea {idea['name']!r} is {idea['status']}")
    return idea


def new_idea(
    ctx: RepoContext,
    ledger: Ledger,
    name: str,
    hypothesis: str,
    metric: str,
    parent: str | None = None,
    *,
    from_run: dict[str, Any] | None = None,
    warn: Callable[[str], None] | None = None,
    seed: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Create branch idea/NAME + worktree + notebook and record the idea.

    With `from_run`, the branch starts at that run's snapshot commit and the run's idea
    is the parent (it may be closed: the snapshot outlives the branch). `seed` (from
    `arb seed plant`): name, notebook, markdown and home of the seed it grew from; the
    notebook header links back to it and the idea_new event records it."""
    validate_name(name)
    hypothesis = validate_text("hypothesis", hypothesis)
    metric = validate_text("metric", metric)
    if ledger.one("SELECT id FROM ideas WHERE name = ?", (name,)):
        raise ArbError(f"an idea named {name!r} already exists (names are never reused)")

    branch = branch_for(name)
    parent_row = None
    if from_run is not None:
        parent_row = ledger.one("SELECT * FROM ideas WHERE id = ?", (from_run["idea_id"],))
        if parent and parent != parent_row["name"]:
            raise ArbError(f"run {from_run['id']} belongs to {parent_row['name']}, not {parent}")
        base = from_run["snapshot_commit"]
        if gitutil.rev_parse(base, ctx.main) is None:
            raise ArbError(f"snapshot {base[:12]} of {from_run['id']} is missing from the repo")
    elif parent:
        parent_row = get_idea(ledger, parent)
        if parent_row["status"] != "open":
            raise ArbError(f"parent idea {parent!r} is {parent_row['status']}; its branch is gone")
        base = gitutil.rev_parse(parent_row["branch"], ctx.main)
        if base is None:
            raise ArbError(f"parent branch {parent_row['branch']} does not exist")
    else:
        base = gitutil.rev_parse("HEAD", ctx.main)
        if base is None:
            raise ArbError(f"the main worktree {ctx.main} has no commits yet; commit something first")

    if gitutil.ref_exists(f"refs/heads/{branch}", ctx.main):
        raise ArbError(f"branch {branch} already exists; pick another name")
    path = worktree_for(ctx, name)
    if path.exists():
        raise ArbError(f"{path} already exists; pick another name or move it away")

    gitutil.run(["worktree", "add", "-q", "-b", branch, str(path), base], ctx.main)
    path = path.resolve()
    try:
        with ledger.write() as conn:
            cur = conn.execute(
                "INSERT INTO ideas (name, hypothesis, metric, parent_id, branch, worktree_path, created_at)"
                " VALUES (?, ?, ?, ?, ?, ?, ?)",
                (name, hypothesis, metric, parent_row["id"] if parent_row else None,
                 branch, str(path), now()),
            )
            row = ledger.one("SELECT * FROM ideas WHERE id = ?", (cur.lastrowid,))
            nb = notebooks.create(ctx, row, parent_row["name"] if parent_row else None,
                                  from_run, warn=warn, seed=seed)
            origin = {"seed": seed["name"], "seed_home": seed.get("home")} if seed else {}
            ledger.event("idea_new", name, {
                "branch": branch, "base": base,
                "parent": parent_row["name"] if parent_row else None,
                "from_run": from_run["id"] if from_run else None,
                "notebook": str(nb.relative_to(ctx.state_dir))} | origin)
    except BaseException:
        gitutil.run(["worktree", "remove", "--force", str(path)], ctx.main, check=False)
        gitutil.run(["branch", "-D", branch], ctx.main, check=False)
        nb_path = notebooks.notebook_path(ctx.state_dir, name)
        if nb_path.exists() and ledger.one("SELECT id FROM ideas WHERE name = ?", (name,)) is None:
            nb_path.unlink()
        raise
    return get_idea(ledger, name) | {"notebook": str(notebooks.notebook_path(ctx.state_dir, name))}


def ensure_notebook(ctx: RepoContext, ledger: Ledger, name: str | None,
                    warn: Callable[[str], None] | None = None) -> tuple[dict[str, Any], Path, bool]:
    """Create the notebook (and, for an open idea, the worktree link) of an idea that has
    none; (idea, notebook path, created). An existing notebook is never touched."""
    if name is None:
        idea = current_idea(ctx, ledger)
        if idea is None:
            raise ArbError("not inside an idea worktree; pass the idea NAME")
    else:
        idea = get_idea(ledger, name)
    path = notebooks.notebook_path(ctx.state_dir, idea["name"])
    existed = path.exists()
    link = idea["status"] == "open" and Path(idea["worktree_path"]).is_dir()
    parent = ledger.one("SELECT name FROM ideas WHERE id = ?", (idea["parent_id"],)) \
        if idea["parent_id"] else None
    if existed:
        if link:  # repair a missing link; never rewrite the notebook
            notebooks.create(ctx, idea, warn=warn)
        return idea, path, False
    with ledger.write():
        notebooks.create(ctx, idea, parent["name"] if parent else None, warn=warn, link=link)
        ledger.event("notebook_new", idea["name"], {"notebook": str(path.relative_to(ctx.state_dir))})
    return idea, path, True


def list_ideas(ledger: Ledger) -> list[dict[str, Any]]:
    return ledger.all(
        "SELECT i.*, p.name AS parent_name,"
        " (SELECT COUNT(*) FROM runs r WHERE r.idea_id = i.id) AS run_count"
        " FROM ideas i LEFT JOIN ideas p ON p.id = i.parent_id ORDER BY i.id"
    )


def _is_dirty(worktree: Path) -> bool:
    return bool(gitutil.out(["status", "--porcelain", "--untracked-files=all"], worktree))


def _remove_worktree_and_branch(ctx: RepoContext, idea: dict[str, Any]) -> None:
    path = Path(idea["worktree_path"])
    if path.exists():
        gitutil.run(["worktree", "remove", "--force", str(path)], ctx.main)
    gitutil.run(["worktree", "prune"], ctx.main)
    if gitutil.ref_exists(f"refs/heads/{idea['branch']}", ctx.main):
        gitutil.run(["branch", "-D", idea["branch"]], ctx.main)


def close_idea(
    ctx: RepoContext,
    ledger: Ledger,
    name: str | None,
    mode: str,
    confidence: str | None = None,
    *,
    allow_large: bool = False,
    warn: Callable[[str], None] | None = None,
) -> dict[str, Any]:
    """Close an idea by merging its branch into main or dropping (archiving) it."""
    if mode not in ("merge", "drop"):
        raise ArbError("choose exactly one of --merge or --drop")
    validate_choice("confidence", confidence, CONFIDENCE)
    if name is None:
        idea = current_idea(ctx, ledger)
        if idea is None:
            raise ArbError("not inside an idea worktree; pass the idea NAME")
    else:
        idea = get_idea(ledger, name)
    name = idea["name"]
    if idea["status"] != "open":
        raise ArbError(f"idea {name!r} is already {idea['status']}")
    path = Path(idea["worktree_path"])
    branch = idea["branch"]
    info: dict[str, Any] = {"name": name, "mode": mode}

    if mode == "drop":
        final_ref = f"refs/arboria/ideas/{name}/final"
        tag = f"arboria/archive/{name}"
        if gitutil.ref_exists(f"refs/tags/{tag}", ctx.main):
            raise ArbError(f"tag {tag} already exists; remove it first")
        if path.exists():
            snap = snapshot.snapshot(
                path, ledger.conn, f"arboria snapshot final {name}",
                allow_large=allow_large, warn=warn,
            )
            final = snap.commit
        else:  # worktree folder deleted by hand: archive the branch tip instead
            final = gitutil.rev_parse(branch, ctx.main)
            if final is None:
                raise ArbError(f"worktree {path} and branch {branch} are both gone")
        snapshot.update_ref(ctx.main, final_ref, final, f"arboria: drop {name}")
        gitutil.run(["tag", tag, final], ctx.main)
        _remove_worktree_and_branch(ctx, idea)
        info |= {"final_commit": final, "final_ref": final_ref, "tag": tag}
        status = "dropped"
    else:
        if path.exists() and _is_dirty(path):
            raise ArbError(
                f"idea worktree {path} has uncommitted changes; commit them on {branch} "
                "(or discard them) before merging"
            )
        target = gitutil.run(["symbolic-ref", "--quiet", "--short", "HEAD"], ctx.main, check=False)
        if target.returncode != 0:
            raise ArbError(f"the main worktree {ctx.main} is on a detached HEAD; check out a branch")
        merge = gitutil.run(
            ["merge", "--no-ff", "--no-edit", "-m", f"arboria: merge idea {name}", branch],
            ctx.main, check=False,
        )
        if merge.returncode != 0:
            conflicted = gitutil.ok(["rev-parse", "--verify", "--quiet", "MERGE_HEAD"], ctx.main)
            if conflicted:
                gitutil.run(["merge", "--abort"], ctx.main, check=False)
                raise ArbError(
                    f"merging {branch} into {target.stdout} conflicts; merge aborted, nothing changed. "
                    f"Resolve by merging {target.stdout} into {branch} in the idea worktree, then retry."
                )
            detail = (merge.stderr or merge.stdout).strip()
            raise ArbError(f"git merge of {branch} into {target.stdout} failed: {detail}")
        info |= {"merged_into": target.stdout, "merge_commit": gitutil.rev_parse("HEAD", ctx.main)}
        _remove_worktree_and_branch(ctx, idea)
        status = "merged"

    with ledger.write() as conn:
        conn.execute(
            "UPDATE ideas SET status = ?, closed_at = ?, confidence = COALESCE(?, confidence)"
            " WHERE id = ?",
            (status, now(), confidence, idea["id"]),
        )
        ledger.event("idea_close", name, info | {"confidence": confidence})
    return get_idea(ledger, name) | {"close": info}
