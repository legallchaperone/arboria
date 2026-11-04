"""`arb` command-line interface."""

from __future__ import annotations

import functools
import json
import sys
from pathlib import Path
from typing import Annotated, Any, Callable, Optional

import typer

from . import claudemd, human, slurm
from . import commands as commands_mod
from . import hook as hook_mod
from . import ideas as ideas_mod
from . import install as install_mod
from . import lessons as lessons_mod
from . import machine as machine_mod
from . import monitor, records, report, submission
from . import seeds as seeds_mod
from . import tasks as tasks_mod
from .config import project_config, require_project_id
from .db import Ledger
from .errors import ArbError, NotARepoError
from .paths import RepoContext, ensure_excluded, locate
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
todo_app = typer.Typer(help="Todos from the idea notebooks (written by the human, done by Claude).",
                       no_args_is_help=True)
app.add_typer(todo_app, name="todo")
commands_app = typer.Typer(help="Notebook commands (written by Claude, called as lab.<name>(...)).",
                           no_args_is_help=True)
app.add_typer(commands_app, name="commands")
seed_app = typer.Typer(help="Seeds (种子): ideas without a repo yet, kept in the global home "
                       "(~/arboria, or $ARBORIA_HOME); work from any directory.",
                       no_args_is_help=True)
app.add_typer(seed_app, name="seed")

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
    if actions:
        typer.echo(NEXT_STEPS)


NEXT_STEPS = """
账本现在是空的，所以 arb ui 里还什么都没有。下一步：
  1. 编辑 arboria.toml：default_machine、[wandb] entity/project、[run] command，然后 commit
  2. arb install-claude          # hook、权限规则、CLAUDE.md 区块
  3. arb idea new NAME --hypothesis "..." --metric "..."
     # 建分支、worktree 和 notebook；之后看板上就有这个 idea 了
  4. 打开那个 worktree 里的 notebook.py（这个 idea 的 notebook）：VS Code 装 marimo 扩展，
     或者 marimo edit notebook.py
     marimo 默认打开时不运行 cell；想一打开就看到 lab.runs() 等，在 ~/.config/marimo/marimo.toml 里写
       [runtime]
       auto_instantiate = true"""


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
        idea = ideas_mod.new_idea(ctx, ledger, name, hypothesis, metric, parent, warn=_warn)
    typer.echo(idea["worktree_path"])
    typer.echo(f"notebook: {idea['notebook']} (linked as notebook.py; open it in VS Code "
               f"with the marimo extension, or `marimo edit notebook.py` in the worktree)",
               err=True)


@idea_app.command("notebook")
@handle_errors
def idea_notebook(
    name: Annotated[Optional[str], typer.Argument(help="Defaults to the idea of the current directory.")] = None,
) -> None:
    """Create the notebook (and notebook.py link) of an idea that has none."""
    ctx, ledger = _open()
    with ledger:
        idea, path, created = ideas_mod.ensure_notebook(ctx, ledger, name, warn=_warn)
    if created:
        typer.echo(f"created {path} (open it with `marimo edit {path}`)")
    else:
        typer.echo(f"{idea['name']} already has a notebook: {path} (left as is)")


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


# ---------------------------------------------------------------- seeds

SEED_OPEN_HINT = "打开：VS Code（marimo 扩展），或 marimo edit {path}"


def _info(msg: str) -> None:
    typer.echo(msg, err=True)


def _seed_line(s: dict[str, Any]) -> str:
    label = seeds_mod.STATUS_LABELS.get(s["status"], s["status"])
    line = f"{s['name']:<24} {label:<4} {s['last_edited'][:10]}  {s['gist'] or ''}"
    for p in s["planted_to"]:
        line += f"\n{'':<25}→ {Path(p['repo']).name} · {p['idea']}"
    return line


@seed_app.command("new")
@handle_errors
def seed_new(
    name: Annotated[str, typer.Argument(help="Slug: lowercase letters, digits, '-'.")],
    gist: Annotated[Optional[str], typer.Argument(help="The idea in one line (≤300 chars).")] = None,
    hypothesis: Annotated[Optional[str], typer.Option("--hypothesis", help="What you expect and why (≤300 chars).")] = None,
    metric: Annotated[Optional[str], typer.Option("--metric", help="How success would be measured (≤200 chars).")] = None,
) -> None:
    """(Human only) new seed and its notebook <home>/seeds/NAME.py (prints its path)."""
    seeds_mod.require_human(f"arb seed new {name}", "写一个种子")
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        seed = seeds_mod.new_seed(h, ledger, name, gist, hypothesis, metric)
    typer.echo(seed["notebook"])
    if seed["kept_existing_notebook"]:
        _warn(f"{seed['notebook']} already existed; left as is")
    _info(SEED_OPEN_HINT.format(path=seed["notebook"]))


