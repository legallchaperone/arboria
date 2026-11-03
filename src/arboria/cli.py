"""`arb` command-line interface."""

from __future__ import annotations

import functools
import json
import sys
from pathlib import Path
from typing import Annotated, Any, Callable, Optional

import typer

from . import claudemd, human, slurm
from . import hook as hook_mod
from . import ideas as ideas_mod
from . import install as install_mod
from . import lessons as lessons_mod
from . import machine as machine_mod
from . import monitor, records, report, submission
from .config import project_config, require_project_id
from .db import Ledger
from .errors import ArbError, NotARepoError
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
claudemd_app = typer.Typer(help="The arb-managed block in the main CLAUDE.md.", no_args_is_help=True)
lesson_app = typer.Typer(help="Reusable project lessons (proposed by Claude, accepted by you).",
                         no_args_is_help=True)
app.add_typer(idea_app, name="idea")
app.add_typer(run_app, name="run")
app.add_typer(hook_app, name="hook")
app.add_typer(claudemd_app, name="claudemd")
app.add_typer(lesson_app, name="lesson")
remote_app = typer.Typer(help="Read-only views of a run's files on the cluster.",
                         no_args_is_help=True)
app.add_typer(remote_app, name="remote")
machine_app = typer.Typer(help="Cluster machine profiles (~/.config/arboria/clusters.toml).",
                          no_args_is_help=True)
app.add_typer(machine_app, name="machine")

JsonOpt = Annotated[bool, typer.Option("--json", help="Print machine-readable JSON.")]
ConfidenceOpt = Annotated[
    Optional[str], typer.Option("--confidence", help="guess | prelim | confirmed | rejected")
]
IDEA_CLOSE_HINT = '如果这次有可复用的教训，用 arb lesson propose "..." 提出。'
NOTE_REFUSED = ("arb note 只由人来写。Claude 的结论请写进 `arb todo done tNNN --result \"...\"`"
                "（做完一个待办时）或 `arb run summary RUN \"...\"`（一个 run 的结果）。")


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
    actions = init_state(ctx, warn=_warn)
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
    typer.echo(IDEA_CLOSE_HINT)


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


# ---------------------------------------------------------------- cluster runs

APPROVE_LINE = "由你批准：arb approve {run}"


