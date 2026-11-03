"""HTML and inline-SVG views of a `Board` (see data.py). No marimo needed here.

Every string that comes from the ledger is HTML-escaped (`_e`): hypotheses, reasons,
summaries and todo text are free text, partly written by an LLM.

Colors: only status carries color (run status, idea confidence), always next to a text
label or inside a legend; everything else is `currentColor` with opacity, so the views
read on marimo's light and dark themes alike. Status steps are mode-invariant.
"""

from __future__ import annotations

import html
import json
import math
import shlex
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import quote

from ..slurm import human_duration
from .data import RUN_FINAL, Board, notebook_path, out_files, parse_ts

UPDATED = "数据更新于 {time}（来自账本；集群状态由 arb watch 更新）"

# status -> (label, css color variable)
RUN_STATUS = {
    "draft": ("草稿", "muted"),
    "awaiting_approval": ("待批准", "warn"),
    "approved": ("已批准", "run"),
    "queued": ("排队中", "run"),
    "running": ("运行中", "run"),
    "succeeded": ("成功", "ok"),
    "failed": ("失败", "bad"),
    "cancelled": ("已取消", "muted"),
    "rejected": ("已拒绝", "muted"),
}
CONFIDENCE = {
    "guess": ("猜测", "muted"),
    "prelim": ("初步", "run"),
    "confirmed": ("已确认", "ok"),
    "rejected": ("已否定", "bad"),
}
IDEA_STATUS = {"open": "进行中", "merged": "已合并", "dropped": "已放弃"}
JOB_STATE = {"submitting": "提交中", "pending": "排队中", "running": "运行中",
             "completed": "完成", "failed": "失败", "cancelled": "已取消",
             "timeout": "超时", "unknown": "未知"}
HOLLOW = ("draft",)  # drawn as an outline only

STYLE = """
<style>
.arb { --arb-ok:#0ca30c; --arb-warn:#e8a200; --arb-bad:#d03b3b; --arb-run:#2f7fdc;
  --arb-muted:#898781; --arb-line:rgba(127,127,127,.32); --arb-soft:rgba(127,127,127,.08);
  font:14px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif; color:inherit; }
.arb h2 { font-size:1.15rem; margin:.2rem 0 .6rem; }
.arb h3 { font-size:.95rem; margin:.1rem 0 .35rem; }
.arb .muted { opacity:.62; }
.arb .count { display:inline-block; min-width:1.4em; padding:0 .45em; border-radius:9px;
  background:var(--arb-soft); font-size:.85em; text-align:center; font-weight:600; }
.arb .group { padding:.45rem 0; border-top:1px solid var(--arb-line); }
.arb .group.empty { opacity:.55; padding:.25rem 0; }
.arb .item { margin:.3rem 0 .55rem; }
.arb code, .arb pre { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.86em; }
.arb code.cmd { user-select:all; padding:.05em .4em; border-radius:4px; background:var(--arb-soft);
  border:1px solid var(--arb-line); white-space:nowrap; }
.arb pre { white-space:pre-wrap; margin:.3rem 0; padding:.5rem .7rem; border-radius:6px;
  background:var(--arb-soft); border:1px solid var(--arb-line); }
.arb .dot { display:inline-block; width:.62em; height:.62em; border-radius:50%;
  margin-right:.3em; vertical-align:baseline; }
.arb .dot.hollow { background:transparent !important; border:1.5px solid currentColor; }
.arb .c-ok { background:var(--arb-ok); } .arb .c-warn { background:var(--arb-warn); }
.arb .c-bad { background:var(--arb-bad); } .arb .c-run { background:var(--arb-run); }
.arb .c-muted { background:var(--arb-muted); }
.arb table { border-collapse:collapse; width:100%; font-size:.9em; }
.arb th { text-align:left; font-weight:600; opacity:.7; border-bottom:1px solid var(--arb-line);
  padding:.25rem .5rem; white-space:nowrap; }
.arb td { padding:.25rem .5rem; border-bottom:1px solid var(--arb-line); vertical-align:top; }
.arb td.num, .arb th.num { text-align:right; font-variant-numeric:tabular-nums; white-space:nowrap; }
.arb .tiles { display:flex; flex-wrap:wrap; gap:.6rem 1.6rem; margin:.4rem 0 .2rem; }
.arb .tile .v { font-size:1.5rem; font-weight:650; line-height:1.2; }
.arb .tile .k { font-size:.8rem; opacity:.65; }
.arb dl.facts { display:grid; grid-template-columns:max-content 1fr; gap:.15rem 1rem; margin:.2rem 0; }
.arb dl.facts dt { opacity:.65; } .arb dl.facts dd { margin:0; }
.arb .meter { height:8px; border-radius:4px; background:var(--arb-soft);
  border:1px solid var(--arb-line); overflow:hidden; max-width:28rem; }
.arb .meter > span { display:block; height:100%; background:currentColor; opacity:.55; }
.arb .meter > span.over { background:var(--arb-bad); opacity:1; }
.arb .legend { display:flex; flex-wrap:wrap; gap:.2rem 1rem; font-size:.82em; opacity:.85; }
.arb svg text { fill:currentColor; }
.arb details > summary { cursor:pointer; }
.arb .warn { color:var(--arb-bad); }
</style>
"""


