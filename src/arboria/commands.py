"""Notebook commands: small analysis functions Claude writes and the human calls from a notebook.

Claude writes the command library, the human writes the notebook. A command is a file
`<name>.py` defining a function `<name>(ctx, *args, **kwargs)` whose docstring is its
description. Two scopes:

- project: `<state dir>/commands/<name>.py` (this repo only), and
- global:  `$ARBORIA_CONFIG_DIR/commands/<name>.py` (default `~/.config/arboria/commands/`).

The project file wins on a name clash. `lab.<name>(...)` (see lab.py) loads and runs it in
the human's kernel; a call to a missing command becomes a todo for Claude instead.

This module never executes command code: listing reads files and parses them with `ast`.
Only `load()` imports a command, and only `lab` calls it (in the human's notebook).
"""

from __future__ import annotations

import ast
import hashlib
import importlib.util
import json
import re
import reprlib
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable

from . import clusters, tasks
from .db import LIMITS, Ledger
from .errors import ArbError

NAME_RE = re.compile(r"^[a-z][a-z0-9_]{0,40}$")
SCOPES = ("project", "global")
SCOPE_LABELS = {"project": "项目", "global": "全局"}
COMMANDS_DIR = "commands"
CACHE_DIR = "cache"
# Keyword arguments `lab` consumes itself; they never reach the command function:
# `scope=` picks where a new command goes, `ask=` is the first-time description.
RESERVED_KWARGS = ("scope", "ask")


def global_dir() -> Path:
    return clusters.config_dir() / COMMANDS_DIR


def project_dir(state_dir: Path) -> Path:
    return Path(state_dir) / COMMANDS_DIR


def scope_dir(scope: str, state_dir: Path | None) -> Path:
    if scope == "global":
        return global_dir()
    if state_dir is None:
        raise ArbError("project commands need an arboria repo (run `arb init`)")
    return project_dir(state_dir)


def cache_dir(state_dir: Path, name: str) -> Path:
    """Per-command scratch space (`<state dir>/cache/commands/<name>/`)."""
    return Path(state_dir) / CACHE_DIR / COMMANDS_DIR / validate_name(name, check_reserved=False)


def reserved_names() -> set[str]:
    """Names a command may not take: everything `lab` already defines."""
    from . import lab  # lazy: lab imports this module

    return {n for n in vars(lab) if not n.startswith("_")}


def validate_name(name: Any, *, check_reserved: bool = True) -> str:
    if not isinstance(name, str) or not NAME_RE.match(name):
        raise ArbError(f"invalid command name {name!r}: lowercase letters, digits and '_', "
                       "starting with a letter, at most 41 chars")
    if check_reserved and name in reserved_names():
        raise ArbError(f"{name!r} is already part of lab (lab.{name}); pick another command name")
    return name


def validate_scope(scope: Any) -> str:
    if scope not in SCOPES:
        raise ArbError(f"invalid command scope {scope!r}; choose one of: {', '.join(SCOPES)}")
    return scope


# ---------------------------------------------------------------- files


@dataclass(frozen=True)
class CommandFile:
    name: str
    scope: str  # project | global
    path: Path
    sha256: str

    @property
    def version(self) -> str:
        return self.sha256[:8]

    @property
    def label(self) -> str:
        return SCOPE_LABELS[self.scope]


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _file(name: str, scope: str, state_dir: Path | None) -> CommandFile | None:
    if scope == "project" and state_dir is None:
        return None
    path = scope_dir(scope, state_dir) / f"{name}.py"
    try:
        if not path.is_file():
            return None
        return CommandFile(name, scope, path, _sha(path))
    except OSError:
        return None


def find(name: str, state_dir: Path | None) -> CommandFile | None:
    """The command `name` as the notebook sees it: project first, then global."""
    validate_name(name, check_reserved=False)
    return _file(name, "project", state_dir) or _file(name, "global", state_dir)


