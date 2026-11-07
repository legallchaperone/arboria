"""`arb` command-line interface."""

from __future__ import annotations

import functools
import json
import sys
from pathlib import Path
from typing import Annotated, Any, Callable, Optional

import typer
import typer.core

from . import claudemd, human, slurm
from . import commands as commands_mod
from . import hook as hook_mod
from . import inspection
from . import ideas as ideas_mod
from . import install as install_mod
from . import lessons as lessons_mod
from . import machine as machine_mod
from . import monitor, records, report, submission
from . import seeds as seeds_mod
from . import tasks as tasks_mod
from . import skill as skill_mod
from . import tree as tree_mod
from .config import project_config, require_project_id
from .db import Ledger
from .errors import ArbError, NotARepoError
from .paths import RepoContext, ensure_excluded, locate
from .state import init_state

class _PanelOrder(typer.core.TyperGroup):
    """Lists the top-level commands in the order of HELP_PANELS (see the end of the file),
    so `arb --help` shows the panels, and the commands in each, in that order."""

    def list_commands(self, ctx: Any) -> list[str]:
        order = [n for names in HELP_PANELS.values() for n in names]
        return sorted(super().list_commands(ctx),
                      key=lambda n: order.index(n) if n in order else len(order))


app = typer.Typer(
    name="arb",
    help="Arboria: local ledger of ML research ideas, runs and notes.",
    cls=_PanelOrder,
    no_args_is_help=True,
    add_completion=False,
    pretty_exceptions_enable=False,
)
idea_app = typer.Typer(help="Research ideas: in a repo, a branch + worktree each; outside any "
                       "repo, seeds (ideas with no repo yet, in ~/arboria).",
                       no_args_is_help=True)
run_app = typer.Typer(help="Runs: the result summary of a finished run.", no_args_is_help=True)
hook_app = typer.Typer(help="Claude Code hooks.", no_args_is_help=True)
claudemd_app = typer.Typer(help="The arb-managed block in the main CLAUDE.md.", no_args_is_help=True)
lesson_app = typer.Typer(help="Reusable project lessons (proposed by the agent, accepted by you).",
                         no_args_is_help=True)
app.add_typer(idea_app, name="idea")
app.add_typer(run_app, name="run")
app.add_typer(hook_app, name="hook")
app.add_typer(claudemd_app, name="claudemd")
app.add_typer(lesson_app, name="lesson")
key_app = typer.Typer(help="(Human only) Touch ID approval keys of the Mac app.", no_args_is_help=True)
app.add_typer(key_app, name="key")
milestone_app = typer.Typer(help="The roadmap: your milestones (deadline, SU budget, ideas, checklist). Changes are human only.",
                            no_args_is_help=True)
app.add_typer(milestone_app, name="milestone")
remote_app = typer.Typer(help="Read-only views of a run's files on the cluster.",
                         no_args_is_help=True)
app.add_typer(remote_app, name="remote")
machine_app = typer.Typer(help="Cluster machine profiles (~/.config/arboria/clusters.toml).",
                          no_args_is_help=True)
app.add_typer(machine_app, name="machine")
todo_app = typer.Typer(help="Todos from the idea notebooks (written by you, done by the agent).",
                       no_args_is_help=True)
app.add_typer(todo_app, name="todo")
commands_app = typer.Typer(help="Notebook commands (written by the agent, called as lab.<name>(...)).",
                           no_args_is_help=True)
app.add_typer(commands_app, name="commands")
inspect_app = typer.Typer(help="读代码 todos from lab.inspect(...): the agent gives locations and "
                          "notes, arb extracts the code from git.", no_args_is_help=True)
app.add_typer(inspect_app, name="inspect")
seed_app = typer.Typer(help="Alias of the seed side of `arb idea` (works from any directory): "
                       "seeds (种子) are ideas without a repo yet, kept in ~/arboria "
                       "(or $ARBORIA_HOME).",
                       no_args_is_help=True)
app.add_typer(seed_app, name="seed", hidden=True)

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


def _done(obj: dict[str, Any], text: str, as_json: bool) -> None:
    """A write command's confirmation: `text`, or `obj` as JSON with --json."""
    if as_json:
        _echo_json(obj)
    else:
        typer.echo(text)


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
#
# `arb idea …` covers both stages of an idea. Where it acts is fixed by the current
# directory (`_idea_route`): in the seed home or outside any git repo, the seeds (ideas
# with no repo yet, ~/arboria); inside a repo, that repo's ideas, and a repo without
# `arb init` is an error as before (never a seed by accident). `arb idea plant` is the
# bridge: run inside the target repo, it turns a seed into an idea there.
# Every command that writes a seed is the human's and refuses inside Claude Code
# (`seeds.require_human`), under either name (`arb idea …` or the `arb seed …` alias).

SEED_OPEN_HINT = "打开：VS Code（marimo 扩展），或 marimo edit {path}"
SEED_RECORDED = ("不在仓库里，所以记成了种子：{path}；以后在仓库里用 "
                 "arb idea plant {name} 把它种下")