@seed_app.command("list")
@handle_errors
def seed_list(
    show_all: Annotated[bool, typer.Option("--all", help="Include dropped seeds.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Seeds, most recently edited first (dropped ones only with --all)."""
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        rows = seeds_mod.list_seeds(h, ledger, include_dropped=show_all)
    if as_json:
        _echo_json(rows)
        return
    if not rows:
        typer.echo('no seeds yet (arb seed new NAME "一句话想法")')
        return
    for r in rows:
        typer.echo(_seed_line(r))


@seed_app.command("show")
@handle_errors
def seed_show(name: Annotated[str, typer.Argument(help="Seed name.")],
              as_json: JsonOpt = False) -> None:
    """One seed: gist, hypothesis, metric, status, notebook, where it was planted."""
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        s = seeds_mod.show(h, ledger, name)
    if as_json:
        _echo_json(s)
        return
    typer.echo(f"{s['name']} · {seeds_mod.STATUS_LABELS[s['status']]}")
    for label, key in (("想法", "gist"), ("假设", "hypothesis"), ("指标", "metric")):
        if s[key]:
            typer.echo(f"{label}：{s[key]}")
    typer.echo(f"notebook：{s['notebook']}")
    typer.echo(f"创建 {s['created_at'][:10]} · 最近编辑 {s['last_edited'][:16].replace('T', ' ')}")
    for p in s["planted_to"]:
        typer.echo(f"种在 {p['repo']} · idea {p['idea']}（{p['created_at'][:10]}）")


@seed_app.command("set")
@handle_errors
def seed_set(
    name: Annotated[str, typer.Argument(help="Seed name.")],
    gist: Annotated[Optional[str], typer.Option("--gist", help="One line (≤300 chars); \"\" clears it.")] = None,
    hypothesis: Annotated[Optional[str], typer.Option("--hypothesis", help="≤300 chars; \"\" clears it.")] = None,
    metric: Annotated[Optional[str], typer.Option("--metric", help="≤200 chars; \"\" clears it.")] = None,
) -> None:
    """(Human only) change a seed's gist, hypothesis or metric."""
    seeds_mod.require_human(f"arb seed set {name}", "改种子")
    if gist is None and hypothesis is None and metric is None:
        raise ArbError("nothing to change: pass --gist, --hypothesis and/or --metric")
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        seeds_mod.set_fields(ledger, name, gist=gist, hypothesis=hypothesis, metric=metric)
    typer.echo(f"updated {name}")


@seed_app.command("drop")
@handle_errors
def seed_drop(name: Annotated[str, typer.Argument(help="Seed name.")]) -> None:
    """(Human only) give up on a seed; its notebook is kept."""
    seeds_mod.require_human(f"arb seed drop {name}", "放弃一个种子")
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        seeds_mod.drop(ledger, name)
    typer.echo(f"dropped {name} (notebook kept: {h.notebook(name)})")


@seed_app.command("plant")
@handle_errors
def seed_plant(
    name: Annotated[str, typer.Argument(help="Seed name.")],
    repo: Annotated[Optional[Path], typer.Option("--repo", help="Target repo (default: the repo of the current directory); must be arb-initialized.")] = None,
    as_: Annotated[Optional[str], typer.Option("--as", help="Idea name (default: the seed's name).")] = None,
    hypothesis: Annotated[Optional[str], typer.Option("--hypothesis", help="Default: the seed's.")] = None,
    metric: Annotated[Optional[str], typer.Option("--metric", help="Default: the seed's.")] = None,
    again: Annotated[bool, typer.Option("--again", help="Plant a seed that is already planted (into another repo, or under another name).")] = False,
) -> None:
    """(Human only) plant a seed into a repo as a new idea, whose notebook links back."""
    seeds_mod.require_human(f"arb seed plant {name}", "会建分支和 worktree")
    h = seeds_mod.ensure_home(_info)
    if repo is None and h.contains(Path.cwd()):
        raise ArbError("which repo? run this inside the target repo, or pass --repo PATH")
    ctx = locate(repo)
    with Ledger(h.state_dir) as ledger:
        idea = seeds_mod.plant(h, ledger, name, ctx, idea_name=as_, hypothesis=hypothesis,
                               metric=metric, again=again, warn=_warn)
    typer.echo(idea["worktree_path"])
    _info(f"planted {name} as idea {idea['name']} in {ctx.main}; notebook: {idea['notebook']} "
          f"(linked as notebook.py)")


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
    command: Annotated[Optional[list[str]], typer.Argument(
        help="The training command, after `--` (default: run.command in arboria.toml, or "
             "the --from run's command).", show_default=False)] = None,
    reason: Annotated[str, typer.Option("--reason", help="What changed vs. the previous run and why (≤300 chars).")] = ...,
    from_run: Annotated[Optional[str], typer.Option("--from", help="Reuse this run's snapshot, resources and overrides (no new snapshot).")] = None,
    set_: Annotated[Optional[list[str]], typer.Option("--set", help="Override key=value (repeatable), rendered per run.override_style.")] = None,
    gpus: Annotated[Optional[int], typer.Option("--gpus", help="Total GPUs (default with --from: the parent's).")] = None,
    time: Annotated[Optional[str], typer.Option("--time", help="Walltime: 90m, 12h, 1d6h or HH:MM:SS (default with --from: the parent's).")] = None,
    nodes: Annotated[Optional[int], typer.Option("--nodes", help="Nodes (GPUs are split evenly; default 1).")] = None,
    partition: Annotated[Optional[str], typer.Option("--partition", help="Default: the machine's default_partition.")] = None,
    machine: Annotated[Optional[str], typer.Option("--machine", help="Default: arboria.toml default_machine.")] = None,
    mem: Annotated[Optional[int], typer.Option("--mem", help="Memory per node in GB (default: GPU share; more is billed as extra GPUs).")] = None,
    cpus: Annotated[Optional[int], typer.Option("--cpus", help="CPUs per node (default: GPU share; more is billed as extra GPUs).")] = None,
    allow_large: Annotated[bool, typer.Option("--allow-large", help="Snapshot large untracked files anyway.")] = False,
    repeat: Annotated[bool, typer.Option("--repeat", help="Always a new run, even if identical to an earlier one (e.g. seed variance).")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Snapshot the idea worktree (or reuse --from's) and ask the human to approve a
    cluster run (local only). An identical earlier run is returned instead of a new one,
    unless --repeat."""
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.require_current_idea(ctx, ledger)
        overrides = [slurm.parse_override(x) for x in set_ or []]
        run, card = submission.submit(ctx, ledger, idea, reason, command or None, gpus=gpus,
                                      time=time, nodes=nodes, partition=partition,
                                      machine=machine, mem_gb=mem, cpus=cpus, from_run=from_run,
                                      overrides=overrides, allow_large=allow_large,
                                      repeat=repeat, warn=_warn)
    if as_json:
        _echo_json({"run": run, "card": card.as_dict(), "card_text": card.text(),
                    "existed": run["existed"]})
        return
    if run["existed"]:
        typer.echo(submission.EXISTS.format(run=run["id"]))
        if run["status"] != "awaiting_approval":
            typer.echo(f"状态：{run['status']}")
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


# ---------------------------------------------------------------- todos


def _todo_ledger() -> tuple[Ledger, int | None]:
    """The ledger todos live in here: the home's (cwd inside it: seed todos) or the
    project's; plus the current idea's id inside an idea worktree."""
    h = seeds_mod.home_of()
    if h is not None and h.initialized:
        return Ledger(h.state_dir), None
    ctx, ledger = _open()
    idea = ideas_mod.current_idea(ctx, ledger)
    return ledger, idea["id"] if idea else None


def _todo_line(t: dict[str, Any]) -> str:
    kind = "" if t.get("kind", "plain") == "plain" else f"[{t['kind']}] "
    line = f"{t['id']:<5} {t['status']:<7} {tasks_mod.owner_name(t):<20} {kind}{t['text']}"
    if t["result"]:
        line += f"\n      → {t['result']}"
    return line


@todo_app.command("list")
@handle_errors
def todo_list(
    show_all: Annotated[bool, typer.Option("--all", help="Include done and dropped todos.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Todos of the current idea (in an idea worktree), of every open idea, or (in the
    seed home) of the seeds."""
    ledger, idea_id = _todo_ledger()
    with ledger:
        rows = tasks_mod.list_tasks(ledger, idea_id, include_closed=show_all)
    if as_json:
        _echo_json(rows)
        return
    if not rows:
        typer.echo("no open todos" if not show_all else "no todos")
        return
    for r in rows:
        typer.echo(_todo_line(r))


@todo_app.command("done")
@handle_errors
def todo_done(
    task_id: Annotated[str, typer.Argument(help="Todo id, e.g. t001.")],
    result: Annotated[str, typer.Option("--result", help="What was done / found (≤600 chars).")],
) -> None:
    """Mark a todo done with a short result for the human."""
    ledger, _ = _todo_ledger()
    with ledger:
        tasks_mod.done(ledger, task_id, result)
    typer.echo(f"done {task_id}")


@todo_app.command("drop")
@handle_errors
def todo_drop(
    task_id: Annotated[str, typer.Argument(help="Todo id, e.g. t001.")],
    reason: Annotated[Optional[str], typer.Option("--reason", help="Why (≤600 chars).")] = None,
) -> None:
    """Drop a todo that will not be done."""
    ledger, _ = _todo_ledger()
    with ledger:
        tasks_mod.drop(ledger, task_id, reason)
    typer.echo(f"dropped {task_id}")


# ---------------------------------------------------------------- notebook commands


@commands_app.command("guide")
def commands_guide() -> None:
    """The conventions for writing a notebook command (for Claude)."""
    typer.echo(commands_mod.guide(), nl=False)


@commands_app.command("list")
@handle_errors
def commands_list(as_json: JsonOpt = False) -> None:
    """Commands of this project and the global library (project wins on a clash)."""
    state_dir = None
    try:
        ctx = locate()
        if ctx.initialized:
            state_dir = ctx.state_dir
    except NotARepoError:
        pass
    rows = commands_mod.discover(state_dir)
    if as_json:
        _echo_json(rows)
        return
    if not rows:
        typer.echo("no commands yet (a notebook call lab.<name>(...) asks Claude to write one)")
        return
    for r in rows:
        mark = "" if r["active"] else "  (shadowed by the project's)"
        problem = f"  ⚠ {r['problem']}" if r["problem"] else ""
        typer.echo(f"{r['name']:<20} {commands_mod.SCOPE_LABELS[r['scope']]:<3} v{r['version']}  "
                   f"{r['description']}{mark}{problem}")


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
    """Current idea (inside an idea worktree), an overview of open ideas, or (in the seed
    home) the seeds."""
    h = seeds_mod.home_of()
    if h is not None and h.initialized:
        with Ledger(h.state_dir) as ledger:
            if as_json:
                _echo_json({"scope": "home", "home": str(h.root),
                            "seeds": seeds_mod.list_seeds(h, ledger, include_dropped=True),
                            "todos": tasks_mod.list_tasks(ledger)})
            else:
                typer.echo(report.home_status(h, ledger, brief=brief))
        return
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


@app.command("ui")
@handle_errors
def ui(
    idea: Annotated[Optional[str], typer.Option("--idea", help="Idea to select (default: the idea of the current worktree, else the most recently active open one).")] = None,
    port: Annotated[Optional[int], typer.Option("--port", help="Port (default: marimo picks a free one).")] = None,
    home: Annotated[bool, typer.Option("--home", help="The seed home (种子园) instead; the default outside any repo.")] = False,
) -> None:
    """Open the read-only dashboard (看板): what needs you, the idea tree, SU budget; or,
    outside any repo (or with --home), the seed home (种子园).

    Runs `marimo run` on 127.0.0.1; it reads the ledger and never changes anything."""
    from .ui import launch

    if not home:
        try:
            locate()
        except NotARepoError:
            home = True
        else:
            home = seeds_mod.home_of() is not None
    if home:
        if idea:
            raise ArbError("--idea is for a project's dashboard, not the seed home")
        h = seeds_mod.ensure_home(_info)
        raise typer.Exit(launch.launch_home(h.root, port=port))
    ctx = locate().require_initialized()
    code = launch.launch(ctx.main, toplevel=ctx.toplevel, idea=idea, port=port)
    raise typer.Exit(code)


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
    home: Annotated[bool, typer.Option("--home", help="Set up the seed home (~/arboria): its .claude/settings.json and CLAUDE.md.")] = False,
) -> None:
    """Add the SessionStart hook and permission rules to .claude/settings.json (the
    machine-specific global commands dir goes into .claude/settings.local.json)."""
    if main + user + home > 1:
        raise ArbError("use only one of --main / --user / --home")
    if home:
        h = seeds_mod.ensure_home(_info)
        path, changes = install_mod.install(h.root, local=False,
                                            deny_rules=install_mod.HOME_DENY_RULES)
        for c in changes:
            typer.echo(c)
        typer.echo(f"updated {path}" if changes else f"{path} already up to date")
        result = claudemd.sync_home(h.root)
        if result.changed:
            typer.echo(f"{result.action} arboria block in {result.path}")
        return
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
    path, changes = install_mod.install(root, local=not user)
    if not user and ctx is not None:  # the local file is per machine: keep it out of git
        if ensure_excluded(ctx.common_dir, install_mod.LOCAL_EXCLUDE, install_mod.LOCAL_COMMENT):
            changes.append(f"added {install_mod.LOCAL_EXCLUDE} to "
                           f"{ctx.common_dir / 'info' / 'exclude'}")
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