def fingerprint(name: str, state_dir: Path | None) -> tuple[str, str] | None:
    """(path, sha) of what `lab.<name>` resolves to now; changes when the file is created,
    edited, deleted or shadowed."""
    found = find(name, state_dir)
    return None if found is None else (str(found.path), found.sha256)


@dataclass(frozen=True)
class Param:
    """One parameter of a command function, read from the source (never executed)."""

    name: str
    kind: str  # positional | vararg | keyword | varkw (positional covers positional-only)
    default: str | None = None  # source text of the default, None when it has none


@dataclass(frozen=True)
class Described:
    """What the source says about a command, without running it."""

    description: str | None  # the docstring (full; its first line is the summary)
    problem: str | None
    params: tuple[Param, ...] | None = None  # after `ctx`; None when there is no def

    @property
    def summary(self) -> str:
        return self.description.strip().splitlines()[0] if self.description else ""

    def signature(self, name: str, limit: int | None = None) -> str:
        if self.params is None:
            return f"{name}(…)"
        return format_signature(name, self.params, limit)

    def accepts_positional(self, n: int) -> bool:
        """Whether a call with `n` positional arguments (after ctx) fits; True if unknown."""
        if self.params is None:
            return True
        if any(p.kind == "vararg" for p in self.params):
            return True
        return n <= sum(1 for p in self.params if p.kind == "positional")


DEFAULT_CLIP = 24  # a long default value is shown clipped in signatures


def format_signature(name: str, params: Iterable[Param], limit: int | None = None) -> str:
    """`svd(run, *, k=20, log=False)`: no annotations, defaults as written. With `limit`,
    trailing parameters that do not fit become `…`."""
    parts: list[str] = []
    star = False
    for p in params:
        if p.kind == "vararg":
            parts.append(f"*{p.name}")
            star = True
            continue
        if p.kind == "varkw":
            parts.append(f"**{p.name}")
            continue
        if p.kind == "keyword" and not star:
            parts.append("*")
            star = True
        default = p.default
        if default is not None and len(default) > DEFAULT_CLIP:
            default = default[: DEFAULT_CLIP - 1] + "…"
        parts.append(p.name if default is None else f"{p.name}={default}")
    full = f"{name}({', '.join(parts)})"
    if limit is None or len(full) <= limit:
        return full
    kept: list[str] = []
    for part in parts:
        if len(f"{name}({', '.join([*kept, part, '…'])})") > limit:
            break
        kept.append(part)
    return f"{name}({', '.join([*kept, '…'])})"


def _params(fn: ast.FunctionDef) -> tuple[Param, ...]:
    a = fn.args
    positional = [*a.posonlyargs, *a.args]
    defaults = [None] * (len(positional) - len(a.defaults)) + list(a.defaults)
    out = [Param(arg.arg, "positional", None if d is None else ast.unparse(d))
           for arg, d in zip(positional, defaults)]
    if a.vararg is not None:
        out.append(Param(a.vararg.arg, "vararg"))
    out += [Param(arg.arg, "keyword", None if d is None else ast.unparse(d))
            for arg, d in zip(a.kwonlyargs, a.kw_defaults)]
    if a.kwarg is not None:
        out.append(Param(a.kwarg.arg, "varkw"))
    return tuple(out[1:]) if out and out[0].kind == "positional" else tuple(out)


def describe(path: Path, name: str) -> Described:
    """Docstring, signature and problems from the source, without running it."""
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except (OSError, UnicodeDecodeError, SyntaxError) as exc:
        return Described(None, f"cannot parse: {exc}")
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name:
            doc = ast.get_docstring(node)
            if isinstance(node, ast.AsyncFunctionDef):
                return Described(doc, f"{name} must be a plain def, not async def")
            params = _params(node)
            problem = None if doc else "no docstring"
            if not (node.args.posonlyargs or node.args.args):
                problem = f"{name} must take ctx as its first argument"
            clash = [p.name for p in params if p.name in RESERVED_KWARGS]
            if clash:
                problem = (f"parameter {clash[0]!r} is lab's own (it never reaches the "
                           "command); rename it")
            return Described(doc, problem, params)
    return Described(ast.get_docstring(tree), f"no top-level `def {name}(ctx, ...)`")