SEED_ONLY = ("arb idea {verb} 只用于种子（还没有仓库的 idea），这里是仓库：请在仓库外运行，"
             "或者用 arb seed {verb} {name}")


def _info(msg: str) -> None:
    typer.echo(msg, err=True)


def _idea_route() -> RepoContext | None:
    """The initialized repo `arb idea …` acts on, or None for the seeds (in the seed
    home, or anywhere outside a git repo). A repo without `arb init` raises."""
    if seeds_mod.home_of() is not None:
        return None
    try:
        ctx = locate()
    except NotARepoError:
        return None
    return ctx.require_initialized()


def _seed_only(verb: str, name: str) -> None:
    if _idea_route() is not None:
        raise ArbError(SEED_ONLY.format(verb=verb, name=name))


@idea_app.command("new")
@handle_errors
def idea_new(
    name: Annotated[str, typer.Argument(help="Slug: lowercase letters, digits, '-'.")],
    gist: Annotated[Optional[str], typer.Argument(help="Seeds only: the idea in one line (≤300 chars).")] = None,
    hypothesis: Annotated[Optional[str], typer.Option("--hypothesis", help="What you expect and why (≤300 chars); required in a repo.")] = None,
    metric: Annotated[Optional[str], typer.Option("--metric", help="How success is measured (≤200 chars); required in a repo.")] = None,
    parent: Annotated[Optional[str], typer.Option("--parent", help="Branch off this open idea (in a repo).")] = None,
    as_json: JsonOpt = False,
) -> None:
    """New idea: in a repo, branch idea/NAME and its worktree folder (prints its path).

    Outside any repo: (human only) a seed in ~/arboria and its notebook
    seeds/NAME.py, to plant into a repo later with `arb idea plant NAME`."""
    ctx = _idea_route()
    if ctx is None:
        if parent is not None:
            raise ArbError("--parent is for ideas in a repo; a seed has no parent")
        seed = _seed_new(name, gist, hypothesis, metric, f"arb idea new {name}")
        from .notebooks import display_path

        _info(SEED_RECORDED.format(path=display_path(seed["notebook"]), name=name))
        return
    if gist is not None:
        raise ArbError(f"unexpected argument {gist!r}: in a repo an idea takes --hypothesis "
                       "and --metric (a one-line gist is for seeds, outside any repo)")
    missing = [o for o, v in (("--hypothesis", hypothesis), ("--metric", metric)) if v is None]
    if missing:
        raise ArbError(f"missing {' and '.join(missing)}: an idea in a repo needs both")
    with Ledger(ctx.state_dir) as ledger:
        idea = ideas_mod.new_idea(ctx, ledger, name, hypothesis, metric, parent, warn=_warn)
    if as_json:
        _echo_json({"name": idea["name"], "branch": idea["branch"], "parent": parent,
                    "worktree": idea["worktree_path"], "notebook": str(idea["notebook"])})
        return
    typer.echo(idea["worktree_path"])
    typer.echo(f"notebook: {idea['notebook']} (linked as notebook.py; open it in VS Code "
               f"with the marimo extension, or `marimo edit notebook.py` in the worktree)",
               err=True)


