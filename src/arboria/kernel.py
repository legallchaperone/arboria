"""The app notebook's kernel (ADR 0005): `python -m arboria.kernel`.

It runs the human's command blocks in one namespace, in the human's own Python: the same
trust as marimo. Protocol: one JSON object per line, requests on stdin, events on stdout.

Requests:  {"id", "op": "exec", "block", "code", "file"} · {"id", "op": "shutdown"}
Events:    {"event": "ready", "python", "lab"} once, then per request
           {"id", "event": "stream", "name": "stdout"|"stderr", "text"}
           {"id", "event": "output", "data": {kind, …}}   (arboria.output)
           {"id", "event": "done", "ok", "defines", "uses", "duration_ms"}

The protocol keeps the original stdout; fd 1 is pointed at stderr, so whatever user code
or its subprocesses write there cannot break a line. SIGINT interrupts a running block
(KeyboardInterrupt) and is ignored while idle.

A block whose lines start with `/` is slash commands: `/plot r001 r002 val/loss` calls
`lab.plot("r001", "r002", "val/loss")` (shlex words; `k=v` is a keyword; values that
read as Python literals are literals, anything else a string).
"""

from __future__ import annotations

import ast
import builtins
import io
import json
import linecache
import os
import shlex
import signal
import sys
import threading
import time
import traceback
from typing import Any

from . import output

MAX_STREAM = output.MAX_TEXT
# set in the kernel's environment: `lab` knows it is not in marimo (ctx.mo is None)
KERNEL_ENV = "ARBORIA_KERNEL"

_lock = threading.Lock()
_proto: Any = None
_current: list[Any] = [None]
_executing = [False]


def send(event: dict[str, Any]) -> None:
    line = json.dumps(event, ensure_ascii=False, allow_nan=False, default=str)
    with _lock:
        _proto.write(line + "\n")
        _proto.flush()


class Stream(io.TextIOBase):
    """sys.stdout / sys.stderr while a block runs: `stream` events, flushed per line.
    A TextIOBase, so libraries that inspect the stream (encoding, errors, …) work."""

    def __init__(self, name: str):
        super().__init__()
        self.name = name
        self.buf = ""
        self.sent = 0
        self.cut = False

    @property
    def encoding(self) -> str:  # type: ignore[override]
        return "utf-8"

    @property
    def errors(self) -> str:  # type: ignore[override]
        return "strict"

    def write(self, s: str) -> int:
        if not isinstance(s, str):
            s = str(s)
        self.buf += s
        if "\n" in self.buf or len(self.buf) > 8192:
            self.flush()
        return len(s)

    def flush(self) -> None:
        if not self.buf:
            return
        text, self.buf = self.buf, ""
        if self.cut:
            return
        room = MAX_STREAM - self.sent
        if len(text) > room:
            text = text[:room] + "\n…（输出太多，后面的没显示）\n"
            self.cut = True
        self.sent += len(text)
        send({"id": _current[0], "event": "stream", "name": self.name, "text": text})

    def reset(self) -> None:
        self.flush()
        self.sent, self.cut = 0, False

    def isatty(self) -> bool:
        return False

    def fileno(self) -> int:
        return 2

    def writable(self) -> bool:
        return True


def emit(value: Any) -> None:
    """Show `value` under the block now (also `display(...)` in the notebook)."""
    sys.stdout.flush()
    sys.stderr.flush()
    try:
        outs = output.to_outputs(value)
    except Exception as exc:  # a broken _arboria_ costs only its own look
        outs = [output.text(f"{value!r:.2000}"),
                output.text(f"（没法按结构显示：{type(exc).__name__}: {exc}）")]
    for o in outs:
        send({"id": _current[0], "event": "output", "data": o})


def display(*values: Any) -> None:
    for v in values:
        emit(v)


# ---------------------------------------------------------------- names