def discover(state_dir: Path | None) -> list[dict[str, Any]]:
    """Every command file of both scopes, sorted by name; `active` marks the one
    `lab.<name>` uses (a project file shadows a global one of the same name)."""
    reserved = reserved_names()
    rows: list[dict[str, Any]] = []
    for scope in SCOPES:
        if scope == "project" and state_dir is None:
            continue
        folder = scope_dir(scope, state_dir)
        if not folder.is_dir():
            continue
        for path in sorted(folder.glob("*.py")):
            name = path.stem
            if not NAME_RE.match(name):
                continue
            try:
                sha = _sha(path)
            except OSError:
                continue
            d = describe(path, name)
            problem = f"shadowed by lab.{name}; rename it" if name in reserved else d.problem
            rows.append({"name": name, "scope": scope, "version": sha[:8], "path": str(path),
                         "signature": d.signature(name), "description": d.summary,
                         "problem": problem})
    names = {r["name"] for r in rows if r["scope"] == "project"}
    for r in rows:
        r["active"] = r["scope"] == "project" or r["name"] not in names
    return sorted(rows, key=lambda r: (r["name"], r["scope"] != "project"))


# ---------------------------------------------------------------- built-ins

# The functions `lab` itself offers, in the order `lab.commands()` lists them, with the
# one line shown there (lab adds their signatures). Helpers and internals are left out.
BUILTINS: dict[str, str] = {
    "runs": "这个 idea 的 run 一览",
    "get": "按 id 取一个 run",
    "compare": "几个 run 的 wandb 曲线（长表）",
    "plot": "几个 run 的一条 wandb 曲线画在一起",
    "diff": "两个 run 哪里不同：命令、资源、代码、config",
    "spend": "各 idea 用了多少 SU 和 GPU 时；给 idea 就逐个 run",
    "run": "生成快照和审核单（不批准）",
    "fork": "从这个 idea 分出新 idea",
    "inspect": "请 agent 讲一段代码：代码摘录加批注、伪代码、简化代码或架构图",
    "todo": "给 agent 留一条待办",
    "todos": "待办列表",
    "conf": "改置信度",
    "metric": "改指标",
    "prune": "放弃这个 idea",
}
KIND_LABELS = {"builtin": "内置", **SCOPE_LABELS}


# ---------------------------------------------------------------- promote


def promote(ledger: Ledger, name: str, *, force: bool = False) -> dict[str, Any]:
    """Copy the project command `name` into the global library. Refuses to replace a
    different global file unless `force`; the project file stays (and still wins here)."""
    validate_name(name)
    src = _file(name, "project", ledger.state_dir)
    if src is None:
        raise ArbError(f"no project command {name!r} ({project_dir(ledger.state_dir)}/{name}.py)")
    dest = global_dir() / f"{name}.py"
    old = _file(name, "global", None)
    if old is not None and old.sha256 == src.sha256:
        return {"name": name, "version": src.version, "path": str(dest), "changed": False,
                "replaced": None}
    if old is not None and not force:
        raise ArbError(f"the global library already has {name} (v{old.version}, {dest}); "
                       "pass --force to replace it")
    dest.parent.mkdir(parents=True, exist_ok=True)
    data = src.path.read_bytes()
    version = hashlib.sha256(data).hexdigest()[:8]
    tmp = dest.with_name(f".{name}.py.tmp")
    tmp.write_bytes(data)
    tmp.replace(dest)
    replaced = old.version if old is not None else None
    with ledger.write():
        ledger.event("command_promote", name, {"version": version, "from": str(src.path),
                                               "to": str(dest), "replaced": replaced})
    return {"name": name, "version": version, "path": str(dest), "changed": True,
            "replaced": replaced}


