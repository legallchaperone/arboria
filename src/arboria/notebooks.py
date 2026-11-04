"""One marimo notebook per idea: `.arboria/notebooks/<idea>.py`, linked as `notebook.py`.

The notebook is the human's: they write it, Claude may read it but never edits it
(install-claude denies that). It lives in the state dir so the backup repo versions
it and it outlives the idea's worktree; the worktree gets a symlink `notebook.py`,
which `info/exclude` keeps out of every snapshot. marimo is not needed to create it.
"""

from __future__ import annotations

import ast
import os
import textwrap
from pathlib import Path
from typing import Any, Callable

from . import style
from .errors import ArbError
from .paths import STATE_DIR_NAME, RepoContext, ensure_excluded
from .style import STYLE_NAME

NOTEBOOKS_DIR = "notebooks"
LINK_NAME = "notebook.py"
EXCLUDE_PATTERN = f"/{LINK_NAME}"
EXCLUDE_COMMENT = "arboria: notebook.py links to the idea's notebook in .arboria/ (never snapshotted)"
STYLE_EXCLUDE_PATTERN = f"/{STYLE_NAME}"
STYLE_EXCLUDE_COMMENT = ("arboria: arboria.css links to the notebooks' stylesheet (the font) "
                         "in .arboria/ (never snapshotted)")
# The marimo file format the template follows (checked with `marimo check --strict`).
MARIMO_FORMAT_VERSION = "0.25.2"


def notebooks_dir(state_dir: Path) -> Path:
    return Path(state_dir) / NOTEBOOKS_DIR


def notebook_path(state_dir: Path, idea_name: str) -> Path:
    return notebooks_dir(state_dir) / f"{idea_name}.py"


def idea_of_notebook(path: Path | str) -> tuple[Path, str] | None:
    """(state dir, idea name) if `path` (symlinks resolved) is `.arboria/notebooks/<idea>.py`."""
    try:
        p = Path(path).resolve()
    except (OSError, RuntimeError):
        return None
    if (p.suffix == ".py" and p.parent.name == NOTEBOOKS_DIR
            and p.parent.parent.name == STATE_DIR_NAME):
        return p.parent.parent, p.stem
    return None


def _md_call(text: str) -> str:
    """`mo.md(...)` in marimo's own layout (what `marimo check --fix` writes): a raw
    triple-quoted string indented with the cell when that is safe, else a plain literal."""
    if '"""' not in text and not text.endswith("\\") and "\r" not in text:
        body = "\n".join(("    " + line) if line else "" for line in text.splitlines())
        return f'mo.md(r"""\n{body}\n    """)'
    return f"mo.md(\n        {text!r}\n    )"


def _generated_with() -> str:
    """The marimo version recorded in the file (marimo itself is optional here)."""
    try:
        from importlib.metadata import version

        return version("marimo")
    except Exception:
        return MARIMO_FORMAT_VERSION


def display_path(path: Path | str) -> str:
    """`~/arboria/seeds/x.py` for a path under the home directory, else the path."""
    p = Path(path)
    try:
        return "~/" + str(p.relative_to(Path.home()))
    except ValueError:
        return str(p)


def header_markdown(idea: dict[str, Any], parent: str | None = None,
                    from_run: dict[str, Any] | None = None,
                    seed: dict[str, Any] | None = None) -> str:
    lines = [f"# {idea['name']}", "",
             f"- **假设：** {idea['hypothesis']}",
             f"- **指标：** {idea['metric']}"]
    if parent:
        origin = ""
        if from_run:
            origin = f"（从 {from_run['id']} 的快照 `{from_run['snapshot_commit'][:10]}` 分出）"
        lines.append(f"- **父 idea：** {parent}{origin}")
    if seed:
        lines.append(f"- **来自种子：** {seed['name']}（`{display_path(seed['notebook'])}`）")
    lines.append(f"- **创建：** {idea['created_at'][:10]}")
    return "\n".join(lines)


SEED_THOUGHTS = "种子里的思路"


def _thoughts_cell(markdown: list[str]) -> str:
    """The seed's markdown cells in one markdown cell, collapsed (marimo's `/// details`
    block); '' when there are none."""
    if not markdown:
        return ""
    md = _md_call(f"/// details | {SEED_THOUGHTS}\n\n" + "\n\n".join(markdown) + "\n\n///")
    return f'''

@app.cell(hide_code=True)
def _(mo):
    {md}
    return
'''


