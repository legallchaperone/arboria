"""Structured outputs for the app's notebook (ADR 0005).

`to_outputs(value)` turns what a command block produced into a list of JSON-ready dicts,
each with a `kind` the app knows how to draw (text, markdown, table, plot, image, error,
pending, run, inspect). The app never renders HTML, so nothing here produces any.

An object may define `_arboria_()` returning one such dict or a list of them; it is
checked (`check`) like everything else: known kinds and fields only, sizes capped.
"""

from __future__ import annotations

import base64
import io
import math
from typing import Any

MAX_TEXT = 200_000
MAX_ROWS = 2_000
MAX_COLS = 60
MAX_POINTS = 200_000
MAX_IMAGE = 8_000_000
MAX_CELL = 2_000

KINDS = {
    "text": {"text", "truncated"},
    "markdown": {"text", "truncated"},
    "table": {"columns", "rows", "total", "title"},
    "plot": {"title", "x", "y", "log", "style", "series", "note"},
    "image": {"mime", "data"},
    "error": {"ename", "evalue", "traceback"},
    "pending": {"task", "what", "name", "ask", "text"},
    "run": {"run", "status", "text"},
    "inspect": {"task", "status", "question", "view", "result", "text"},
}
IMAGE_MIMES = {"image/png", "image/jpeg"}


def _clip(text: str, limit: int = MAX_TEXT) -> tuple[str, bool]:
    return (text, False) if len(text) <= limit else (text[:limit], True)


def text(value: str) -> dict[str, Any]:
    t, cut = _clip(value)
    out: dict[str, Any] = {"kind": "text", "text": t}
    if cut:
        out["truncated"] = True
    return out


def markdown(value: str) -> dict[str, Any]:
    t, cut = _clip(value)
    out: dict[str, Any] = {"kind": "markdown", "text": t}
    if cut:
        out["truncated"] = True
    return out


def _cell(v: Any) -> Any:
    if v is None or isinstance(v, bool):
        return v
    if isinstance(v, int):
        return v if abs(v) < 2**53 else str(v)
    if isinstance(v, float):
        return v if math.isfinite(v) else str(v)
    s = v if isinstance(v, str) else str(v)
    return s if len(s) <= MAX_CELL else s[:MAX_CELL] + "…"


def table(columns: list[Any], rows: list[list[Any]], total: int | None = None,
          title: str | None = None) -> dict[str, Any]:
    cols = [str(c) for c in columns][:MAX_COLS]
    body = [[_cell(c) for c in list(r)[:MAX_COLS]] for r in rows[:MAX_ROWS]]
    out: dict[str, Any] = {"kind": "table", "columns": cols, "rows": body,
                           "total": total if total is not None else len(rows)}
    if title:
        out["title"] = str(title)
    return out


def _num(v: Any) -> float | None:
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return None
    f = float(v)
    return f if math.isfinite(f) else None


def plot(series: list[dict[str, Any]], *, title: str | None = None, x: str | None = None,
         y: str | None = None, log: bool = False, style: str = "line",
         note: str | None = None) -> dict[str, Any]:
    """series: [{"name", "x": [...], "y": [...]}]; non-numbers become gaps (None)."""
    budget = MAX_POINTS
    out_series = []
    for s in series:
        xs = [_num(v) for v in list(s.get("x") or [])][:budget]
        ys = [_num(v) for v in list(s.get("y") or [])][:len(xs)]
        xs = xs[:len(ys)]
        budget -= len(xs)
        out_series.append({"name": str(s.get("name", "")), "x": xs, "y": ys})
        if budget <= 0:
            break
    out: dict[str, Any] = {"kind": "plot", "series": out_series, "log": bool(log),
                           "style": "bar" if style == "bar" else "line"}
    for k, v in (("title", title), ("x", x), ("y", y), ("note", note)):
        if v:
            out[k] = str(v)
    return out


def image(data: bytes, mime: str = "image/png") -> dict[str, Any]:
    if mime not in IMAGE_MIMES:
        raise ValueError(f"image type {mime!r} is not shown")
    if len(data) > MAX_IMAGE:
        return text(f"（图片太大：{len(data) // 1_000_000} MB，最多 {MAX_IMAGE // 1_000_000} MB）")
    return {"kind": "image", "mime": mime, "data": base64.b64encode(data).decode("ascii")}


def error(ename: str, evalue: str, tb: list[str]) -> dict[str, Any]:
    tb_text, _ = _clip("".join(tb), 50_000)
    return {"kind": "error", "ename": ename, "evalue": _clip(evalue, 5_000)[0],
            "traceback": tb_text.splitlines()}


