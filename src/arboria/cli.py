"""`arb` command-line interface."""

from __future__ import annotations

import functools
import json
import sys
from pathlib import Path
from typing import Annotated, Any, Callable, Optional

import typer

from . import hook as hook_mod
from . import ideas as ideas_mod
from . import install as install_mod
from . import records, report
from .db import Ledger
from .errors import ArbError
from .paths import RepoContext, locate
from .state import init_state

app = typer.Typer(
    name="arb",
    help="Arboria: local ledger of ML research ideas, runs and notes.",
    no_args_is_help=True,
    add_completion=False,
    pretty_exceptions_enable=False,
)
idea_app = typer.Typer(help="Create, list and close research ideas.", no_args_is_help=True)
run_app = typer.Typer(help="Record experiment runs.", no_args_is_help=True)
hook_app = typer.Typer(help="Claude Code hooks.", no_args_is_help=True)
app.add_typer(idea_app, name="idea")
app.add_typer(run_app, name="run")
app.add_typer(hook_app, name="hook")

JsonOpt = Annotated[bool, typer.Option("--json", help="Print machine-readable JSON.")]
ConfidenceOpt = Annotated[
    Optional[str], typer.Option("--confidence", help="guess | prelim | confirmed | rejected")
]


def handle_errors(fn: Callable[..., Any]) -> Callable[..., Any]:
    """Turn ArbError into `error: ...` on stderr and exit status 1."""

    @functools.wraps(fn)
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        try:
            return fn(*args, **kwargs)
        except ArbError as exc:
            typer.echo(f"error: {exc}", err=True)
            raise typer.Exit(1)

    return wrapper


def _warn(msg: str) -> None:
    typer.echo(f"warning: {msg}", err=True)


def _echo_json(obj: Any) -> None:
    typer.echo(json.dumps(obj, indent=2, ensure_ascii=False, default=str))


def _open() -> tuple[RepoContext, Ledger]:
    ctx = locate().require_initialized()
    return ctx, Ledger(ctx.state_dir)


# ---------------------------------------------------------------- init


@app.command()
@handle_errors
def init() -> None:
    """Set up arboria for this repository (idempotent)."""
    ctx = locate()
    actions = init_state(ctx)
    for a in actions:
        typer.echo(a)
    typer.echo(f"arboria ready: state in {ctx.state_dir}" if actions else "already initialized")


# ---------------------------------------------------------------- ideas


@idea_app.command("new")
@handle_errors
def idea_new(
    name: Annotated[str, typer.Argument(help="Slug: lowercase letters, digits, '-'.")],
    hypothesis: Annotated[str, typer.Option("--hypothesis", help="What you expect and why (≤300 chars).")],
    metric: Annotated[str, typer.Option("--metric", help="How success is measured (≤200 chars).")],
    parent: Annotated[Optional[str], typer.Option("--parent", help="Branch off this open idea.")] = None,
) -> None:
    """Create branch idea/NAME and its worktree folder; prints the worktree path."""
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.new_idea(ctx, ledger, name, hypothesis, metric, parent)
    typer.echo(idea["worktree_path"])


@idea_app.command("list")
@handle_errors
def idea_list(as_json: JsonOpt = False) -> None:
    """List all ideas."""
    _, ledger = _open()
    with ledger:
        rows = ideas_mod.list_ideas(ledger)
    if as_json:
        _echo_json(rows)
        return
    if not rows:
        typer.echo("no ideas yet")
        return
    for r in rows:
        parent = f" (from {r['parent_name']})" if r["parent_name"] else ""
        typer.echo(f"{r['name']:<24} {r['status']:<7} {r['confidence']:<9} runs:{r['run_count']:<3} "
                   f"{r['hypothesis'][:60]}{parent}")