def render(idea: dict[str, Any], parent: str | None = None,
           from_run: dict[str, Any] | None = None, seed: dict[str, Any] | None = None) -> str:
    """The notebook source (marimo's file format; plain Python). `seed` (an idea planted
    from a seed): its name, notebook path and markdown cells (see `seed_markdown`)."""
    md = _md_call(header_markdown(idea, parent, from_run, seed))
    thoughts = _thoughts_cell(seed.get("markdown") or []) if seed else ""
    return f'''import marimo

__generated_with = "{_generated_with()}"
app = marimo.App(width="medium", css_file="{STYLE_NAME}")


@app.cell(hide_code=True)
def _(mo):
    {md}
    return


@app.cell
def _():
    import marimo as mo
    from arboria import lab

    return lab, mo
{thoughts}

@app.cell
def _(lab):
    lab.runs()
    return


if __name__ == "__main__":
    app.run()
'''


def write_new(path: Path, text: str) -> bool:
    """Write `text` to `path` unless it exists (atomic; never overwrites). True if written."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        return False
    tmp = path.with_name(f".{path.name}.arboria-tmp")
    tmp.write_text(text, encoding="utf-8")
    tmp.replace(path)
    return True


def create(ctx: RepoContext, idea: dict[str, Any], parent: str | None = None,
           from_run: dict[str, Any] | None = None,
           warn: Callable[[str], None] | None = None, *, link: bool = True,
           seed: dict[str, Any] | None = None) -> Path:
    """Write the idea's notebook (never overwriting one) and its stylesheet, and link
    both into the worktree."""
    path = notebook_path(ctx.state_dir, idea["name"])
    write_new(path, render(idea, parent, from_run, seed))
    ensure_style(path.parent, warn)
    if not link:
        return path
    link_style(ctx, Path(idea["worktree_path"]), warn)
    ensure_excluded(ctx.common_dir, EXCLUDE_PATTERN, EXCLUDE_COMMENT)
    link = Path(idea["worktree_path"]) / LINK_NAME
    if link.is_symlink() and link.resolve() == path.resolve():
        return path
    if link.exists() or link.is_symlink():
        if warn:
            warn(f"{link} already exists (tracked in the repo?); not linking the notebook. "
                 f"Open {path} instead")
        return path
    try:
        link.symlink_to(path)
    except OSError as exc:
        raise ArbError(f"cannot link {link} -> {path}: {exc}") from exc
    return path


# ---------------------------------------------------------------- the stylesheet


def ensure_style(directory: Path, warn: Callable[[str], None] | None = None) -> Path | None:
    """`arboria.css` (the font and typography, see arboria.style) next to the notebooks in
    `directory`. A failure only costs the font: warned, never raised."""
    try:
        return style.ensure_notebook_style(directory)
    except OSError as exc:
        if warn:
            warn(f"cannot write the notebook stylesheet in {directory}: {exc}")
        return None


def link_style(ctx: RepoContext, worktree: Path,
               warn: Callable[[str], None] | None = None) -> bool:
    """Link `<worktree>/arboria.css` to the notebooks' stylesheet: marimo reads css_file
    next to the path it opened (`notebook.py`, the link), not next to the link's target."""
    target = notebooks_dir(ctx.state_dir) / STYLE_NAME
    if not worktree.is_dir():
        return False
    ensure_excluded(ctx.common_dir, STYLE_EXCLUDE_PATTERN, STYLE_EXCLUDE_COMMENT)
    try:
        if style.link(worktree / STYLE_NAME, target):
            return True
    except OSError:
        pass
    if warn:
        warn(f"{worktree / STYLE_NAME} already exists; not linking the notebook stylesheet")
    return False


CSS_KEYWORD = f'css_file="{STYLE_NAME}"'


def _app_call(tree: ast.Module) -> ast.Call | None:
    """The `app = marimo.App(...)` call at module level."""
    for node in tree.body:
        if (isinstance(node, ast.Assign) and len(node.targets) == 1
                and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "app"
                and isinstance(node.value, ast.Call)):
            f = node.value.func
            if (isinstance(f, ast.Attribute) and f.attr == "App"
                    and isinstance(f.value, ast.Name) and f.value.id == "marimo"):
                return node.value
    return None