# ---------------------------------------------------------------- small helpers


def _e(value: Any) -> str:
    return html.escape("" if value is None else str(value), quote=True)


def _clip(text: str | None, n: int) -> str:
    text = " ".join((text or "").split())
    return text if len(text) <= n else text[: n - 1] + "…"


def local(dt: datetime) -> datetime:
    return dt.astimezone()


def fmt_time(ts: str | datetime | None, with_year: bool = False) -> str:
    dt = parse_ts(ts) if isinstance(ts, str) or ts is None else ts
    if dt is None:
        return "—"
    return local(dt).strftime("%Y-%m-%d %H:%M" if with_year else "%m-%d %H:%M")


def age(ts: str | None, now: datetime) -> str:
    """`3 天前`, `5 小时前`, `12 分钟前`, `刚刚`."""
    dt = parse_ts(ts)
    if dt is None:
        return "—"
    s = max(0, (now - dt).total_seconds())
    if s < 60:
        return "刚刚"
    if s < 3600:
        return f"{int(s // 60)} 分钟前"
    if s < 86400:
        return f"{int(s // 3600)} 小时前"
    return f"{int(s // 86400)} 天前"


def fmt_su(value: float | None, unit: bool = True) -> str:
    if value is None:
        return "—"
    text = f"{value:,.2f}"
    return f"{text} SU" if unit else text


def cmd(text: str) -> str:
    """A command for the human's terminal, as select-all code text (never a button)."""
    return f'<code class="cmd">{_e(text)}</code>'


def badge(kind: str, value: str) -> str:
    table = {"run": RUN_STATUS, "confidence": CONFIDENCE}[kind]
    label, color = table.get(value, (value, "muted"))
    hollow = " hollow" if kind == "run" and value in HOLLOW else ""
    hollow = " hollow" if kind == "confidence" and value == "guess" else hollow
    return (f'<span title="{_e(value)}"><span class="dot c-{color}{hollow}"></span>'
            f"{_e(label)}</span>")


def _group(title: str, items: list[str], empty: str = "无") -> str:
    if not items:
        return f'<div class="group empty">{_e(title)} · 0 · {_e(empty)}</div>'
    return (f'<div class="group"><h3>{_e(title)} <span class="count">{len(items)}</span></h3>'
            + "".join(items) + "</div>")


def _resources(run: dict[str, Any]) -> str:
    if not run.get("gpus"):
        return "—"
    return f"{run['gpus']} GPU × {run['walltime']}"


def _table(headers: list[tuple[str, bool]], rows: Iterable[list[str]]) -> str:
    """`headers`: (label, numeric). Cells are HTML already."""
    head = "".join(f'<th class="num">{_e(h)}</th>' if num else f"<th>{_e(h)}</th>"
                   for h, num in headers)
    body = []
    for row in rows:
        cells = "".join(f'<td class="num">{c}</td>' if num else f"<td>{c}</td>"
                        for c, (_, num) in zip(row, headers))
        body.append(f"<tr>{cells}</tr>")
    return f"<table><thead><tr>{head}</tr></thead><tbody>{''.join(body)}</tbody></table>"


def wrap(*parts: str) -> str:
    """One section inside the `.arb` scope. Each carries the (small) style block, so a
    section renders right on its own, whichever cell output the browser shows first."""
    return STYLE + '<div class="arb">' + "".join(parts) + "</div>"


# ---------------------------------------------------------------- header


def header(board: Board) -> str:
    used = board.su_used()
    open_ideas = sum(i["status"] == "open" for i in board.ideas)
    jobs = board.active_jobs()
    tiles = [
        (str(board.needs_you()), "需要你处理"),
        (str(open_ideas), "进行中的 idea"),
        (str(len(jobs)), "活跃作业"),
        (fmt_su(used) + (f" / {board.total_su:,.0f}" if board.total_su else ""), "已用算力"),
    ]
    tile_html = "".join(f'<div class="tile"><div class="v">{_e(v)}</div>'
                        f'<div class="k">{_e(k)}</div></div>' for v, k in tiles)
    project = _e(board.project_id or board.root.name)
    warn = (f'<div class="warn">⚠ {_e(board.config_error)}</div>' if board.config_error else "")
    return wrap(
        f'<h2>Arboria 看板 · {project}</h2>',
        f'<div class="muted">{_e(UPDATED.format(time=local(board.loaded_at).strftime("%H:%M:%S")))}'
        f" · {_e(board.root)}</div>",
        warn, f'<div class="tiles">{tile_html}</div>')


# ---------------------------------------------------------------- 待你处理