@app.command(context_settings={"allow_interspersed_args": False})
@handle_errors
def submit(
    command: Annotated[list[str], typer.Argument(help="The training command, after `--`.",
                                                 show_default=False)],
    reason: Annotated[str, typer.Option("--reason", help="What changed vs. the previous run and why (≤300 chars).")],
    gpus: Annotated[int, typer.Option("--gpus", help="Total GPUs.")],
    time: Annotated[str, typer.Option("--time", help="Walltime: 90m, 12h, 1d6h or HH:MM:SS.")],
    nodes: Annotated[int, typer.Option("--nodes", help="Nodes (GPUs are split evenly).")] = 1,
    partition: Annotated[Optional[str], typer.Option("--partition", help="Default: the machine's default_partition.")] = None,
    machine: Annotated[Optional[str], typer.Option("--machine", help="Default: arboria.toml default_machine.")] = None,
    mem: Annotated[Optional[int], typer.Option("--mem", help="Memory per node in GB (default: GPU share; more is billed as extra GPUs).")] = None,
    cpus: Annotated[Optional[int], typer.Option("--cpus", help="CPUs per node (default: GPU share; more is billed as extra GPUs).")] = None,
    allow_large: Annotated[bool, typer.Option("--allow-large", help="Snapshot large untracked files anyway.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Snapshot the idea worktree and ask the human to approve a cluster run (local only)."""
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.require_current_idea(ctx, ledger)
        run, card = submission.submit(ctx, ledger, idea, reason, command, gpus=gpus, time=time,
                                      nodes=nodes, partition=partition, machine=machine,
                                      mem_gb=mem, cpus=cpus, allow_large=allow_large, warn=_warn)
    if as_json:
        _echo_json({"run": run, "card": card.as_dict(), "card_text": card.text()})
        return
    typer.echo(card.text())
    typer.echo(APPROVE_LINE.format(run=run["id"]))


@app.command()
@handle_errors
def card(
    run_id: Annotated[str, typer.Argument(help="Run id, e.g. r001.")],
    as_json: JsonOpt = False,
) -> None:
    """Show a submitted run's approval card, recomputed now (local only)."""
    ctx, ledger = _open()
    with ledger:
        c, matches = submission.card_for(ctx, ledger, run_id)
        status = records.get_run(ledger, run_id)["status"]
    if as_json:
        _echo_json(c.as_dict() | {"matches_submitted": matches, "status": status})
        return
    typer.echo(c.text())
    if not matches:
        typer.echo(f"⚠ {submission.CARD_CHANGED}")
    elif status == "awaiting_approval":
        typer.echo(APPROVE_LINE.format(run=run_id))
    else:
        typer.echo(f"状态：{status}")


@app.command()
@handle_errors
def approve(run_id: Annotated[str, typer.Argument(help="Run id, e.g. r001.")]) -> None:
    """(Human only, in your own terminal) approve a run's card and submit it to the
    cluster; you type the run id to confirm. Re-run to resume."""
    human.require_terminal(f"arb approve {run_id}")
    ctx, ledger = _open()
    with ledger:
        res = submission.approve(ctx, ledger, run_id, show=typer.echo)
    if res.decision_id:
        typer.echo(f"approved {run_id} (decision {res.decision_id})")
    else:
        typer.echo(f"resuming approved {run_id}")
    for segment, job_id, adopted in res.submitted:
        typer.echo(f"  s{segment}: job {job_id}" + (" (already on the cluster, adopted)" if adopted else ""))
    typer.echo(f"{run_id} is {res.run['status']}; follow it with `arb poll {run_id}` or `arb watch`")


@app.command()
@handle_errors
def reject(
    run_id: Annotated[str, typer.Argument(help="Run id, e.g. r001.")],
    reason: Annotated[Optional[str], typer.Option("--reason", help="Why (saved as a note, ≤1000 chars).")] = None,
) -> None:
    """Reject a run awaiting approval (nothing is sent to the cluster)."""
    ctx, ledger = _open()
    with ledger:
        submission.reject(ctx, ledger, run_id, reason)
    typer.echo(f"rejected {run_id}")


@app.command("cancel")
@handle_errors
def cancel_cmd(run_id: Annotated[str, typer.Argument(help="Run id, e.g. r001.")]) -> None:
    """Cancel a submitted run's jobs on the cluster."""
    ctx, ledger = _open()
    with ledger:
        run, ids = monitor.cancel(ctx, ledger, run_id)
    typer.echo(f"cancelled {run_id}" + (f" (scancel {' '.join(ids)})" if ids else " (no live jobs)"))


@app.command()
@handle_errors
def poll(
    run_id: Annotated[Optional[str], typer.Argument(help="Default: every active run.")] = None,
    as_json: JsonOpt = False,
) -> None:
    """One pass: update job states, run status and cost; fetch ARBORIA_OUT when done."""
    ctx, ledger = _open()
    require_project_id(ctx)
    outcomes = []
    with ledger:
        ids = [run_id] if run_id else [r["id"] for r in monitor.active_runs(ledger)]
        for rid in ids:
            outcomes.append(monitor.poll_run(ctx, ledger, rid))
    if as_json:
        _echo_json([o.__dict__ for o in outcomes])
        return
    if not outcomes:
        typer.echo("no active runs")
    for o in outcomes:
        typer.echo(f"{o.run_id}: {o.status}" + (f" (was {o.previous})" if o.previous != o.status else ""))
        for m in o.messages:
            typer.echo(f"  {m}")


@app.command()
@handle_errors
def watch(
    once: Annotated[bool, typer.Option("--once", help="One pass, then exit.")] = False,
    interval: Annotated[int, typer.Option("--interval", help="Seconds between passes.")] = 60,
    window: Annotated[Optional[int], typer.Option("--window", help="Observation window after a job starts, in minutes (default: arboria.toml [watch] observation_minutes).")] = None,
) -> None:
    """Keep polling active runs; in the observation window cancel on crash or a log line
    matching arboria.toml [watch] fail_patterns (default: loss NaN, CUDA OOM)."""
    if interval < 10:
        raise ArbError("--interval must be at least 10 seconds")
    if window is not None and window < 1:
        raise ArbError("--window must be at least 1 minute")
    ctx, ledger = _open()
    with ledger:
        code = monitor.watch(ctx, ledger, once=once, interval=interval,
                             window_s=None if window is None else window * 60, echo=typer.echo)
    raise typer.Exit(code)


@app.command()
@handle_errors
def logs(
    run_id: Annotated[str, typer.Argument(help="Run id, e.g. r001.")],
    segment: Annotated[Optional[int], typer.Option("--segment", help="Default: the latest started segment.")] = None,
    n: Annotated[int, typer.Option("-n", help="Number of lines.")] = 100,
) -> None:
    """Tail a run's Slurm log on the cluster (use this instead of ssh)."""
    ctx, ledger = _open()
    with ledger:
        text = monitor.logs(ctx, ledger, run_id, segment, n)
    typer.echo(text, nl=not text.endswith("\n"))


@remote_app.command("ls")
@handle_errors
def remote_ls(
    run_id: Annotated[str, typer.Argument(help="Run id, e.g. r001.")],
    subpath: Annotated[Optional[str], typer.Argument(help="Relative path inside the run dir.")] = None,
    out: Annotated[bool, typer.Option("--out", help="List ARBORIA_OUT instead of the run dir.")] = False,
) -> None:
    """List a run's directory (or its ARBORIA_OUT) on the cluster."""
    ctx, ledger = _open()
    with ledger:
        base, names = monitor.remote_ls(ctx, ledger, run_id, subpath, out)
    typer.echo(f"{base}:")
    for name in names:
        typer.echo(f"  {name}")


# ---------------------------------------------------------------- lessons


def _lesson_line(l: dict[str, Any]) -> str:
    source = l["source_id"] or "-"
    return f"{l['id']:<5} {l['status']:<9} {l['created_at'][:10]}  {source:<16} {l['text']}"


def _echo_lesson_result(result: lessons_mod.LessonResult, verb: str) -> None:
    typer.echo(f"{verb} {result.lesson['id']}")
    if result.sync is not None:
        for line in result.sync.messages():
            typer.echo(line)
    if result.warning:
        _warn(result.warning + "; then run `arb claudemd sync`")


@lesson_app.command("propose")
@handle_errors
def lesson_propose(
    text: Annotated[str, typer.Argument(help="One line, ≤200 chars: a reusable project pitfall.")],
    idea: Annotated[Optional[str], typer.Option("--idea", help="Source idea (default: current idea).")] = None,
    run: Annotated[Optional[str], typer.Option("--run", help="Source run id.")] = None,
) -> None:
    """Propose a lesson; it only goes into CLAUDE.md after the human accepts it."""
    ctx, ledger = _open()
    with ledger:
        lesson = lessons_mod.propose(ctx, ledger, text, idea=idea, run=run)
    typer.echo(f"proposed {lesson['id']} (decision {lesson['decision_id']}); "
               f"the human accepts it with `arb lesson accept {lesson['id']}`")


@lesson_app.command("list")
@handle_errors
def lesson_list(
    show_all: Annotated[bool, typer.Option("--all", help="Include retired/rejected lessons.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """List lessons (default: proposed + accepted), newest first."""
    _, ledger = _open()
    with ledger:
        rows = lessons_mod.list_lessons(ledger, include_retired=show_all)
    if as_json:
        _echo_json(rows)
        return
    if not rows:
        typer.echo("no lessons yet")
        return
    for r in rows:
        typer.echo(_lesson_line(r))


@lesson_app.command("accept")
@handle_errors
def lesson_accept(lesson_id: Annotated[str, typer.Argument(help="Lesson id, e.g. l001.")]) -> None:
    """(Human only, in your own terminal) accept a proposed lesson and update CLAUDE.md;
    you type the lesson id to confirm."""
    human.require_terminal(f"arb lesson accept {lesson_id}")
    ctx, ledger = _open()
    with ledger:
        result = lessons_mod.accept(ctx, ledger, lesson_id, show=typer.echo)
    _echo_lesson_result(result, "accepted")


@lesson_app.command("reject")
@handle_errors
def lesson_reject(lesson_id: Annotated[str, typer.Argument(help="Lesson id, e.g. l001.")]) -> None:
    """Reject a proposed lesson."""
    ctx, ledger = _open()
    with ledger:
        result = lessons_mod.reject(ctx, ledger, lesson_id)
    _echo_lesson_result(result, "rejected")


@lesson_app.command("retire")
@handle_errors
def lesson_retire(lesson_id: Annotated[str, typer.Argument(help="Lesson id, e.g. l001.")]) -> None:
    """Retire an accepted lesson and remove it from CLAUDE.md."""
    ctx, ledger = _open()
    with ledger:
        result = lessons_mod.retire(ctx, ledger, lesson_id)
    _echo_lesson_result(result, "retired")


# ---------------------------------------------------------------- notes / status / export


@app.command()
@handle_errors
def note(
    text: Annotated[str, typer.Argument(help="The note (≤1000 chars).")],
    run: Annotated[Optional[str], typer.Option("--run", help="Attach to this run id.")] = None,
    confidence: ConfidenceOpt = None,
) -> None:
    """(Human only) add a note to the current idea (--confidence also updates the idea)."""
    if human.in_claude_code():
        raise ArbError(NOTE_REFUSED)
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


# ---------------------------------------------------------------- machines


@machine_app.command("init")
@handle_errors
def machine_init(
    name: Annotated[str, typer.Argument(help="Machine name, e.g. deltaai.")],
    host: Annotated[Optional[str], typer.Option("--host", help="~/.ssh/config alias (default: the profile's host, else NAME).")] = None,
    account: Annotated[Optional[str], typer.Option("--account", help="Slurm account, if you have several.")] = None,
    write: Annotated[bool, typer.Option("--write", help="Write the profile into clusters.toml.")] = False,
    replace: Annotated[bool, typer.Option("--replace", help="With --write: rewrite an existing profile (keeps a backup).")] = False,
) -> None:
    """Measure a Slurm machine over your ssh master (read-only) and print its profile."""
    res = machine_mod.machine_init(name, host=host, account=account, write=write,
                                   replace=replace)
    for problem in res.ssh_problems:
        _warn(f"ssh: {problem}; add to ~/.ssh/config:\n"
              + machine_mod.SSH_BLOCK.format(host=res.proposal.values["host"]))
    for w in res.proposal.warnings:
        _warn(w)
    typer.echo(res.text, nl=False)
    if res.proposal.other_partitions:
        typer.echo(f"# also usable, left out: {', '.join(res.proposal.other_partitions)}")
    if res.written:
        typer.echo(f"wrote [{name}] to {res.path}"
                   + (f" (backup {res.backup.name})" if res.backup else ""))
    else:
        typer.echo(f"(not written; rerun with --write to save it to {res.path})")
    if res.still_invalid:
        _warn(f"still to fill in: {res.still_invalid}")
    elif not res.proposal.values["env_activate"]:
        _warn("env_activate is empty: decide the job environment, then set it")


@app.command()
@handle_errors
def doctor(
    name: Annotated[Optional[str], typer.Argument(help="Machine (default: arboria.toml default_machine).")] = None,
) -> None:
    """Read-only checks of a machine profile, the ssh setup and the cluster side."""
    project_id = None
    try:
        ctx = locate()
    except NotARepoError:
        ctx = None
    if ctx is not None and ctx.initialized:
        cfg = project_config(ctx)
        name = name or cfg.default_machine or None
        project_id = cfg.project_id
    if not name:
        raise ArbError("which machine? pass a name or set default_machine in arboria.toml")
    checks = machine_mod.doctor(name, project_id=project_id)
    marks = {"ok": "ok  ", "warn": "WARN", "fail": "FAIL", "info": "    "}
    for c in checks:
        typer.echo(f"{marks[c.level]} {c.what}")
    failed = sum(c.level == "fail" for c in checks)
    typer.echo(f"{failed} problem(s)" if failed else "all checks passed")
    raise typer.Exit(1 if failed else 0)


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
    ctx: RepoContext | None = None
    if user:
        root = Path.home()
        try:
            ctx = locate()
        except ArbError:  # --user works outside any repository
            ctx = None
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
    _sync_claudemd_after(ctx, quiet_if_unchanged=True)


def _sync_claudemd_after(ctx: RepoContext | None, *, quiet_if_unchanged: bool = False) -> None:
    """Sync CLAUDE.md as a follow-up step; a failure only warns (the main action succeeded)."""
    if ctx is None or not ctx.initialized:
        return
    try:
        with Ledger(ctx.state_dir) as ledger:
            result = claudemd.sync(ctx, ledger)
    except ArbError as exc:
        _warn(f"CLAUDE.md not updated: {exc}")
        return
    if result.changed or not quiet_if_unchanged:
        for line in result.messages():
            typer.echo(line)


@claudemd_app.command("sync")
@handle_errors
def claudemd_sync(
    no_commit: Annotated[bool, typer.Option("--no-commit", help="Write CLAUDE.md but do not commit it.")] = False,
) -> None:
    """Write the arboria block (rules + accepted lessons) into the main worktree's CLAUDE.md."""
    ctx, ledger = _open()
    with ledger:
        result = claudemd.sync(ctx, ledger, commit=not no_commit)
    for line in result.messages():
        typer.echo(line)