def add_style(source: str) -> tuple[str, str]:
    """`source` with `css_file="arboria.css"` added to its `marimo.App(...)`, and what
    happened: "added", "already" (it names arboria.css), "own" (another css_file: left
    alone), "unknown" (no marimo.App call found, or not Python). Only the App call
    changes; every cell stays byte-identical. Parsed with `ast`, never executed."""
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return source, "unknown"
    call = _app_call(tree)
    if call is None or call.end_lineno is None or call.end_col_offset is None:
        return source, "unknown"
    for kw in call.keywords:
        if kw.arg == "css_file":
            ok = isinstance(kw.value, ast.Constant) and kw.value.value == STYLE_NAME
            return source, "already" if ok else "own"
    lines = source.splitlines(keepends=True)
    line = lines[call.end_lineno - 1]
    # ast offsets count UTF-8 bytes; the call ends with its ")"
    close = len(line.encode("utf-8")[:call.end_col_offset - 1].decode("utf-8"))
    if line[close:close + 1] != ")":
        return source, "unknown"
    offset = sum(len(l) for l in lines[:call.end_lineno - 1]) + close
    head = source[:offset]
    stripped = head.rstrip()
    sep = "" if stripped.endswith("(") else " " if stripped.endswith(",") else ", "
    return stripped + sep + CSS_KEYWORD + head[len(stripped):] + source[offset:], "added"


def restyle(path: Path) -> str:
    """Add the stylesheet to the notebook file at `path` (see `add_style`); returns what
    happened. Writes only when something was added (atomic, keeps the file mode; a
    link is followed, the file it points at is written)."""
    path = Path(path).resolve()
    text = path.read_text(encoding="utf-8")
    new, what = add_style(text)
    if what == "added":
        tmp = path.with_name(f".{path.name}.arboria-tmp")
        tmp.write_text(new, encoding="utf-8")
        try:
            os.chmod(tmp, path.stat().st_mode & 0o7777)
        except OSError:
            pass
        tmp.replace(path)
    return what


# ---------------------------------------------------------------- seeds


def seed_header_markdown(seed: dict[str, Any]) -> str:
    lines = [f"# {seed['name']}"]
    if seed.get("gist"):
        lines += ["", seed["gist"]]
    facts = []
    if seed.get("hypothesis"):
        facts.append(f"- **假设：** {seed['hypothesis']}")
    if seed.get("metric"):
        facts.append(f"- **指标：** {seed['metric']}")
    facts.append(f"- **创建：** {seed['created_at'][:10]} · 种子")
    return "\n".join(lines + [""] + facts)


def render_seed(seed: dict[str, Any]) -> str:
    """A seed's notebook: the header and `import marimo as mo`, nothing else (no lab.run:
    a seed has no repo to run in)."""
    md = _md_call(seed_header_markdown(seed))
    return f'''import marimo

__generated_with = "{_generated_with()}"
app = marimo.App(width="medium", css_file="{STYLE_NAME}")


@app.cell(hide_code=True)
def _(mo):
    {md}
    return


@app.cell
def _():
    import marimo as mo

    return (mo,)


if __name__ == "__main__":
    app.run()
'''


def _is_cell(node: ast.AST) -> bool:
    if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
        return False
    for d in node.decorator_list:
        target = d.func if isinstance(d, ast.Call) else d
        if isinstance(target, ast.Attribute) and target.attr == "cell":
            return True
    return False


def _md_text(fn: ast.FunctionDef | ast.AsyncFunctionDef) -> str | None:
    """The text of a cell that is only `mo.md("...")` (a plain string, not an f-string)."""
    body = [n for n in fn.body if not isinstance(n, ast.Return)]
    if len(body) != 1 or not isinstance(body[0], ast.Expr):
        return None
    call = body[0].value
    if not (isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute)
            and call.func.attr == "md" and len(call.args) == 1 and not call.keywords):
        return None
    arg = call.args[0]
    if not (isinstance(arg, ast.Constant) and isinstance(arg.value, str)):
        return None
    return textwrap.dedent(arg.value).strip() or None


def seed_markdown(source: str, name: str | None = None) -> list[str]:
    """The markdown cells of a seed notebook (the human's thinking), in order, without
    the generated header (a first cell starting `# <name>`). Code cells are left out:
    they may assume another context. Parsed with `ast`, never executed."""
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return []
    out = []
    for k, fn in enumerate(n for n in tree.body if _is_cell(n)):
        text = _md_text(fn)
        if text is None or (k == 0 and name and text.startswith(f"# {name}")):
            continue
        out.append(text)
    return out