def check(d: Any) -> dict[str, Any]:
    """One output from `_arboria_()`: known kind, known fields, JSON-ready, capped."""
    if not isinstance(d, dict) or d.get("kind") not in KINDS:
        raise ValueError(f"not an arboria output: {d!r:.200}")
    kind = d["kind"]
    extra = set(d) - KINDS[kind] - {"kind"}
    if extra:
        raise ValueError(f"unknown fields for {kind}: {sorted(extra)}")
    if kind == "text":
        return text(str(d.get("text", "")))
    if kind == "markdown":
        return markdown(str(d.get("text", "")))
    if kind == "table":
        return table(list(d.get("columns") or []), list(d.get("rows") or []), d.get("total"),
                     d.get("title"))
    if kind == "plot":
        return plot(list(d.get("series") or []), title=d.get("title"), x=d.get("x"),
                    y=d.get("y"), log=bool(d.get("log")), style=d.get("style") or "line",
                    note=d.get("note"))
    if kind == "image":
        raw = base64.b64decode(str(d.get("data", "")), validate=True)
        return image(raw, str(d.get("mime", "image/png")))
    if kind == "error":
        return error(str(d.get("ename", "")), str(d.get("evalue", "")),
                     [str(x) + "\n" for x in d.get("traceback") or []])
    import json

    out = {k: v for k, v in d.items()}
    json.dumps(out)  # JSON-ready or raise
    for k in ("text", "question", "ask"):
        if isinstance(out.get(k), str):
            out[k] = _clip(out[k], 20_000)[0]
    return out


def _records_table(rows: list[Any]) -> dict[str, Any] | None:
    if not rows or not all(isinstance(r, dict) for r in rows[:MAX_ROWS]):
        return None
    cols: list[str] = []
    for r in rows[:MAX_ROWS]:
        for k in r:
            if str(k) not in cols:
                cols.append(str(k))
    return table(cols, [[r.get(c) for c in cols] for r in rows[:MAX_ROWS]], len(rows))


def _figure(value: Any) -> dict[str, Any] | None:
    """A matplotlib Figure (or Axes) as a PNG; PIL images too."""
    mod = type(value).__module__ or ""
    if mod.startswith("matplotlib"):
        fig = getattr(value, "figure", None) if not hasattr(value, "savefig") else value
        if fig is None or not hasattr(fig, "savefig"):
            return None
        buf = io.BytesIO()
        fig.savefig(buf, format="png", dpi=144, bbox_inches="tight")
        return image(buf.getvalue())
    if mod.startswith("PIL.") and hasattr(value, "save"):
        buf = io.BytesIO()
        value.save(buf, format="PNG")
        return image(buf.getvalue())
    return None


def to_outputs(value: Any) -> list[dict[str, Any]]:
    """What a block's last expression shows (nothing for None)."""
    if value is None:
        return []
    hook = getattr(type(value), "_arboria_", None)
    if callable(hook):
        got = value._arboria_()
        items = got if isinstance(got, list) else [got]
        return [check(x) for x in items if x is not None]
    if isinstance(value, dict) and value.get("kind") in KINDS:
        return [check(value)]
    if isinstance(value, list) and value and all(
            isinstance(v, dict) and v.get("kind") in KINDS for v in value):
        return [check(v) for v in value]
    if isinstance(value, list) and not value:
        return [text("（空）")]
    mod = type(value).__module__ or ""
    if mod.startswith("pandas") and hasattr(value, "to_dict") and hasattr(value, "columns"):
        df = value
        head = df.head(MAX_ROWS)
        cols = [str(c) for c in df.columns]
        rows = head.astype(object).where(head.notna(), None).values.tolist()
        index = getattr(df, "index", None)
        if index is not None and not (type(index).__name__ == "RangeIndex"):
            cols = [str(index.name or "")] + cols
            rows = [[i] + r for i, r in zip(head.index.tolist(), rows)]
        return [table(cols, rows, len(df))]
    if mod.startswith("pandas") and hasattr(value, "to_frame"):
        return to_outputs(value.to_frame())
    fig = _figure(value)
    if fig is not None:
        return [fig]
    if isinstance(value, list):
        t = _records_table(value)
        if t is not None:
            return [t]
    md = getattr(value, "_repr_markdown_", None)
    if callable(md):
        try:
            got = md()
        except Exception:
            got = None
        if isinstance(got, str):
            return [markdown(got)]
    if isinstance(value, str):
        return [text(value)]
    return [text(repr(value))]