def _awaiting_item(board: Board, run: dict[str, Any], open_card: bool) -> str:
    idea = board.idea_by_id(run["idea_id"])
    rid = run["id"]
    head = (f"<b>{_e(rid)}</b> · {_e(idea['name'] if idea else '?')} · "
            f"{_e(_clip(run['change_reason'], 140))} · {_e(_resources(run))}"
            f"{(' · ' + _e(run['partition'])) if run.get('partition') else ''} · "
            f"估计上限 {_e(fmt_su(run['est_cost']))}")
    card = board.cards.get(rid) or {}
    if card.get("text"):
        warn = "" if card.get("matches") else \
            '<div class="warn">⚠ 审核单已变化，请重新 arb submit</div>'
        body = (f'<details{" open" if open_card else ""}><summary>审核单</summary>'
                f"<pre>{_e(card['text'])}</pre></details>{warn}")
    else:
        body = (f'<div class="muted">无法重算审核单（{_e(card.get("error") or "未知原因")}）；'
                f"在终端运行 {cmd('arb card ' + rid)}</div>")
    return (f'<div class="item">{head}<div>在你的终端运行 {cmd("arb approve " + rid)} '
            f"或 {cmd('arb reject ' + rid)}</div>{body}</div>")


def _finished_item(board: Board, run: dict[str, Any]) -> str:
    idea = board.idea_by_id(run["idea_id"])
    n_out, _ = out_files(board, run["id"])
    summary = (f'<div class="muted">{_e(_clip(run["summary"], 200))}</div>'
               if run.get("summary") else "")
    outs = f" · 输出 {n_out} 个文件" if n_out else ""
    return (f'<div class="item"><b>{_e(run["id"])}</b> {badge("run", run["status"])} · '
            f"{_e(idea['name'] if idea else '?')} · {_e(_clip(run['change_reason'], 100))} · "
            f"结束于 {_e(age(run['finished_at'], board.loaded_at))} · "
            f"{_e(fmt_su(run['actual_cost'] or 0))}{_e(outs)}{summary}</div>")


def _lesson_item(lesson: dict[str, Any]) -> str:
    src = f"（来自 {_e(lesson['source_id'])}）" if lesson.get("source_id") else ""
    return (f'<div class="item"><b>{_e(lesson["id"])}</b> {_e(lesson["text"])} '
            f'<span class="muted">{src}</span><div>{cmd("arb lesson accept " + lesson["id"])} '
            f"或 {cmd('arb lesson reject ' + lesson['id'])}</div></div>")


def todo_kind(todo: dict[str, Any]) -> str:
    kind = todo.get("kind") or "plain"
    if kind == "plain":
        return "待办"
    try:
        name = json.loads(todo.get("payload_json") or "{}").get("name", "?")
    except ValueError:
        name = "?"
    return f"命令 {name}" + ("（修改）" if kind == "command_revise" else "（新建）")


def _todo_item(board: Board, todo: dict[str, Any], with_idea: bool = True) -> str:
    where = f" · {_e(todo['idea'])}" if with_idea else ""
    return (f'<div class="item"><b>{_e(todo["id"])}</b> <span class="muted">'
            f"{_e(todo_kind(todo))}{where} · {_e(age(todo['created_at'], board.loaded_at))}</span>"
            f" {_e(_clip(todo['text'], 220))}</div>")


def inbox(board: Board) -> str:
    awaiting = board.awaiting()
    finished = board.finished_unseen()
    groups = [
        _group("待批准的 run", [_awaiting_item(board, r, len(awaiting) <= 2) for r in awaiting]),
        _group("你上次写笔记之后结束的 run",
               [_finished_item(board, r) for r in finished]),
        _group("待确认的经验", [_lesson_item(l) for l in board.lessons]),
        _group("未完成的待办（等 Claude 处理）",
               [_todo_item(board, t) for t in board.todos]),
        _group(f"最近的通知（最新 {len(board.notifications)} 条）",
               [f'<div class="item"><span class="muted">{_e(fmt_time(t))}</span> {_e(msg)}</div>'
                for t, msg in board.notifications], empty="notifications.log 里还没有"),
    ]
    note = ("" if not finished else
            '<div class="muted">结束的 run 会一直列在这里，直到你给那个 idea 写一条笔记'
            "（notebook 里，或在 idea 的 worktree 里 <code>arb note \"…\" --run rNNN</code>）。</div>")
    return wrap(f'<h2>待你处理 <span class="count">{board.needs_you()}</span></h2>',
                *groups, note)


# ---------------------------------------------------------------- idea tree (SVG)


def tree_order(board: Board) -> list[tuple[dict[str, Any], int]]:
    """(idea, depth), parents before children, siblings by creation time."""
    children: dict[int | None, list[dict[str, Any]]] = {}
    ids = {i["id"] for i in board.ideas}
    for i in board.ideas:
        parent = i["parent_id"] if i["parent_id"] in ids else None
        children.setdefault(parent, []).append(i)
    out: list[tuple[dict[str, Any], int]] = []

    def walk(parent: int | None, depth: int) -> None:
        for i in sorted(children.get(parent, []), key=lambda x: (x["created_at"], x["id"])):
            out.append((i, depth))
            walk(i["id"], depth + 1)

    walk(None, 0)
    return out


