"""One marimo notebook per idea: `.arboria/notebooks/<idea>.py`, linked as `notebook.py`.

The notebook is the human's: they write it, Claude may read it but never edits it
(install-claude denies that). It lives in the state dir so the backup repo versions
it and it outlives the idea's worktree; the worktree gets a symlink `notebook.py`,
which `info/exclude` keeps out of every snapshot. marimo is not needed to create it.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable

from .errors import ArbError
from .paths import STATE_DIR_NAME, RepoContext, ensure_excluded

NOTEBOOKS_DIR = "notebooks"
LINK_NAME = "notebook.py"
EXCLUDE_PATTERN = f"/{LINK_NAME}"
EXCLUDE_COMMENT = "arboria: notebook.py links to the idea's notebook in .arboria/ (never snapshotted)"
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


def header_markdown(idea: dict[str, Any], parent: str | None = None,
                    from_run: dict[str, Any] | None = None) -> str:
    lines = [f"# {idea['name']}", "",
             f"- **假设：** {idea['hypothesis']}",
             f"- **指标：** {idea['metric']}"]
    if parent:
        origin = ""
        if from_run:
            origin = f"（从 {from_run['id']} 的快照 `{from_run['snapshot_commit'][:10]}` 分出）"
        lines.append(f"- **父 idea：** {parent}{origin}")
    lines.append(f"- **创建：** {idea['created_at'][:10]}")
    return "\n".join(lines)


def render(idea: dict[str, Any], parent: str | None = None,
           from_run: dict[str, Any] | None = None) -> str:
    """The notebook source (marimo's file format; plain Python)."""
    md = _md_call(header_markdown(idea, parent, from_run))
    return f'''import marimo

__generated_with = "{_generated_with()}"
app = marimo.App(width="medium")


@app.cell(hide_code=True)
def _(mo):
    {md}
    return


@app.cell
def _():
    import marimo as mo
    from arboria import lab

    return lab, mo


@app.cell
def _(lab):
    lab.runs()
    return


if __name__ == "__main__":
    app.run()
'''


def create(ctx: RepoContext, idea: dict[str, Any], parent: str | None = None,
           from_run: dict[str, Any] | None = None,
           warn: Callable[[str], None] | None = None, *, link: bool = True) -> Path:
    """Write the idea's notebook (never overwriting one) and link it into the worktree."""
    path = notebook_path(ctx.state_dir, idea["name"])
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        tmp = path.with_name(f".{path.name}.arboria-tmp")
        tmp.write_text(render(idea, parent, from_run), encoding="utf-8")
        tmp.replace(path)
    if not link:
        return path
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