def names(tree: ast.AST) -> tuple[list[str], list[str]]:
    """(defines, uses): names bound at the top level, and names read that the block does
    not bind itself (builtins left out)."""
    defines: list[str] = []
    loads: list[str] = []

    def bind(n: str) -> None:
        if n not in defines:
            defines.append(n)

    def local_names(fn: ast.AST) -> set[str]:
        """A function's (or lambda's) own names: its parameters and what it assigns."""
        out: set[str] = set()
        a = getattr(fn, "args", None)
        if a is not None:
            for arg in [*a.posonlyargs, *a.args, *a.kwonlyargs, a.vararg, a.kwarg]:
                if arg is not None:
                    out.add(arg.arg)
        body = fn.body if isinstance(fn.body, list) else [fn.body]
        for stmt in body:
            for n in ast.walk(stmt):
                if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Store):
                    out.add(n.id)
                elif isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    out.add(n.name)
        return out

    def visit(node: ast.AST, top: bool, local: frozenset[str] = frozenset()) -> None:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if top:
                bind(node.name)
            for d in node.decorator_list:
                visit(d, top, local)
            inner = local | local_names(node) if not isinstance(node, ast.ClassDef) else local
            for child in node.body:
                visit(child, False, frozenset(inner))
            return
        if isinstance(node, ast.Lambda):
            visit(node.body, False, frozenset(local | local_names(node)))
            return
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            for a in node.names:
                if top and a.name != "*":
                    bind((a.asname or a.name).split(".")[0])
            return
        if isinstance(node, ast.Name):
            if isinstance(node.ctx, ast.Store):
                if top:
                    bind(node.id)
            elif node.id not in loads and node.id not in local:
                loads.append(node.id)
        for child in ast.iter_child_nodes(node):
            visit(child, top, local)

    for stmt in getattr(tree, "body", []):
        visit(stmt, True)
    builtin = set(dir(builtins))
    uses = [n for n in loads if n not in defines and n not in builtin]
    return defines, uses


# ---------------------------------------------------------------- slash commands


def _value(word: str) -> Any:
    try:
        return ast.literal_eval(word)
    except (ValueError, SyntaxError, MemoryError, RecursionError):
        return word


def slash_lines(code: str) -> list[str] | None:
    """The command lines when the block is slash commands, else None."""
    lines = [ln.strip() for ln in code.splitlines()]
    lines = [ln for ln in lines if ln and not ln.startswith("#")]
    if not lines or not lines[0].startswith("/"):
        return None
    if not all(ln.startswith("/") for ln in lines):
        raise SyntaxError("命令块里不能把斜杠命令和 Python 混在一起：一种写法一块")
    return lines


def parse_slash(line: str) -> tuple[str, list[Any], dict[str, Any]]:
    words = shlex.split(line[1:], comments=True)
    if not words:
        raise SyntaxError("斜杠后面要有命令名，例如 /plot r001 val/loss")
    name, rest = words[0], words[1:]
    if not name.isidentifier() or name.startswith("_"):
        raise SyntaxError(f"不是命令名：/{name}")
    args: list[Any] = []
    kwargs: dict[str, Any] = {}
    for w in rest:
        key, eq, val = w.partition("=")
        if eq and key.isidentifier():
            kwargs[key] = _value(val)
        elif kwargs:
            raise SyntaxError(f"位置参数要写在 k=v 前面：{w}")
        else:
            args.append(_value(w))
    return name, args, kwargs


# ---------------------------------------------------------------- execution


def _figures() -> None:
    """Open matplotlib figures, shown and closed at the end of a block (like Jupyter)."""
    plt = sys.modules.get("matplotlib.pyplot")
    if plt is None:
        return
    for num in plt.get_fignums():
        emit(plt.figure(num))
    plt.close("all")


def _traceback(exc: BaseException, filename: str) -> list[str]:
    te = traceback.TracebackException.from_exception(exc)
    frames = [f for f in te.stack if f.filename != __file__]
    te.stack = traceback.StackSummary.from_list(frames)
    return list(te.format())