@idea_app.command("list")
@handle_errors
def idea_list(
    show_all: Annotated[bool, typer.Option("--all", help="Seeds: include dropped ones.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """List all ideas; outside any repo, the seeds (most recently edited first)."""
    ctx = _idea_route()
    if ctx is None:
        _seed_list(show_all, as_json)
        return
    with Ledger(ctx.state_dir) as ledger:
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


@idea_app.command("show")
@handle_errors
def idea_show(
    name: Annotated[Optional[str], typer.Argument(help="Default (in a repo): the idea of the current directory.")] = None,
    as_json: JsonOpt = False,
) -> None:
    """One idea, or (outside any repo) one seed.

    An idea: hypothesis, metric, status, branch, notebook and runs. A seed: gist,
    hypothesis, metric, status, notebook, where it was planted."""
    ctx = _idea_route()
    if ctx is None:
        if name is None:
            raise ArbError("which seed? pass its NAME (see `arb idea list`)")
        _seed_show(name, as_json)
        return
    with Ledger(ctx.state_dir) as ledger:
        i = ideas_mod.show_idea(ctx, ledger, name)
    if as_json:
        _echo_json(i)
        return
    typer.echo(f"{i['name']} · {i['status']} · {i['confidence']}")
    typer.echo(f"假设：{i['hypothesis']}")
    typer.echo(f"指标：{i['metric']}")
    if i["parent_name"]:
        typer.echo(f"来自：{i['parent_name']}")
    typer.echo(f"分支：{i['branch']}" + (f" · 文件夹：{i['worktree_path']}"
                                       if i["status"] == "open" else ""))
    typer.echo(f"notebook：{i['notebook']}")
    typer.echo(f"创建 {i['created_at'][:10]}"
               + (f" · 结束 {i['closed_at'][:10]}" if i["closed_at"] else ""))
    for r in i["runs"]:
        typer.echo(f"  {r['id']:<5} {r['status']:<18} {r['change_reason'][:70]}")


@idea_app.command("set")
@handle_errors
def idea_set(
    name: Annotated[str, typer.Argument(help="Seed name.")],
    gist: Annotated[Optional[str], typer.Option("--gist", help="One line (≤300 chars); \"\" clears it.")] = None,
    hypothesis: Annotated[Optional[str], typer.Option("--hypothesis", help="≤300 chars; \"\" clears it.")] = None,
    metric: Annotated[Optional[str], typer.Option("--metric", help="≤200 chars; \"\" clears it.")] = None,
) -> None:
    """(Human only) change a seed's gist, hypothesis or metric (outside any repo)."""
    seeds_mod.require_human(f"arb idea set {name}", "改种子")
    _seed_only("set", name)
    _seed_set(name, gist, hypothesis, metric)


@idea_app.command("drop")
@handle_errors
def idea_drop(
    name: Annotated[Optional[str], typer.Argument(help="Default (in a repo): the idea of the current directory.")] = None,
    confidence: ConfidenceOpt = None,
    allow_large: Annotated[bool, typer.Option("--allow-large", help="Snapshot large untracked files anyway.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Drop an idea (= `close --drop`), or (outside any repo) a seed.

    An idea: archive a final snapshot, remove its worktree and branch. A seed (human
    only): give up on it; its notebook is kept."""
    if _idea_route() is None:
        seeds_mod.require_human(f"arb idea drop {name or 'NAME'}", "放弃一个种子")
        if name is None:
            raise ArbError("which seed? pass its NAME (see `arb idea list`)")
        if confidence is not None or allow_large:
            raise ArbError("--confidence and --allow-large are for ideas in a repo, not seeds")
        _seed_drop(name, as_json)
        return
    _close(name, "drop", confidence, allow_large, as_json)


@idea_app.command("revive")
@handle_errors
def idea_revive(name: Annotated[str, typer.Argument(help="Seed name.")]) -> None:
    """(Human only) bring a dropped seed back to thinking (outside any repo)."""
    seeds_mod.require_human(f"arb idea revive {name}", "恢复一个已放弃的种子")
    _seed_only("revive", name)
    _seed_revive(name)


@idea_app.command("plant")
@handle_errors
def idea_plant(
    name: Annotated[str, typer.Argument(help="Seed name.")],
    repo: Annotated[Optional[Path], typer.Option("--repo", help="Target repo (default: the repo of the current directory); must be arb-initialized.")] = None,
    as_: Annotated[Optional[str], typer.Option("--as", help="Idea name (default: the seed's name).")] = None,
    hypothesis: Annotated[Optional[str], typer.Option("--hypothesis", help="Default: the seed's.")] = None,
    metric: Annotated[Optional[str], typer.Option("--metric", help="Default: the seed's.")] = None,
    again: Annotated[bool, typer.Option("--again", help="Plant a seed that is already planted (into another repo, or under another name).")] = False,
) -> None:
    """(Human only) plant a seed into this repo as a new idea, whose notebook links back.

    Run it inside the target repo (or pass --repo)."""
    _seed_plant(name, repo, as_, hypothesis, metric, again, f"arb idea plant {name}")


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


STYLE_RESULT = {"added": "加上了 Arboria 字体", "already": "已经有了",
                "own": "有自己的 css_file，没动", "unknown": "找不到 marimo.App(...)，没动"}


@idea_app.command("style")
@handle_errors
def idea_style() -> None:
    """Give older notebooks the Arboria font (Maple Mono CN).

    In a repo: every idea notebook; outside any repo: every seed notebook. Refreshes
    the arboria.css next to them and adds css_file="arboria.css" to marimo.App(...)
    where it is missing; no cell is touched. Idempotent. Yours only: refused inside
    Claude Code (it writes the notebooks)."""
    seeds_mod.require_human("arb idea style", "会改 notebook 文件")
    ctx = _idea_route()
    if ctx is None:
        results = seeds_mod.restyle(seeds_mod.ensure_home(_info))
    else:
        with Ledger(ctx.state_dir) as ledger:
            results = ideas_mod.restyle(ctx, ledger, warn=_warn)
    if not results:
        typer.echo("没有 notebook")
    for path, what in results:
        typer.echo(f"{path.stem}: {STYLE_RESULT[what]}")


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
    _close(name, "merge" if merge else "drop", confidence, allow_large, as_json)


def _close(name: str | None, mode: str, confidence: str | None, allow_large: bool,
           as_json: bool) -> None:
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.close_idea(ctx, ledger, name, mode, confidence,
                                    allow_large=allow_large, warn=_warn)
    if as_json:
        _echo_json(idea)
        return
    info = idea["close"]
    if mode == "merge":
        typer.echo(f"merged {idea['branch']} into {info['merged_into']}; removed {idea['worktree_path']}")
    else:
        typer.echo(f"dropped {idea['name']}: final snapshot {info['final_commit'][:12]} "
                   f"tagged {info['tag']}; removed {idea['worktree_path']}")
    typer.echo(IDEA_CLOSE_HINT)


# ---------------------------------------------------------------- seeds
#
# The seed side of `arb idea …`, and `arb seed …`, the older spelling kept as an alias
# (hidden from --help).


def _seed_line(s: dict[str, Any]) -> str:
    label = seeds_mod.STATUS_LABELS.get(s["status"], s["status"])
    line = f"{s['name']:<24} {label:<4} {s['last_edited'][:10]}  {s['gist'] or ''}"
    for p in s["planted_to"]:
        line += f"\n{'':<25}→ {Path(p['repo']).name} · {p['idea']}"
    return line


def _seed_new(name: str, gist: str | None, hypothesis: str | None, metric: str | None,
              command: str) -> dict[str, Any]:
    seeds_mod.require_human(command, "写一个种子")
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        seed = seeds_mod.new_seed(h, ledger, name, gist, hypothesis, metric)
    typer.echo(seed["notebook"])
    if seed["kept_existing_notebook"]:
        _warn(f"{seed['notebook']} already existed; left as is")
    _info(SEED_OPEN_HINT.format(path=seed["notebook"]))
    return seed


def _seed_list(show_all: bool, as_json: bool) -> None:
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        rows = seeds_mod.list_seeds(h, ledger, include_dropped=show_all)
    if as_json:
        _echo_json(rows)
        return
    if not rows:
        typer.echo('no seeds yet (outside any repo: arb idea new NAME "一句话想法")')
        return
    for r in rows:
        typer.echo(_seed_line(r))


def _seed_show(name: str, as_json: bool) -> None:
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


def _seed_set(name: str, gist: str | None, hypothesis: str | None, metric: str | None) -> None:
    if gist is None and hypothesis is None and metric is None:
        raise ArbError("nothing to change: pass --gist, --hypothesis and/or --metric")
    _, ledger = seeds_mod.open_home(_info)
    with ledger:
        seeds_mod.set_fields(ledger, name, gist=gist, hypothesis=hypothesis, metric=metric)
    typer.echo(f"updated {name}")


def _seed_drop(name: str, as_json: bool = False) -> None:
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        seed = seeds_mod.drop(ledger, name)
    if as_json:
        _echo_json(seed)
        return
    typer.echo(f"dropped {name} (notebook kept: {h.notebook(name)})")


def _seed_revive(name: str) -> None:
    h, ledger = seeds_mod.open_home(_info)
    with ledger:
        seeds_mod.revive(ledger, name)
    typer.echo(f"revived {name}: thinking again ({h.notebook(name)})")


def _seed_plant(name: str, repo: Path | None, as_: str | None, hypothesis: str | None,
                metric: str | None, again: bool, command: str) -> None:
    seeds_mod.require_human(command, "会建分支和 worktree")
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


@seed_app.command("new")
@handle_errors
def seed_new(
    name: Annotated[str, typer.Argument(help="Slug: lowercase letters, digits, '-'.")],
    gist: Annotated[Optional[str], typer.Argument(help="The idea in one line (≤300 chars).")] = None,
    hypothesis: Annotated[Optional[str], typer.Option("--hypothesis", help="What you expect and why (≤300 chars).")] = None,
    metric: Annotated[Optional[str], typer.Option("--metric", help="How success would be measured (≤200 chars).")] = None,
) -> None:
    """(Human only) new seed and its notebook <home>/seeds/NAME.py (prints its path)."""
    _seed_new(name, gist, hypothesis, metric, f"arb seed new {name}")


@seed_app.command("list")
@handle_errors
def seed_list(
    show_all: Annotated[bool, typer.Option("--all", help="Include dropped seeds.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """Seeds, most recently edited first (dropped ones only with --all)."""
    _seed_list(show_all, as_json)


@seed_app.command("show")
@handle_errors
def seed_show(name: Annotated[str, typer.Argument(help="Seed name.")],
              as_json: JsonOpt = False) -> None:
    """One seed: gist, hypothesis, metric, status, notebook, where it was planted."""
    _seed_show(name, as_json)


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
    _seed_set(name, gist, hypothesis, metric)


@seed_app.command("drop")
@handle_errors
def seed_drop(name: Annotated[str, typer.Argument(help="Seed name.")]) -> None:
    """(Human only) give up on a seed; its notebook is kept."""
    seeds_mod.require_human(f"arb seed drop {name}", "放弃一个种子")
    _seed_drop(name)


@seed_app.command("revive")
@handle_errors
def seed_revive(name: Annotated[str, typer.Argument(help="Seed name.")]) -> None:
    """(Human only) bring a dropped seed back to thinking."""
    seeds_mod.require_human(f"arb seed revive {name}", "恢复一个已放弃的种子")
    _seed_revive(name)


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
    _seed_plant(name, repo, as_, hypothesis, metric, again, f"arb seed plant {name}")


# ---------------------------------------------------------------- runs


@run_app.command("new", hidden=True)
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
    as_json: JsonOpt = False,
) -> None:
    """Set a run's result summary (run must belong to the current idea)."""
    ctx, ledger = _open()
    with ledger:
        idea = ideas_mod.require_current_idea(ctx, ledger)
        records.set_run_summary(ledger, idea, run_id, text)
    _done({"run": run_id, "summary": "saved"}, f"summary saved for {run_id}", as_json)


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
    """Ask the human to approve a cluster run (local only: snapshot + approval card).

    Snapshots the idea worktree (or reuses --from's). An identical earlier run is
    returned instead of a new one, unless --repeat."""
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
    window: Annotated[Optional[int], typer.Option("--window", help="Observation window after a job starts, in minutes (default: watch.observation_minutes in arboria.toml).")] = None,
) -> None:
    """Keep polling active runs; cancel a run that crashes or logs a failure early on.

    In the observation window a run is cancelled on a crash or a log line matching
    watch.fail_patterns in arboria.toml (default: loss NaN, CUDA OOM)."""
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
    as_json: JsonOpt = False,
) -> None:
    """List a run's directory (or its ARBORIA_OUT) on the cluster."""
    ctx, ledger = _open()
    with ledger:
        base, names = monitor.remote_ls(ctx, ledger, run_id, subpath, out)
    if as_json:
        _echo_json({"run": run_id, "dir": base, "entries": names})
        return
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
    as_json: JsonOpt = False,
) -> None:
    """Propose a lesson; it only goes into CLAUDE.md after the human accepts it."""
    ctx, ledger = _open()
    with ledger:
        lesson = lessons_mod.propose(ctx, ledger, text, idea=idea, run=run)
    if as_json:
        _echo_json({"lesson": lesson["id"], "decision": lesson["decision_id"], "status": "proposed"})
        return
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
    as_json: JsonOpt = False,
) -> None:
    """Mark a todo done with a short result for the human."""
    ledger, _ = _todo_ledger()
    with ledger:
        tasks_mod.done(ledger, task_id, result)
    _done({"todo": task_id, "status": "done"}, f"done {task_id}", as_json)


@todo_app.command("drop")
@handle_errors
def todo_drop(
    task_id: Annotated[str, typer.Argument(help="Todo id, e.g. t001.")],
    reason: Annotated[Optional[str], typer.Option("--reason", help="Why (≤600 chars).")] = None,
    as_json: JsonOpt = False,
) -> None:
    """Drop a todo that will not be done."""
    ledger, _ = _todo_ledger()
    with ledger:
        tasks_mod.drop(ledger, task_id, reason)
    _done({"todo": task_id, "status": "dropped"}, f"dropped {task_id}", as_json)


# ---------------------------------------------------------------- notebook commands


@commands_app.command("guide")
def commands_guide() -> None:
    """The conventions for writing a notebook command (for the agent)."""
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
    builtins = ("built-in (内置, signatures in lab.commands()): "
                + ", ".join(commands_mod.BUILTINS))
    if not rows:
        typer.echo("no commands yet (a notebook call lab.<name>(..., ask=\"...\") asks Claude "
                   f"to write one)\n{builtins}")
        return
    width = min(max(len(r["signature"]) for r in rows), 48)
    for r in rows:
        mark = "" if r["active"] else "  (shadowed by the project's)"
        problem = f"  ⚠ {r['problem']}" if r["problem"] else ""
        typer.echo(f"{r['signature']:<{width}}  {commands_mod.SCOPE_LABELS[r['scope']]}  "
                   f"v{r['version']}  {r['description']}{mark}{problem}")
    typer.echo(builtins)


@commands_app.command("promote")
@handle_errors
def commands_promote(
    name: Annotated[str, typer.Argument(help="The project command to copy.")],
    force: Annotated[bool, typer.Option("--force", help="Replace a different global one.")] = False,
) -> None:
    """Copy a project command into the global library (~/.config/arboria/commands)."""
    _, ledger = _open()
    with ledger:
        res = commands_mod.promote(ledger, name, force=force)
    if not res["changed"]:
        typer.echo(f"{name} v{res['version']} 已经是全局命令库里的版本（{res['path']}）")
        return
    replaced = f"，替换了 v{res['replaced']}" if res["replaced"] else ""
    typer.echo(f"promoted {name} v{res['version']} → {res['path']}{replaced}\n"
               f"项目里的 {name} 仍然优先；别的项目和种子里 lab.{name}(...) 用全局的这份。")


# ---------------------------------------------------------------- lab.inspect (读代码)


def _answer(file: str) -> Any:
    if file == "-":
        text = sys.stdin.read()
    else:
        try:
            text = Path(file).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            raise ArbError(f"cannot read {file}: {exc}") from exc
    return inspection.load_answer(text)


FileOpt = Annotated[str, typer.Option("--file", help="The answer as JSON: a file, or - for stdin.")]


@inspect_app.command("guide")
def inspect_guide() -> None:
    """The conventions for answering a lab.inspect todo (for the agent)."""
    typer.echo(inspection.guide(), nl=False)


@inspect_app.command("source")
@handle_errors
def inspect_source(
    task_id: Annotated[str, typer.Argument(help="Todo id, e.g. t007.")],
    path: Annotated[Optional[str], typer.Argument(help="File to print with line numbers (omit to list the files).")] = None,
) -> None:
    """The code a 读代码 todo is about, at its code_ref (read from git; nothing is changed)."""
    ctx, ledger = _open()
    with ledger:
        task = inspection.open_inspect_task(ledger, task_id)
    commit = tasks_mod.payload(task)["code_ref"]
    if path is None:
        for f in inspection.list_files(ctx.main, commit):
            typer.echo(f)
        return
    lines = inspection.blob_lines(ctx.main, commit, path)
    width = len(str(len(lines)))
    typer.echo(f"# {path} @ {inspection.short(commit)} ({len(lines)} lines)")
    for i, line in enumerate(lines, 1):
        typer.echo(f"{i:>{width}}  {line}")


@inspect_app.command("check")
@handle_errors
def inspect_check(
    task_id: Annotated[str, typer.Argument(help="Todo id, e.g. t007.")],
    file: FileOpt,
    as_json: JsonOpt = False,
) -> None:
    """Validate an answer and print what the human will see; saves nothing."""
    ctx, ledger = _open()
    with ledger:
        task = inspection.open_inspect_task(ledger, task_id)
    result = inspection.validate_result(ctx.main, task, _answer(file))
    if as_json:
        _echo_json({"todo": task_id, "valid": True, "saved": False, "result": result})
        return
    typer.echo(inspection.to_text(None, result))
    typer.echo(f"\nok: valid for {task_id} (nothing saved; deliver with "
               f"`arb inspect done {task_id} --file …`)")


@inspect_app.command("done")
@handle_errors
def inspect_done(
    task_id: Annotated[str, typer.Argument(help="Todo id, e.g. t007.")],
    file: FileOpt,
    as_json: JsonOpt = False,
) -> None:
    """Deliver the answer to a 读代码 todo: arb extracts the code, stores it, closes the todo."""
    ctx, ledger = _open()
    with ledger:
        result = inspection.deliver(ledger, ctx.main, task_id, _answer(file))
    lines = sum(len(s["lines"]) for s in result["spans"])
    if as_json:
        _echo_json({"todo": task_id, "status": "done", "title": result["title"],
                    "spans": len(result["spans"]), "lines": lines, "code_ref": result["code_ref"]})
        return
    typer.echo(f"done {task_id}: {result['title']} ({len(result['spans'])} spans, {lines} lines "
               f"@ {inspection.short(result['code_ref'])}); the notebook cell re-runs by itself")


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
    """Where things stand: the current idea, the open ideas, or the seeds.

    Inside an idea worktree, that idea; elsewhere in a repo, an overview of its open
    ideas; in the seed home, the seeds."""
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
def tree(
    idea: Annotated[Optional[str], typer.Option("--idea", help="Only this idea and what grew from it.")] = None,
    depth: Annotated[Optional[int], typer.Option("--depth", min=1, help="Levels of ideas to show.")] = None,
    budget: Annotated[int, typer.Option("--budget", min=200, help="Characters before finished subtrees fold.")] = tree_mod.TREE_CHARS,
    as_json: JsonOpt = False,
) -> None:
    """The research tree: ideas and their runs, one line each (parent, change, result,
    conclusion and who wrote it).

    Built from the ledger and the fetched metrics.jsonl only; the order is stable. Over
    the budget, finished ideas off your path fold to one line."""
    ctx, ledger = _open()
    with ledger:
        here = ideas_mod.current_idea(ctx, ledger)
        t = tree_mod.build(ctx, ledger)
    name = here["name"] if here else None
    if as_json:
        _echo_json(tree_mod.to_json(t, idea=idea, depth=depth, here=name))
    else:
        typer.echo(tree_mod.render(t, idea=idea, depth=depth, here=name, budget=budget))


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
    """Open the read-only dashboard (看板).

    In a repo: what needs you, the idea tree, SU budget; outside any repo (or with
    --home), the seed home (种子园). Runs `marimo run` on 127.0.0.1; it reads the ledger and never changes anything."""
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


@key_app.command("trust")
@handle_errors
def key_trust(
    public_key: Annotated[Optional[str], typer.Option("--public-key", help="Base64 X9.63 P-256 public key (default: the one the app saved).")] = None,
    label: Annotated[str, typer.Option("--label", help="A name for this device.")] = "Mac",
) -> None:
    """(Human only) Trust the app's Touch ID key; you type the fingerprint the app shows."""
    from . import approvekeys

    pub = public_key
    if pub is None:
        path = approvekeys.APP_PUBLIC_KEY.expanduser()
        if not path.is_file():
            raise ArbError(f"no key at {path}: open the Arboria app first, or pass --public-key")
        pub = path.read_text().strip()
    entry = approvekeys.trust(pub, label)
    typer.echo(f"trusted {entry['fingerprint']} ({entry['label']})")


@key_app.command("list")
@handle_errors
def key_list(as_json: JsonOpt = False) -> None:
    """List trusted Touch ID approval keys."""
    from . import approvekeys

    keys = approvekeys.load()
    if as_json:
        _echo_json([{k: v for k, v in key.items() if k != "public_key"} for key in keys])
        return
    if not keys:
        typer.echo("no trusted approval keys")
    for key in keys:
        typer.echo(f"{key['fingerprint']}  {key['label']}  {key['added_at']}")


@key_app.command("revoke")
@handle_errors
def key_revoke(
    fingerprint: Annotated[str, typer.Argument(help="Fingerprint, as `arb key list` shows it.")],
) -> None:
    """(Human only) Stop trusting a Touch ID approval key."""
    from . import approvekeys

    hit = approvekeys.revoke(fingerprint)
    typer.echo(f"revoked {hit['fingerprint']} ({hit['label']})")


def _milestone_text(p: dict) -> list[str]:
    head = f"{p['name']} · {p['title']}"
    if p["status"] != "open":
        head += f" · {'已完成' if p['status'] == 'done' else '已放弃'}"
    lines = [head]
    when = "无截止日期" if p["deadline"] is None else (
        f"截止 {p['deadline']}（" + (f"已过 {-p['days_left']} 天" if p["days_left"] < 0
                                    else f"还有 {p['days_left']} 天") + "）")
    su = (f"已用 {p['spent_su']:g} SU" if p["su_budget"] is None
          else f"已用 {p['spent_su']:g} / {p['su_budget']:g} SU，剩余 {p['left_su']:g}")
    if p["awaiting_su"]:
        su += f"，等签字 ≈ {p['awaiting_su']:g}"
    lines.append(f"  {when} · {su}")
    if p["idea_progress"]:
        lines.append("  idea: " + "，".join(f"{i['name']}（{i['status']}，{i['runs']} 个 run，{i['su']:g} SU）"
                                          for i in p["idea_progress"]))
    for k, it in enumerate(p["items"], 1):
        lines.append(f"  [{'x' if it['done'] else ' '}] {k}. {it['text']}")
    return lines


@milestone_app.command("list")
@handle_errors
def milestone_list(
    all_: Annotated[bool, typer.Option("--all", help="Also done and dropped milestones.")] = False,
    as_json: JsonOpt = False,
) -> None:
    """The roadmap: open milestones by deadline, with spend and checklist."""
    from . import roadmap

    _, ledger = _open()
    with ledger:
        rows = roadmap.roadmap(ledger, all_=all_)
    if as_json:
        _echo_json(rows)
        return
    if not rows:
        typer.echo("no milestones (arb milestone new NAME --title … in your terminal)")
    for p in rows:
        typer.echo("\n".join(_milestone_text(p)))


@milestone_app.command("show")
@handle_errors
def milestone_show(name: Annotated[str, typer.Argument()], as_json: JsonOpt = False) -> None:
    """One milestone."""
    from . import roadmap

    _, ledger = _open()
    with ledger:
        p = roadmap.progress(ledger, roadmap.get(ledger, name))
    if as_json:
        _echo_json(p)
    else:
        typer.echo("\n".join(_milestone_text(p)))


IdeaOpt = Annotated[Optional[list[str]], typer.Option("--idea", help="An idea of this project (repeatable).")]
ItemOpt = Annotated[Optional[list[str]], typer.Option("--item", help="A checklist item (≤120 chars, repeatable).")]


@milestone_app.command("new")
@handle_errors
def milestone_new(
    name: Annotated[str, typer.Argument(help="Short name (lowercase, digits, '-').")],
    title: Annotated[str, typer.Option("--title", help="What it is (≤80 chars).")],
    deadline: Annotated[Optional[str], typer.Option("--deadline", help="YYYY-MM-DD.")] = None,
    su: Annotated[Optional[float], typer.Option("--su", help="SU budget for the runs of its ideas.")] = None,
    idea: IdeaOpt = None,
    item: ItemOpt = None,
) -> None:
    """(Human only) Add a milestone to the roadmap."""
    from . import roadmap

    _, ledger = _open()
    with ledger:
        m = roadmap.new(ledger, name, title, deadline=deadline, su_budget=su, ideas=idea or [], items=item or [])
    typer.echo(f"milestone {m.name} added")


@milestone_app.command("set")
@handle_errors
def milestone_set(
    name: Annotated[str, typer.Argument()],
    title: Annotated[Optional[str], typer.Option("--title")] = None,
    deadline: Annotated[Optional[str], typer.Option("--deadline", help="YYYY-MM-DD.")] = None,
    no_deadline: Annotated[bool, typer.Option("--no-deadline", help="Remove the deadline.")] = False,
    su: Annotated[Optional[float], typer.Option("--su", help="SU budget.")] = None,
    no_su: Annotated[bool, typer.Option("--no-su", help="Remove the SU budget.")] = False,
    add_idea: Annotated[Optional[list[str]], typer.Option("--add-idea")] = None,
    remove_idea: Annotated[Optional[list[str]], typer.Option("--remove-idea")] = None,
    add_item: Annotated[Optional[list[str]], typer.Option("--add-item")] = None,
    remove_item: Annotated[Optional[list[int]], typer.Option("--remove-item", help="Checklist item number.")] = None,
) -> None:
    """(Human only) Change a milestone."""
    from . import roadmap

    _, ledger = _open()
    with ledger:
        roadmap.update(ledger, name, title=title, deadline=deadline, clear_deadline=no_deadline,
                       su_budget=su, clear_budget=no_su, add_ideas=add_idea or [], remove_ideas=remove_idea or [],
                       add_items=add_item or [], remove_items=remove_item or [])
    typer.echo(f"milestone {name} updated")


def _milestone_check(name: str, item: int, done: bool) -> None:
    from . import roadmap

    _, ledger = _open()
    with ledger:
        m = roadmap.check(ledger, name, item, done)
    typer.echo(f"[{'x' if done else ' '}] {item}. {m.items[item - 1]['text']}")


@milestone_app.command("check")
@handle_errors
def milestone_check(name: Annotated[str, typer.Argument()],
                    item: Annotated[int, typer.Argument(help="Checklist item number.")]) -> None:
    """(Human only) Tick a checklist item."""
    _milestone_check(name, item, True)


@milestone_app.command("uncheck")
@handle_errors
def milestone_uncheck(name: Annotated[str, typer.Argument()],
                      item: Annotated[int, typer.Argument(help="Checklist item number.")]) -> None:
    """(Human only) Untick a checklist item."""
    _milestone_check(name, item, False)


def _milestone_status(name: str, op: str) -> None:
    from . import roadmap

    _, ledger = _open()
    with ledger:
        m = roadmap.set_status(ledger, name, op)
    typer.echo(f"milestone {name}: {m.status}")


@milestone_app.command("done")
@handle_errors
def milestone_done(name: Annotated[str, typer.Argument()]) -> None:
    """(Human only) Mark a milestone done."""
    _milestone_status(name, "done")


@milestone_app.command("reopen")
@handle_errors
def milestone_reopen(name: Annotated[str, typer.Argument()]) -> None:
    """(Human only) Reopen a done or dropped milestone."""
    _milestone_status(name, "reopen")


@milestone_app.command("drop")
@handle_errors
def milestone_drop(name: Annotated[str, typer.Argument()]) -> None:
    """(Human only) Drop a milestone (it stays in the history)."""
    _milestone_status(name, "drop")


@app.command("arbd", hidden=True)
@handle_errors
def arbd(
    socket_path: Annotated[Path, typer.Option("--socket", help="Unix socket to listen on.")],
    project: Annotated[Path, typer.Option("--project", help="A worktree of the project (default: here).")] = Path("."),
) -> None:
    """The Mac app's core process: JSON over a Unix socket; exits when stdin closes."""
    from . import arbd as arbd_mod

    arbd_mod.serve(socket_path, project)


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


@hook_app.command("prompt-submit")
def hook_prompt_submit() -> None:
    """UserPromptSubmit hook: one line of context when the ledger changed since the last prompt."""
    code = hook_mod.prompt_submit(sys.stdin, sys.stdout)
    raise typer.Exit(code)


@app.command("install-claude")
@handle_errors
def install_claude(
    main: Annotated[bool, typer.Option("--main", help="Write into the main worktree instead.")] = False,
    user: Annotated[bool, typer.Option("--user", help="Write into ~/.claude/settings.json instead.")] = False,
    home: Annotated[bool, typer.Option("--home", help="Set up the seed home (~/arboria): its .claude/settings.json and CLAUDE.md.")] = False,
) -> None:
    """Add the hooks and permission rules to .claude/settings.json (the machine-specific
    global commands dir goes into .claude/settings.local.json) and the arboria Skill to
    .claude/skills/arboria/SKILL.md."""
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
    skill_change = skill_mod.install(root)
    if skill_change:
        changes.append(skill_change)
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


# ---------------------------------------------------------------- `arb --help` panels
#
# The top-level commands and groups, grouped for `arb --help` (Typer's rich_help_panel).
# Kept in one table so the grouping reads at a glance; tests/test_cli_help.py checks that
# every visible command is in exactly one panel.
HELP_PANELS: dict[str, list[str]] = {
    "你做的": ["idea", "milestone", "submit", "card", "approve", "reject", "key", "note", "status", "tree", "ui"],
    "集群": ["watch", "poll", "cancel", "logs", "remote", "machine", "doctor"],
    "agent 做的": ["todo", "commands", "inspect", "run", "lesson"],
    "安装与内部": ["init", "install-claude", "hook", "claudemd", "export"],
}


def _set_help_panels() -> None:
    panel_of = {name: panel for panel, names in HELP_PANELS.items() for name in names}
    for info in app.registered_commands:
        name = info.name or info.callback.__name__.lower().replace("_", "-")
        info.rich_help_panel = panel_of.get(name)
    for info in app.registered_groups:
        info.rich_help_panel = panel_of.get(info.name)


_set_help_panels()