@idea_app.command("close")
@handle_errors
def idea_close(
    name: Annotated[Optional[str], typer.Argument(help="Defaults to the idea of the current directory.")] = None,
    merge: Annotated[bool, typer.Option("--merge", help="Merge idea/NAME into main's branch.")] = False,
    drop: Annotated[bool, typer.Option("--drop", help="Archive a final snapshot and delete the branch.")] = False,
    confidence: ConfidenceOpt = None,
    allow_large: Annotated[bool, typer.Option("--allow-large", help="Snapshot large untracked files anyway.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Close an idea with --merge or --drop; removes its worktree and branch."""
    if merge == drop:
        raise ArbError("choose exactly one of --merge or --drop")
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.close_idea(ctx, ledger, name, "merge" if merge else "drop", confidence,
                                    allow_large=allow_large, warn=_warn)
    if as_json:
        _echo_json(idea)
        return
    info = idea["close"]
    if merge:
        typer.echo(f"merged {idea['branch']} into {info['merged_into']}; removed {idea['worktree_path']}")
    else:
        typer.echo(f"dropped {idea['name']}: final snapshot {info['final_commit'][:12]} "
                   f"tagged {info['tag']}; removed {idea['worktree_path']}")


# ---------------------------------------------------------------- runs


@run_app.command("new")
@handle_errors
def run_new(
    reason: Annotated[str, typer.Option("--reason", help="What changed vs. the previous run and why (≤300 chars).")],
    allow_large: Annotated[bool, typer.Option("--allow-large", help="Snapshot large untracked files anyway.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Snapshot the current idea worktree as a draft run; prints the run id."""
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.require_current_idea(ctx, ledger)
        run = records.new_run(ledger, idea, reason, allow_large=allow_large, warn=_warn)
    if as_json:
        _echo_json(run)
    else:
        typer.echo(run["id"])


@run_app.command("summary")
@handle_errors
def run_summary(
    run_id: Annotated[str, typer.Argument(help="Run id, e.g. r001.")],
    text: Annotated[str, typer.Argument(help="Result summary (≤10 lines, ≤1500 chars).")],
) -> None:
    """Set a run's result summary (run must belong to the current idea)."""
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.require_current_idea(ctx, ledger)
        records.set_run_summary(ledger, idea, run_id, text)
    typer.echo(f"summary saved for {run_id}")


# ---------------------------------------------------------------- notes / status / export


@app.command()
@handle_errors
def note(
    text: Annotated[str, typer.Argument(help="The note (≤1000 chars).")],
    run: Annotated[Optional[str], typer.Option("--run", help="Attach to this run id.")] = None,
    confidence: ConfidenceOpt = None,
) -> None:
    """Add a note to the current idea (--confidence also updates the idea)."""
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.require_current_idea(ctx, ledger)
        n = records.add_note(ledger, idea, text, run, confidence)
    suffix = f"; confidence now {confidence}" if confidence else ""
    typer.echo(f"note {n['id']} added to {idea['name']}{suffix}")


@app.command()
@handle_errors
def status(
    as_json: JsonOpt = False,
    brief: Annotated[bool, typer.Option("--brief", help="Compact text (as used by the hook).")] = False,
) -> None:
    """Current idea (inside an idea worktree) or an overview of open ideas."""
    ctx, ledger = _open()
    with ledger:
        data = report.status_data(ctx, ledger)
    if as_json:
        _echo_json(data)
    else:
        typer.echo(report.format_status(data, brief=brief))


@app.command()
@handle_errors
def export(
    out: Annotated[Optional[Path], typer.Option("--out", help="Write to FILE instead of stdout.")] = None,
) -> None:
    """Markdown of all ideas, notes and runs (newest first)."""
    ctx, ledger = _open()
    with ledger:
        md = report.export_markdown(ctx, ledger)
    if out is None:
        typer.echo(md, nl=False)
    else:
        out.write_text(md, encoding="utf-8")
        typer.echo(f"wrote {out}")


# ---------------------------------------------------------------- Claude Code integration


@hook_app.command("session-start")
def hook_session_start() -> None:
    """SessionStart hook: reads hook JSON on stdin, prints additionalContext JSON."""
    code = hook_mod.session_start(sys.stdin, sys.stdout)
    raise typer.Exit(code)


@app.command("install-claude")
@handle_errors
def install_claude(
    main: Annotated[bool, typer.Option("--main", help="Write into the main worktree instead.")] = False,
    user: Annotated[bool, typer.Option("--user", help="Write into ~/.claude/settings.json instead.")] = False,
) -> None:
    """Add the SessionStart hook and permission rules to .claude/settings.json."""
    if main and user:
        raise ArbError("use only one of --main / --user")
    if user:
        root = Path.home()
    else:
        ctx = locate()
        root = ctx.main if main else ctx.toplevel
        if root is None:
            raise ArbError("not inside a worktree")
    path, changes = install_mod.install(root)
    if changes:
        for c in changes:
            typer.echo(c)
        typer.echo(f"updated {path}")
    else:
        typer.echo(f"{path} already up to date")