_STEPS = [(3600, "h"), (3 * 3600, "h"), (6 * 3600, "h"), (12 * 3600, "h"), (86400, "d"),
          (2 * 86400, "d"), (7 * 86400, "d"), (14 * 86400, "d"), (30 * 86400, "d"),
          (90 * 86400, "d"), (365 * 86400, "d")]


def time_ticks(t0: datetime, t1: datetime, target: int = 6) -> list[tuple[datetime, str]]:
    """Round ticks in local time between t0 and t1 with short labels."""
    span = max((t1 - t0).total_seconds(), 1)
    step, unit = next(((s, u) for s, u in _STEPS if span / s <= target), _STEPS[-1])
    lt0 = local(t0)
    if unit == "h":
        start = lt0.replace(minute=0, second=0, microsecond=0)
        start += timedelta(hours=(-start.hour) % (step // 3600))
    else:
        start = lt0.replace(hour=0, minute=0, second=0, microsecond=0)
    ticks = []
    t = start
    while t <= local(t1):
        if t >= lt0:
            ticks.append((t, t.strftime("%H:%M" if unit == "h" and step < 86400 else "%m-%d")))
        t += timedelta(seconds=step)
    if unit == "h":  # mark day changes
        ticks = [(t, t.strftime("%m-%d %H:%M") if t.hour < step // 3600 or i == 0 else lab)
                 for i, (t, lab) in enumerate(ticks)]
    return ticks


def _color_var(color: str) -> str:
    return f"var(--arb-{color})"


def tree_svg(board: Board, show_dropped: bool = True, selected: str | None = None) -> str:
    """One row per idea on a time axis; parent→child links; run ticks by status."""
    rows = [(i, d) for i, d in tree_order(board) if show_dropped or i["status"] != "dropped"]
    if not rows:
        return '<div class="muted">还没有 idea（arb idea new NAME --hypothesis … --metric …）</div>'
    now = board.loaded_at
    t0 = min(parse_ts(i["created_at"]) or now for i, _ in rows)
    t1 = now
    if (t1 - t0).total_seconds() < 3600:
        t0 = t1 - timedelta(hours=1)
    pad = (t1 - t0) * 0.02
    t0, t1 = t0 - pad, t1 + pad
    width, left, right, top, row_h = 960, 200, 16, 30, 28
    height = top + row_h * len(rows) + 8
    span = (t1 - t0).total_seconds()

    def x(ts: str | datetime | None) -> float:
        dt = parse_ts(ts) if isinstance(ts, str) or ts is None else ts
        dt = dt or now
        return round(left + (width - left - right) * (dt - t0).total_seconds() / span, 1)

    y_of = {i["id"]: top + row_h * k + row_h / 2 for k, (i, _) in enumerate(rows)}
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
             f'width="100%" role="img" aria-label="idea 树：时间轴上的 idea 与 run">']
    # axis + grid
    for t, label in time_ticks(t0, t1):
        tx = x(t.astimezone(timezone.utc))
        parts.append(f'<line x1="{tx}" y1="{top - 6}" x2="{tx}" y2="{height - 4}" '
                     'stroke="currentColor" stroke-opacity=".12" stroke-width="1"/>')
        parts.append(f'<text x="{tx}" y="{top - 12}" font-size="11" text-anchor="middle" '
                     f'opacity=".6">{_e(label)}</text>')
    nx = x(now)
    parts.append(f'<line x1="{nx}" y1="{top - 6}" x2="{nx}" y2="{height - 4}" '
                 'stroke="currentColor" stroke-opacity=".45" stroke-dasharray="3 3"/>')
    parts.append(f'<text x="{nx}" y="{height - 1}" font-size="10" text-anchor="end" '
                 'opacity=".55">现在</text>')
    # links first (under the rows)
    for i, _ in rows:
        parent = board.idea_by_id(i["parent_id"])
        if parent is None or parent["id"] not in y_of:
            continue
        fork = board.run(i.get("from_run"))
        x1 = x(fork["created_at"]) if fork else x(i["created_at"])
        y1, y2, x2 = y_of[parent["id"]], y_of[i["id"]], x(i["created_at"])
        mid = (y1 + y2) / 2
        via = f"从 {fork['id']} 分出" if fork else "分支"
        dash = "" if fork else ' stroke-dasharray="4 3"'
        parts.append(f'<path d="M{x1},{y1} C{x1},{mid} {x2},{mid} {x2},{y2}" fill="none" '
                     f'stroke="currentColor" stroke-opacity=".45" stroke-width="1.5"{dash}>'
                     f"<title>{_e(parent['name'])} → {_e(i['name'])}（{_e(via)}）</title></path>")
    for i, depth in rows:
        y = y_of[i["id"]]
        dim = ' opacity=".42"' if i["status"] == "dropped" else ""
        sel = i["name"] == selected
        parts.append(f'<g{dim}>')
        if sel:
            parts.append(f'<rect x="0" y="{y - row_h / 2 + 1}" width="{width}" '
                         f'height="{row_h - 2}" rx="4" fill="currentColor" fill-opacity=".07"/>')
        mark = {"merged": " ◆", "dropped": " ✕"}.get(i["status"], "")
        name = _clip(i["name"], 24 - 2 * min(depth, 4))
        weight = ' font-weight="650"' if sel else ""
        parts.append(f'<text x="{8 + 12 * min(depth, 4)}" y="{y + 4}" font-size="12.5"'
                     f"{weight}>{_e(name)}{_e(mark)}"
                     f"<title>{_e(i['name'])} · {_e(IDEA_STATUS.get(i['status'], i['status']))}"
                     f" · {_e(i['hypothesis'])}</title></text>")
        xs, xe = x(i["created_at"]), x(i["closed_at"] or now)
        parts.append(f'<line x1="{xs + 6}" y1="{y}" x2="{max(xe, xs + 7)}" y2="{y}" '
                     'stroke="currentColor" stroke-opacity=".35" stroke-width="2" '
                     'stroke-linecap="round"/>')
        if i["status"] == "merged":
            parts.append(f'<path d="M{xe},{y - 5} L{xe + 5},{y} L{xe},{y + 5} L{xe - 5},{y} Z" '
                         'fill="currentColor"><title>已合并</title></path>')
        elif i["status"] == "dropped":
            parts.append(f'<path d="M{xe - 4},{y - 4} L{xe + 4},{y + 4} M{xe - 4},{y + 4} '
                         f'L{xe + 4},{y - 4}" stroke="currentColor" stroke-width="1.8">'
                         "<title>已放弃</title></path>")
        for r in board.runs_of(i["id"]):
            label, color = RUN_STATUS.get(r["status"], (r["status"], "muted"))
            rx = x(r["created_at"])
            fill = "none" if r["status"] in HOLLOW else _color_var(color)
            tip = (f"{r['id']} · {label} · {_resources(r)} · "
                   f"{fmt_su(r['actual_cost']) if r['actual_cost'] is not None else '—'} · "
                   f"{_clip(r['change_reason'], 80)}")
            parts.append(f'<rect x="{rx - 2}" y="{y - 8}" width="4" height="16" rx="1.5" '
                         f'style="fill:{fill};stroke:{_color_var(color) if fill == "none" else "none"}">'
                         f"<title>{_e(tip)}</title></rect>")
        conf_label, conf_color = CONFIDENCE.get(i["confidence"], (i["confidence"], "muted"))
        if i["confidence"] == "guess":
            style = "fill:none;stroke:currentColor;stroke-width:1.6"
        else:
            style = f"fill:{_color_var(conf_color)};stroke:none"
        parts.append(f'<circle cx="{xs}" cy="{y}" r="6" style="{style}">'
                     f"<title>{_e(i['name'])} · 置信度 {_e(conf_label)} · 创建于 "
                     f"{_e(fmt_time(i['created_at']))}</title></circle>")
        parts.append("</g>")
    parts.append("</svg>")
    return "".join(parts)


def tree_legend() -> str:
    conf = "".join(f'<span>{badge("confidence", k)}</span>' for k in CONFIDENCE)
    runs = "".join(f'<span><span class="dot c-{c}{" hollow" if k in HOLLOW else ""}" '
                   f'style="border-radius:2px"></span>{_e(lab)}</span>'
                   for k, (lab, c) in RUN_STATUS.items())
    return (f'<div class="legend"><span class="muted">idea 置信度：</span>{conf}</div>'
            f'<div class="legend"><span class="muted">run：</span>{runs}'
            '<span>◆ 已合并</span><span>✕ 已放弃（变淡）</span>'
            '<span class="muted">虚线：从分支头分出；实线：从某个 run 的快照分出</span></div>')


def tree(board: Board, show_dropped: bool = True, selected: str | None = None) -> str:
    dropped = [i["name"] for i in board.ideas if i["status"] == "dropped"]
    hidden = ("" if show_dropped or not dropped else
              f'<div class="muted">已隐藏 {len(dropped)} 个已放弃的 idea：'
              f"{_e('、'.join(dropped))}</div>")
    return wrap("<h2>idea 树</h2>", tree_svg(board, show_dropped, selected), tree_legend(),
                hidden)


# ---------------------------------------------------------------- idea detail


def idea_options(board: Board) -> dict[str, str]:
    """Dropdown label -> idea name: open ideas first (most recently active first)."""
    ordered = sorted(board.ideas, key=board.last_active, reverse=True)
    ordered.sort(key=lambda i: i["status"] != "open")  # stable: open first
    return {f"{i['name']} · {IDEA_STATUS.get(i['status'], i['status'])} · "
            f"{CONFIDENCE.get(i['confidence'], (i['confidence'],))[0]}": i["name"]
            for i in ordered}


def default_idea(board: Board, requested: str | None = None) -> str | None:
    """`--idea` if it exists, else the most recently active open idea, else any."""
    if requested and board.idea(requested):
        return requested
    names = list(idea_options(board).values())
    return names[0] if names else None


def _overrides(req: dict[str, Any]) -> dict[str, str]:
    return {str(k): str(v) for k, v in req.get("overrides") or []}


def changes_vs_parent(board: Board, run: dict[str, Any]) -> list[str]:
    """`k: old → new` for overrides (and resources) that differ from the parent run."""
    mine = _overrides(run["request"])
    parent = board.run(run.get("parent_run"))
    if parent is None:
        return [f"{k}={v}" for k, v in mine.items()]
    theirs = _overrides(parent["request"])
    out = [f"{k}: {theirs.get(k, '—')} → {v}" for k, v in mine.items() if theirs.get(k) != v]
    for key, label in (("gpus", "GPU"), ("walltime", "时长"), ("partition", "分区")):
        if run.get(key) != parent.get(key) and run.get(key) is not None:
            out.append(f"{label}: {parent.get(key) or '—'} → {run.get(key)}")
    return out


def vscode_link(path: Path) -> str:
    return "vscode://file" + quote(str(path.resolve() if path.exists() else path))


def runs_table(board: Board, idea: dict[str, Any]) -> str:
    runs = board.runs_of(idea["id"])
    if not runs:
        return '<div class="muted">还没有 run</div>'
    rows = []
    for r in reversed(runs):
        n_out, names = out_files(board, r["id"])
        outs = (f'<span title="{_e(", ".join(names))}">{n_out} 个</span>' if n_out else "—")
        wandb = (f'<a href="{_e(r["wandb_url"])}" target="_blank" rel="noopener">wandb</a>'
                 if r.get("wandb_url") else "—")
        changes = changes_vs_parent(board, r)
        rows.append([
            f"<b>{_e(r['id'])}</b>", _e(r.get("parent_run") or "—"), badge("run", r["status"]),
            _e(_resources(r)), _e(r.get("partition") or "—"),
            _e(fmt_su(r["est_cost"], unit=False)), _e(fmt_su(r["actual_cost"], unit=False)),
            _e(fmt_time(r["created_at"])), wandb,
            "<br>".join(_e(c) for c in changes) or "—",
            f'<span title="{_e(r["change_reason"])}">{_e(_clip(r["change_reason"], 60))}</span>',
            outs,
        ])
    return _table([("run", False), ("父 run", False), ("状态", False), ("资源", False),
                   ("分区", False), ("估计 SU", True), ("实际 SU", True), ("创建", False),
                   ("wandb", False), ("相对父 run 的改动", False), ("原因", False),
                   ("输出", True)], rows)


def idea_detail(board: Board, name: str | None) -> str:
    idea = board.idea(name) if name else None
    if idea is None:
        return wrap("<h2>idea 详情</h2>", '<div class="muted">没有选中的 idea</div>')
    nb = notebook_path(board, idea["name"])
    if nb.exists():
        notebook = (f'<a href="{_e(vscode_link(nb))}">{_e(nb)}</a> · '
                    f"{cmd('marimo edit ' + shlex.quote(str(nb)))}")
    else:
        notebook = (f'<span class="muted">没有 notebook</span> · '
                    f"{cmd('arb idea notebook ' + idea['name'])}")
    parent = idea.get("parent_name")
    origin = (f"{_e(parent)}" + (f"（从 {_e(idea['from_run'])} 的快照分出）"
                                 if idea.get("from_run") else "")) if parent else "—"
    closed = f" · 关闭于 {_e(fmt_time(idea['closed_at']))}" if idea.get("closed_at") else ""
    facts = [
        ("假设", _e(idea["hypothesis"])), ("指标", _e(idea["metric"])),
        ("置信度", badge("confidence", idea["confidence"])),
        ("状态", _e(IDEA_STATUS.get(idea["status"], idea["status"])) + closed),
        ("父 idea", origin),
        ("分支", f"<code>{_e(idea['branch'])}</code>"),
        ("worktree", f"<code>{_e(idea['worktree_path'])}</code>"
         + ("" if idea["status"] == "open" else ' <span class="muted">（已删除）</span>')),
        ("notebook", notebook),
        ("创建", _e(fmt_time(idea["created_at"], with_year=True))),
        ("已用算力", _e(fmt_su(board.su_used(idea["id"])))),
    ]
    dl = "".join(f"<dt>{_e(k)}</dt><dd>{v}</dd>" for k, v in facts)
    notes = board.human_notes(idea["id"])
    note_items = []
    for n in notes:
        tags = []
        if n.get("run_id"):
            tags.append(n["run_id"])
        if n.get("confidence"):
            tags.append("置信度 → " + CONFIDENCE.get(n["confidence"], (n["confidence"],))[0])
        tag = f" <span class=\"muted\">[{_e(', '.join(tags))}]</span>" if tags else ""
        note_items.append(f'<div class="item"><span class="muted">{_e(fmt_time(n["created_at"]))}'
                          f"</span>{tag} {_e(n['text'])}</div>")
    summaries = [
        f"<details><summary><b>{_e(r['id'])}</b> {badge('run', r['status'])} "
        f"{_e(_clip(r['summary'], 90))}</summary><pre>{_e(r['summary'])}</pre></details>"
        for r in reversed(board.runs_of(idea["id"])) if r.get("summary")]
    todos = [t for t in board.todos if t["idea_id"] == idea["id"]]
    return wrap(
        f"<h2>idea 详情 · {_e(idea['name'])}</h2>",
        f'<dl class="facts">{dl}</dl>',
        f"<h3>runs（{len(board.runs_of(idea['id']))}）</h3>", runs_table(board, idea),
        _group("你的笔记", note_items, empty="还没有笔记"),
        _group("run 总结（点开看全文）", summaries, empty="还没有 run 总结"),
        _group("未完成的待办", [_todo_item(board, t, with_idea=False) for t in todos]),
    )


# ---------------------------------------------------------------- 算力与预算


def su_bars(board: Board) -> str:
    rows = [(i, board.su_used(i["id"])) for i in board.ideas]
    rows = sorted([(i, su) for i, su in rows if su > 0], key=lambda p: -p[1])
    if not rows:
        return '<div class="muted">还没有花费（jobs.billed_su 为空）</div>'
    top = max(su for _, su in rows)
    width, label_w, value_w, row_h = 640, 170, 90, 22
    bar_w = width - label_w - value_w
    height = row_h * len(rows) + 4
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
             f'width="100%" style="max-width:{width}px" role="img" aria-label="各 idea 已用 SU">']
    for k, (i, su) in enumerate(rows):
        y = k * row_h + 2
        w = max(2.0, bar_w * su / top)
        dim = ' opacity=".5"' if i["status"] == "dropped" else ""
        parts.append(f"<g{dim}>")
        parts.append(f'<text x="0" y="{y + 15}" font-size="12">{_e(_clip(i["name"], 22))}</text>')
        parts.append(f'<rect x="{label_w}" y="{y + 4}" width="{w:.1f}" height="14" rx="3" '
                     f'fill="currentColor" fill-opacity=".55"><title>{_e(i["name"])}: '
                     f"{_e(fmt_su(su))}</title></rect>")
        parts.append(f'<text x="{width}" y="{y + 15}" font-size="12" text-anchor="end" '
                     f'style="font-variant-numeric:tabular-nums">{_e(fmt_su(su))}</text>')
        parts.append("</g>")
    parts.append("</svg>")
    return "".join(parts)


def su_line(board: Board) -> str:
    points = board.su_timeline()
    if not points:
        return '<div class="muted">还没有花费记录</div>'
    now = board.loaded_at
    t0 = points[0][0]
    t1 = max(now, points[-1][0])
    if (t1 - t0).total_seconds() < 3600:
        t0 = t1 - timedelta(hours=1)
    total = board.total_su
    peak = points[-1][1]
    show_budget = total > 0 and peak >= 0.25 * total
    ymax = max(peak, total if show_budget else 0) * 1.08 or 1
    width, height, left, right, top, bottom = 640, 200, 56, 12, 10, 24
    span = (t1 - t0).total_seconds()

    def x(t: datetime) -> float:
        return round(left + (width - left - right) * (t - t0).total_seconds() / span, 1)

    def y(v: float) -> float:
        return round(top + (height - top - bottom) * (1 - v / ymax), 1)

    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
             f'width="100%" style="max-width:{width}px" role="img" aria-label="累计 SU">']
    for v in nice_ticks(ymax):
        parts.append(f'<line x1="{left}" x2="{width - right}" y1="{y(v)}" y2="{y(v)}" '
                     'stroke="currentColor" stroke-opacity=".12"/>')
        parts.append(f'<text x="{left - 6}" y="{y(v) + 4}" font-size="11" text-anchor="end" '
                     f'opacity=".6" style="font-variant-numeric:tabular-nums">{v:g}</text>')
    for t, label in time_ticks(t0, t1, target=5):
        tx = x(t.astimezone(timezone.utc))
        parts.append(f'<text x="{tx}" y="{height - 6}" font-size="11" text-anchor="middle" '
                     f'opacity=".6">{_e(label)}</text>')
    path = [f"M{x(t0)},{y(0)}"]
    for t, v, _ in points:
        path.append(f"H{x(t)}")
        path.append(f"V{y(v)}")
    path.append(f"H{x(t1)}")
    parts.append(f'<path d="{" ".join(path)}" fill="none" stroke="currentColor" '
                 'stroke-width="2" stroke-linejoin="round"/>')
    for t, v, run_id in points:
        parts.append(f'<circle cx="{x(t)}" cy="{y(v)}" r="5" fill="currentColor" '
                     f'fill-opacity="0"><title>{_e(fmt_time(t))} · {_e(run_id)} · '
                     f"累计 {_e(fmt_su(v))}</title></circle>")
    if show_budget:
        parts.append(f'<line x1="{left}" x2="{width - right}" y1="{y(total)}" y2="{y(total)}" '
                     'style="stroke:var(--arb-bad)" stroke-dasharray="5 4" stroke-width="1.5"/>')
        parts.append(f'<text x="{width - right}" y="{y(total) - 4}" font-size="11" '
                     f'text-anchor="end">预算 {total:g} SU</text>')
    parts.append(f'<text x="{x(t1)}" y="{y(peak) - 6}" font-size="11.5" text-anchor="end" '
                 f'font-weight="600">{_e(fmt_su(peak))}</text>')
    parts.append("</svg>")
    return "".join(parts)