# ---------------------------------------------------------------- loading (human's kernel only)

_loaded: dict[str, tuple[str, Callable[..., Any]]] = {}


def load(found: CommandFile) -> Callable[..., Any]:
    """Import the command file (again only when its content changed) and return the function."""
    key = str(found.path)
    cached = _loaded.get(key)
    if cached is not None and cached[0] == found.sha256:
        return cached[1]
    module_name = f"arboria_commands.{found.scope}.{found.name}"
    spec = importlib.util.spec_from_file_location(module_name, found.path)
    if spec is None or spec.loader is None:
        raise ArbError(f"cannot load command file {found.path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module  # e.g. dataclasses and pickling look themselves up
    try:
        spec.loader.exec_module(module)
    except BaseException:
        sys.modules.pop(module_name, None)
        raise
    func = getattr(module, found.name, None)
    if not callable(func):
        raise ArbError(f"{found.path} defines no function {found.name}(ctx, ...)")
    _loaded[key] = (found.sha256, func)
    return func


# ---------------------------------------------------------------- todos for Claude

_repr = reprlib.Repr()
_repr.maxstring = 200
_repr.maxother = 80
_repr.maxlist = _repr.maxtuple = _repr.maxdict = _repr.maxset = 8
_SAFE_TYPES = (str, int, float, bool, type(None), list, tuple, dict, set, frozenset)


def safe_repr(value: Any, convert: Callable[[Any], str | None] | None = None) -> str:
    """A short repr that never runs arbitrary `__repr__` code: builtins are shown (clipped),
    `convert` may name known objects (e.g. Run('r012')), anything else is `<TypeName>`."""
    if convert is not None:
        named = convert(value)
        if named is not None:
            return named
    if isinstance(value, _SAFE_TYPES) and type(value) in _SAFE_TYPES:
        if isinstance(value, (list, tuple, dict, set, frozenset)) and not _all_safe(value):
            return f"<{type(value).__name__} of {len(value)}>"
        return _repr.repr(value)
    return f"<{type(value).__name__}>"


def _all_safe(value: Any, depth: int = 0) -> bool:
    if depth > 3:
        return False
    items = list(value.items()) if isinstance(value, dict) else list(value)
    for item in items[:50]:
        parts = item if isinstance(value, dict) else (item,)
        for p in parts:
            if type(p) not in _SAFE_TYPES:
                return False
            if isinstance(p, (list, tuple, dict, set, frozenset)) and not _all_safe(p, depth + 1):
                return False
    return True


def description_of(args: tuple, kwargs: dict[str, Any]) -> str | None:
    """The call's first-time description: `ask="…"`, else (older notebooks) the last
    positional string argument (`lab.svd(r, "做 SVD …")`)."""
    ask = kwargs.get("ask")
    if isinstance(ask, str) and ask.strip():
        return ask.strip()
    for a in reversed(args):
        if isinstance(a, str) and a.strip():
            return a.strip()
    return None


def fit_args(described: Described, args: tuple) -> tuple:
    """`args` for an existing command: a trailing string the signature has no room for is
    the description of an older call (`lab.svd(r, "做 SVD")`) and is dropped."""
    if (args and isinstance(args[-1], str) and not described.accepts_positional(len(args))
            and described.accepts_positional(len(args) - 1)):
        return args[:-1]
    return args


def call_text(name: str, args: list[str], kwargs: dict[str, str]) -> str:
    parts = list(args) + [f"{k}={v}" for k, v in kwargs.items()]
    return f"lab.{name}({', '.join(parts)})"


def _clip(text: str, limit: int) -> str:
    return text if len(text) <= limit else text[: limit - 1] + "…"


def todo_text(kind: str, name: str, scope: str, description: str | None, call: str) -> str:
    limit = LIMITS["todo"]
    if kind == "command_new":
        head = f"写 notebook 命令 {name}（{SCOPE_LABELS[scope]}）：{description or '（没有说明，看调用）'}"
    else:
        head = f"修改 notebook 命令 {name}（{SCOPE_LABELS[scope]}）：{description}"
    tail = f"\n调用：{call}\n约定见 `arb commands guide`"
    return _clip(head, limit - min(len(tail), limit // 2)) + _clip(tail, limit // 2)


def request(ledger: Ledger, idea: dict[str, Any], kind: str, name: str, scope: str,
            description: str | None, args: list[str] | None = None,
            kwargs: dict[str, str] | None = None,
            path: str | None = None) -> tuple[dict[str, Any], bool]:
    """A command todo for Claude; (task, created). `idea` is the idea's row, or a seed's
    (seeds have no repo: global commands only). An open command_new todo for the same
    name is reused (re-running the cell must not pile up todos); a revise todo is reused
    only for the same instruction."""
    validate_name(name)
    validate_scope(scope)
    seed = tasks.is_seed(idea)
    if seed and scope != "global":
        raise ArbError("a seed has no repo: only global commands (scope=\"global\")")
    if kind not in ("command_new", "command_revise"):
        raise ArbError(f"not a command todo kind: {kind!r}")
    description = description.strip() if isinstance(description, str) else None
    if kind == "command_revise" and not description:
        raise ArbError("say what to change, e.g. lab.svd.revise(\"改成 log 坐标\")")
    if description:
        description = _clip(description, LIMITS["todo"])
    for task in tasks.open_command_tasks(ledger, name, kind):
        if kind == "command_new" or tasks.payload(task).get("description") == description:
            return task, False
    args, kwargs = list(args or []), dict(kwargs or {})
    call = call_text(name, args, kwargs)
    payload = {"name": name, "scope": scope, "args": args, "kwargs": kwargs,
               "description": description, "seed" if seed else "idea": idea["name"],
               "call": call,
               "path": path or str(scope_dir(scope, ledger.state_dir) / f"{name}.py")}
    text = todo_text(kind, name, scope, description, call)
    return tasks.add(ledger, idea, text, kind, payload), True


def push_text(task: dict[str, Any]) -> str:
    """The short message sent into the running Claude Code session (built from validated
    fields only: the todo id and the command name, never free text)."""
    data = tasks.payload(task)
    verb = "写" if task["kind"] == "command_new" else "修改"
    return (f"[arboria] notebook 里有新的命令待办 {task['id']}：{verb}命令 {data['name']}"
            f"（{'种子 ' + task['seed'] if task.get('seed') else 'idea ' + task['idea']}）。"
            "运行 arb commands guide 查看约定，"
            f"arb todo list --json 看调用和说明；做完 arb todo done {task['id']} --result \"...\"。")


# ---------------------------------------------------------------- the guide for Claude

GUIDE = """\
# notebook 命令：给 Claude 的约定

人在 marimo notebook 里调用 `lab.<名字>(..., ask="想要什么")`；命令不存在时会生成一个
命令待办（`arb todo list --json` 里 kind 是 command_new / command_revise，payload 里有
name、scope、调用的参数 args/kwargs、说明 description、idea 和要写的文件 path）。
你的活就是写好这个文件。notebook 是人的：读它，绝不修改。

先看 `arb commands list`：内置的（runs、compare、plot、diff、spend…）和已有的命令
能做的，不要再写一个。

## 文件
- 项目范围：{project}/<名字>.py；全局：{global_}/<名字>.py（项目里的同名文件优先）。
  写到待办 payload 的 path；修改（command_revise）就改现有的那个文件。
- 名字 ^[a-z][a-z0-9_]{{0,40}}$，不能和 lab 已有的函数重名。
- 文件里定义同名函数，第一个参数是 ctx。docstring 第一行是描述（命令表里显示它），
  后面几行说明参数；人在 notebook 里输入 `lab.<名字>` 看到的帮助就是签名加 docstring：

```python
def svd(ctx, run, *, k=20, log=False):
    \"\"\"对 run 的 attention 权重做 SVD，画前 k 个奇异值。

    k：画几个奇异值；log：纵轴用对数。
    \"\"\"
    r = ctx.run(run)
    ...
    return fig
```

- 函数要能接受第一次调用的形状（payload 里的 args/kwargs）。说明（`ask=`）只是给你
  看的，不会传进来，不要为它留参数；旧 notebook 里说明写成末尾的位置字符串时，lab
  发现签名放不下它就会去掉。`scope=`、`ask=` 是 lab 自己用的，不能当参数名。
- 好的项目命令以后可以 `arb commands promote <名字>` 复制进全局命令库（人决定）。

## ctx
- `ctx.run(run_or_id)`：run 句柄（`lab.Run`）。`.id`、`.status()`、`.row()`、
  `.local_out`（拉回来的 ARBORIA_OUT：.arboria/runs/<run>/out/，可能还不存在）、
  `.config`（解析后的 config，dict 或 None）、`.metrics(keys)`（wandb 曲线，需要 wandb）。
- `ctx.idea`：name、hypothesis、metric。在种子的 notebook 里（`~/arboria/seeds/`）
  `ctx.idea` 是 None，`ctx.seed` 有 name、gist、hypothesis、metric、notebook；种子没有
  run，`ctx.run` / `ctx.runs` 会报错，所以给种子用的命令只能是全局命令、不碰 run。
- `ctx.mo`：marimo 模块（不在 marimo 里时是 None）。
- `ctx.cache_dir`：这个命令自己的缓存目录（.arboria/cache/commands/<名字>/）。
- `ctx.value(x)`：x 是 marimo 控件时取 `.value`，否则原样返回。

## 写法
- 返回值就是 notebook 里显示的东西（图、表、`ctx.mo.md(...)`、`ctx.mo.vstack([...])`）。
  返回 None 只显示出处。lab 会在下面加一行“命令 <名字> · v<版本> · 项目/全局”。
- 有歧义的选择不要先问人：做成带合理默认值的关键字参数（k=20、log=False），在输出里
  写明用了什么、怎么改（例如“k=20，改成 lab.svd(r, k=50)”）；参数也接受 marimo 控件
  （人把 `k = mo.ui.slider(...)` 绑定到变量后传进来，用 `ctx.value(k)` 取值）；几种看法
  都便宜时可以用 `ctx.mo.ui.tabs` 一起给出。只有实在没法继续时，才在 Claude 面板里问人。
- 只读 run 的输出和账本：不改 `.arboria/runs/`、不写 notebook、不碰集群。
- 不联网（wandb 曲线用 `ctx.run(r).metrics(...)`）。
- 只在 `ctx.cache_dir` 下写文件。
- 文件要短，一个命令一件事，开头有 docstring；依赖只用人环境里已有的包。
- 代码在人的 kernel 里、以人的权限运行；人会看 diff 再用，写得让人一眼能看懂。

## 做完
- 存盘后 notebook 里那个 cell 会自动重跑（marimo 正在运行时）。
- 能在终端里试的就试：`python -c 'from arboria import lab; lab.use("<idea>"); print(lab.<名字>("r001"))'`。
- `arb todo done tNNN --result "写了 <名字>：…，默认 k=20"`（≤600 字）。
- `arb commands list` 查看现有命令；`lab.<名字>.revise("...")` 是人要你改的时候用的。
"""


def guide() -> str:
    return GUIDE.format(project=".arboria/commands", global_=str(global_dir()))


def as_json(rows: list[dict[str, Any]]) -> str:
    return json.dumps(rows, ensure_ascii=False, indent=2)
