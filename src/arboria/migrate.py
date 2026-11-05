"""marimo notebook → the app's markdown notebook (ADR 0005), read with `ast` only.

`convert(source)` returns the parts in order: {"kind": "text", "text"} for `mo.md(...)`
cells (a plain string; an f-string or anything computed stays code), {"kind": "block",
"code"} for the other cells, without marimo's `def _(…):` wrapper and its last `return`.
Cells that only import marimo or arboria go away (the kernel has `lab` already).
Nothing is executed.
"""

from __future__ import annotations

import ast
import textwrap
from typing import Any

DROPPED_IMPORTS = {"marimo", "arboria", "arboria.lab"}


def _is_cell(dec: ast.expr) -> bool:
    """`@app.cell` or `@app.cell(...)`."""
    node = dec.func if isinstance(dec, ast.Call) else dec
    return (isinstance(node, ast.Attribute) and node.attr in ("cell", "function")
            and isinstance(node.value, ast.Name) and node.value.id == "app")


def _only_imports_we_drop(stmt: ast.stmt) -> bool:
    if isinstance(stmt, ast.Import):
        return all(a.name in DROPPED_IMPORTS for a in stmt.names)
    if isinstance(stmt, ast.ImportFrom):
        return (stmt.module in DROPPED_IMPORTS
                or (stmt.module == "arboria" and all(a.name == "lab" for a in stmt.names)))
    return False


def _md_text(stmt: ast.stmt) -> str | None:
    """The text of `mo.md("...")` (also `mo.md(r\"\"\"...\"\"\")`), when it is a constant."""
    if not (isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call)):
        return None
    f = stmt.value.func
    if not (isinstance(f, ast.Attribute) and f.attr == "md" and isinstance(f.value, ast.Name)
            and f.value.id == "mo" and len(stmt.value.args) == 1 and not stmt.value.keywords):
        return None
    arg = stmt.value.args[0]
    if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
        return textwrap.dedent(arg.value).strip("\n").rstrip()
    return None


def _source(lines: list[str], stmts: list[ast.stmt]) -> str:
    first = min(getattr(s, "decorator_list", None) and s.decorator_list[0].lineno or s.lineno
                for s in stmts)
    last = max(s.end_lineno or s.lineno for s in stmts)
    return textwrap.dedent("".join(lines[first - 1:last])).strip("\n")


def convert(source: str) -> list[dict[str, Any]]:
    tree = ast.parse(source)
    lines = source.splitlines(keepends=True)
    parts: list[dict[str, Any]] = []
    for node in tree.body:
        body: list[ast.stmt] | None = None
        if isinstance(node, ast.With) and any(
                isinstance(i.context_expr, ast.Attribute) and i.context_expr.attr == "setup"
                for i in node.items):
            body = list(node.body)  # `with app.setup:` → its statements
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and any(
                _is_cell(d) for d in node.decorator_list):
            is_function = any(
                (d.func if isinstance(d, ast.Call) else d).attr == "function"  # type: ignore[union-attr]
                for d in node.decorator_list if _is_cell(d))
            if is_function:  # `@app.function`: the def itself, without the decorator
                start, end = node.lineno, node.end_lineno or node.lineno
                parts.append({"kind": "block",
                              "code": textwrap.dedent("".join(lines[start - 1:end])).strip("\n")})
                continue
            body = list(node.body)
            if body and isinstance(body[-1], ast.Return):
                body.pop()
        if body is None:
            continue
        body = [s for s in body if not _only_imports_we_drop(s)
                and not (isinstance(s, ast.Pass))]
        if not body:
            continue
        if len(body) == 1:
            text = _md_text(body[0])
            if text is not None:
                if text:
                    parts.append({"kind": "text", "text": text})
                continue
        parts.append({"kind": "block", "code": _source(lines, body)})
    return parts