def run_block(ns: dict[str, Any], block: str, code: str, file: str | None) -> dict[str, Any]:
    filename = f"<块 {block}>"
    if file:
        ns["__file__"] = file
    defines: list[str] = []
    uses: list[str] = []
    ok = True
    try:
        slash = slash_lines(code)
        if slash is not None:
            lab = ns.get("lab")
            if lab is None:
                raise RuntimeError("这个 Python 里 import 不了 arboria，斜杠命令用不了")
            for line in slash:
                name, args, kwargs = parse_slash(line)
                # called from a frame whose globals are the notebook's, so lab finds the
                # notebook through __file__ exactly as for `lab.name(...)`
                ns["__arb_call__"] = (getattr(lab, name), args, kwargs)
                try:
                    emit(eval("__arb_call__[0](*__arb_call__[1], **__arb_call__[2])", ns))
                finally:
                    ns.pop("__arb_call__", None)
        else:
            linecache.cache[filename] = (len(code), None, code.splitlines(True), filename)
            tree = ast.parse(code, filename, "exec")
            defines, uses = names(tree)
            last = None
            if tree.body and isinstance(tree.body[-1], ast.Expr):
                last = ast.Expression(tree.body.pop().value)
            exec(compile(tree, filename, "exec"), ns)
            if last is not None:
                value = eval(compile(last, filename, "eval"), ns)
                if value is not None:
                    ns["_"] = value
                    emit(value)
        _figures()
    except KeyboardInterrupt:
        ok = False
        send({"id": _current[0], "event": "output",
              "data": output.error("KeyboardInterrupt", "已中断", [])})
    except BaseException as exc:  # noqa: BLE001 - the block's error is its output
        ok = False
        if isinstance(exc, SystemExit):
            exc = RuntimeError(f"代码调用了 exit({exc.code})；kernel 没有退出")
        send({"id": _current[0], "event": "output",
              "data": output.error(type(exc).__name__, str(exc), _traceback(exc, filename))})
    return {"ok": ok, "defines": defines, "uses": uses}


def main() -> None:
    global _proto
    _proto = os.fdopen(os.dup(1), "w", encoding="utf-8", buffering=1)
    os.dup2(2, 1)
    os.environ.setdefault("MPLBACKEND", "Agg")
    os.environ[KERNEL_ENV] = "1"
    out, err = Stream("stdout"), Stream("stderr")
    sys.stdout, sys.stderr = out, err  # type: ignore[assignment]

    def on_sigint(signum: int, frame: Any) -> None:
        if _executing[0]:
            raise KeyboardInterrupt

    signal.signal(signal.SIGINT, on_sigint)
    ns: dict[str, Any] = {"__name__": "__main__", "__builtins__": builtins, "display": display}
    lab_error = None
    try:
        from arboria import lab

        ns["lab"] = lab
    except Exception as exc:  # noqa: BLE001
        lab_error = f"{type(exc).__name__}: {exc}"
    send({"event": "ready", "python": sys.version.split()[0], "executable": sys.executable,
          "lab": lab_error is None, "lab_error": lab_error})
    for line in sys.stdin:
        try:
            req = json.loads(line)
        except ValueError:
            continue
        rid = req.get("id")
        op = req.get("op")
        if op == "shutdown":
            send({"id": rid, "event": "done", "ok": True, "defines": [], "uses": [],
                  "duration_ms": 0})
            break
        if op != "exec":
            send({"id": rid, "event": "done", "ok": False, "defines": [], "uses": [],
                  "duration_ms": 0, "error": f"unknown op {op!r}"})
            continue
        _current[0] = rid
        t0 = time.monotonic()
        _executing[0] = True
        try:
            res = run_block(ns, str(req.get("block") or "anon"), str(req.get("code") or ""),
                            req.get("file"))
        except KeyboardInterrupt:  # landed between the block and here
            res = {"ok": False, "defines": [], "uses": []}
        finally:
            _executing[0] = False
        out.reset()
        err.reset()
        send({"id": rid, "event": "done", **res,
              "duration_ms": int((time.monotonic() - t0) * 1000)})
        _current[0] = None


if __name__ == "__main__":
    main()