def nice_ticks(vmax: float, n: int = 4) -> list[float]:
    if vmax <= 0:
        return [0]
    raw = vmax / n
    mag = 10 ** math.floor(math.log10(raw))
    step = next(m * mag for m in (1, 2, 2.5, 5, 10) if m * mag >= raw)
    return [round(k * step, 6) for k in range(int(vmax // step) + 1)]


def budget(board: Board) -> str:
    used = board.su_used()
    total = board.total_su
    pending = sum(r["est_cost"] or 0 for r in board.awaiting())
    running = [r for r in board.runs if r["status"] in ("approved", "queued", "running")]
    committed = sum(max(0.0, (r["est_cost"] or 0) - sum(j["billed_su"] or 0 for j in r["jobs"]))
                    for r in running)
    if total > 0:
        pct = used / total * 100
        over = " over" if used > total else ""
        head = (f'<div class="tiles"><div class="tile"><div class="v">{_e(fmt_su(used))}</div>'
                f'<div class="k">已用（共 {total:,.0f} SU，{pct:.1f}%）</div></div>'
                f'<div class="tile"><div class="v">{_e(fmt_su(total - used))}</div>'
                f'<div class="k">剩余</div></div>'
                f'<div class="tile"><div class="v">{_e(fmt_su(committed))}</div>'
                f'<div class="k">进行中的 run 最多还会花</div></div>'
                f'<div class="tile"><div class="v">{_e(fmt_su(pending))}</div>'
                f'<div class="k">待批准的 run 估计上限</div></div></div>'
                f'<div class="meter"><span class="{over.strip()}" '
                f'style="width:{min(pct, 100):.1f}%"></span></div>')
    else:
        head = (f'<div class="tiles"><div class="tile"><div class="v">{_e(fmt_su(used))}</div>'
                f'<div class="k">已用</div></div></div>'
                '<div class="muted">未设项目预算（arboria.toml <code>[budget] total_su</code>）</div>')
    jobs = board.active_jobs()
    job_rows = [[f"<b>{_e(j['run']['id'])}</b>",
                 _e((board.idea_by_id(j["run"]["idea_id"]) or {}).get("name", "?")),
                 f"s{j['segment']}", _e(j["job_id"] or "—"),
                 _e(JOB_STATE.get(j["state"], j["state"])),
                 _e(human_duration(j["elapsed_s"]) if j["elapsed_s"] else "—"),
                 _e(fmt_su(j["billed_su"], unit=False)),
                 _e(age(j["updated_at"], board.loaded_at))] for j in jobs]
    jobs_html = (_table([("run", False), ("idea", False), ("段", False), ("作业", False),
                         ("状态", False), ("已运行", True), ("SU", True), ("账本更新", False)],
                        job_rows) if jobs else '<div class="muted">没有活跃作业</div>')
    done = [r for r in board.runs if r["status"] in RUN_FINAL
            and r["est_cost"] is not None and r["actual_cost"] is not None]
    est_rows = [[f"<b>{_e(r['id'])}</b>",
                 _e((board.idea_by_id(r["idea_id"]) or {}).get("name", "?")),
                 badge("run", r["status"]), _e(_resources(r)),
                 _e(fmt_su(r["est_cost"], unit=False)), _e(fmt_su(r["actual_cost"], unit=False)),
                 _e(f"{r['actual_cost'] / r['est_cost'] * 100:.0f}%" if r["est_cost"] else "—")]
                for r in reversed(done)]
    est_html = (_table([("run", False), ("idea", False), ("状态", False), ("资源", False),
                        ("估计上限 SU", True), ("实际 SU", True), ("实际/估计", True)], est_rows)
                if done else '<div class="muted">还没有既有估计又有实际花费的已结束 run</div>')
    return wrap("<h2>算力与预算</h2>", head,
                "<h3>各 idea 已用</h3>", su_bars(board),
                "<h3>累计花费</h3>", su_line(board),
                '<div class="muted">按每个作业最后一次写进账本的时间累计（arb poll / arb watch）。</div>',
                f"<h3>活跃作业 <span class=\"count\">{len(jobs)}</span></h3>", jobs_html,
                "<h3>估计 vs 实际（已结束的 run）</h3>", est_html)
