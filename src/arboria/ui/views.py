"""HTML and inline-SVG views of a `Board` (see data.py). No marimo needed here.

Every string that comes from the ledger is HTML-escaped (`_e`): hypotheses, reasons,
summaries and todo text are free text, partly written by an LLM.

Styling lives in `dashboard.css` (next to this file): the marimo app loads it with
`App(css_file=...)`, and `wrap` / `page` embed it for standalone HTML. All colours are
CSS variables (`--arb-*`) with a light and a dark set, so the SVGs carry no hard-coded
colour. Status colour is always paired with a text label; in the idea tree only the
exceptions (a failed run, a run waiting for approval) are coloured at all.

Sections show the short version first; the rest is in collapsed `<details>`.
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
from .data import RUN_FINAL, Board, Garden, notebook_path, out_files, parse_ts

CSS_FILE = Path(__file__).with_name("dashboard.css")
UPDATED = "更新于 {time}"
UPDATED_TIP = "数据来自账本；集群状态由 arb watch / arb poll 写入"
SHOWN = 3  # notes, summaries, notifications: the newest few, the rest collapsed

# status -> (label, pill class)
RUN_STATUS = {
    "draft": ("草稿", "draft"),
    "awaiting_approval": ("待批准", "warn"),
    "approved": ("已批准", "run"),
    "queued": ("排队中", "run"),
    "running": ("运行中", "run"),
    "succeeded": ("成功", "ok"),
    "failed": ("失败", "bad"),
    "cancelled": ("已取消", "slate"),
    "rejected": ("已拒绝", "slate"),
}
CONFIDENCE = {
    "guess": ("猜测", "guess"),
    "prelim": ("初步", "prelim"),
    "confirmed": ("已确认", "confirmed"),
    "rejected": ("已否定", "rejected"),
}
IDEA_STATUS = {"open": "进行中", "merged": "已合并", "dropped": "已放弃"}
JOB_STATE = {"submitting": "提交中", "pending": "排队中", "running": "运行中",
             "completed": "完成", "failed": "失败", "cancelled": "已取消",
             "timeout": "超时", "unknown": "未知"}
HOLLOW = ("draft",)  # drawn as an outline only
# idea tree: run ticks are neutral; only these stand out
TICK_COLOR = {"failed": "var(--arb-bad)", "awaiting_approval": "var(--arb-warn)"}


def css() -> str:
    return CSS_FILE.read_text(encoding="utf-8")


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


def cmd(text: str, alt: bool = False) -> str:
    """A command for the human's terminal, as select-all code text (never a button)."""
    cls = "cmd alt" if alt else "cmd"
    return f'<code class="{cls}" title="{_e(text)}">{_e(text)}</code>'


def pill(value: str) -> str:
    """Run status as a small tinted pill with its label."""
    label, color = RUN_STATUS.get(value, (value, "slate"))
    return f'<span class="pill s-{color}" title="{_e(value)}">{_e(label)}</span>'


def confidence(value: str) -> str:
    """Idea confidence: its own small scale, always with the label (guess is hollow)."""
    label, cls = CONFIDENCE.get(value, (value, "guess"))
    return f'<span class="conf c-{cls}" title="置信度">{_e(label)}</span>'


def _sep(*parts: str) -> str:
    """Already-escaped parts joined by a faint middle dot."""
    return '<span class="sep">·</span>'.join(p for p in parts if p)


def _head(title: str, count: int | None = None, controls: str = "", hot: bool = False,
          tag: str = "h2") -> str:
    badge_ = ""
    if count is not None:
        badge_ = f' <span class="count{" hot" if hot and count else ""}">{count}</span>'
    ctl = f'<div class="controls">{controls}</div>' if controls else ""
    return (f'<div class="sec-head"><div class="title"><{tag}>{_e(title)}</{tag}>{badge_}</div>'
            f"{ctl}</div>")


def _section(cls: str, *parts: str) -> str:
    return f'<section class="arb card {cls}">' + "".join(parts) + "</section>"


def _group(title: str, items: list[str], tip: str = "", newest: bool = False) -> str:
    """A titled group with its count; `newest`: only the first few shown."""
    if not items:
        return f'<div class="group empty">{_e(title)} · 无</div>'
    tip_attr = f' title="{_e(tip)}"' if tip else ""
    body = _newest(items) if newest else "".join(items)
    return (f'<div class="group"><h3{tip_attr}>{_e(title)} <span class="count">{len(items)}'
            "</span></h3>" + body + "</div>")


def _newest(items: list[str], label: str = "更早") -> str:
    """The first SHOWN items, the rest behind one disclosure."""
    shown, rest = items[:SHOWN], items[SHOWN:]
    more = (f'<details class="more"><summary>{_e(label)} {len(rest)} 条</summary>'
            + "".join(rest) + "</details>") if rest else ""
    return "".join(shown) + more


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
    return (f'<div class="tbl"><table><thead><tr>{head}</tr></thead>'
            f"<tbody>{''.join(body)}</tbody></table></div>")


def wrap(*parts: str) -> str:
    """Standalone HTML: the stylesheet plus the parts on the page background. (Inside
    marimo the same stylesheet comes from the app's css_file.)"""
    return f"<style>{css()}</style>" + '<div class="arb-page">' + "".join(parts) + "</div>"


def columns(left: str, right: str) -> str:
    """Two columns on wide screens (inbox | tree + detail), one on narrow ones."""
    return (f'<div class="arb-grid"><div class="arb-col">{left}</div>'
            f'<div class="arb-col">{right}</div></div>')


def page(board: Board, selected: str | None = None, show_dropped: bool = True) -> str:
    """The whole dashboard as one standalone HTML fragment (same layout as the app)."""
    name = default_idea(board, selected)
    return wrap(header(board),
                columns(inbox(board), tree(board, show_dropped, name) + idea_detail(board, name)),
                budget(board))


# ---------------------------------------------------------------- header


def header(board: Board, controls: str = "") -> str:
    used = board.su_used()
    open_ideas = sum(i["status"] == "open" for i in board.ideas)
    jobs = board.active_jobs()
    need = board.needs_you()

    def kpi(label: str, value: str, extra: str = "", hot: bool = False) -> str:
        return (f'<div class="kpi{" hot" if hot else ""}"><div class="k">{_e(label)}</div>'
                f'<div class="v">{value}</div>{extra}</div>')

    if board.total_su:
        pct = used / board.total_su * 100
        su_value = (f"{_e(fmt_su(used, unit=False))}<small>/ {board.total_su:,.0f} SU</small>")
        su_extra = (f'<div class="meter" title="{pct:.1f}%"><span class="'
                    f'{"over" if used > board.total_su else ""}" '
                    f'style="width:{min(pct, 100):.1f}%"></span></div>')
    else:
        su_value, su_extra = f"{_e(fmt_su(used, unit=False))}<small>SU</small>", ""
    kpis = "".join([
        kpi("需要你处理", str(need), hot=need > 0),
        kpi("进行中的 idea", str(open_ideas)),
        kpi("活跃作业", str(len(jobs))),
        kpi("已用 SU", su_value, su_extra),
    ])
    project = _e(board.project_id or board.root.name)
    updated = _e(UPDATED.format(time=local(board.loaded_at).strftime("%H:%M:%S")))
    if board.home_thinking:  # a quiet pointer to the seed home
        updated += (f'<span class="sep">·</span><span class="garden-link">种子园：'
                    f"{board.home_thinking} 个在思考中</span>"
                    f'<span class="sep">·</span><code>arb ui --home</code>')
    warn = (f'<div class="warn-line">{_e(board.config_error)}</div>'
            if board.config_error else "")
    ctl = f'<div class="controls">{controls}</div>' if controls else ""
    return (f'<section class="arb head"><div class="titlebar"><div>'
            f'<h1>Arboria 看板<span class="proj">{project}</span></h1>'
            f'<div class="meta" title="{_e(UPDATED_TIP)}">{updated}</div></div>{ctl}</div>'
            f'{warn}<div class="kpis">{kpis}</div></section>')


# ---------------------------------------------------------------- 待你处理


def _idea_name(board: Board, run: dict[str, Any]) -> str:
    idea = board.idea_by_id(run["idea_id"])
    return idea["name"] if idea else "?"


def _line(head: str, text: str, end: str = "", tip: str | None = None) -> str:
    tip_attr = f' title="{_e(tip)}"' if tip else ""
    end_html = f'<span class="end">{end}</span>' if end else ""
    return (f'<div class="line">{head}<span class="text"{tip_attr}>{text}</span>'
            f"{end_html}</div>")


def _awaiting_item(board: Board, run: dict[str, Any]) -> str:
    rid = run["id"]
    est = f"≤ {fmt_su(run['est_cost'])}" if run.get("est_cost") is not None else ""
    text = _sep(_e(_idea_name(board, run)), _e(_clip(run["change_reason"], 40)), _e(est))
    line = _line(f'<span class="id">{_e(rid)}</span>', text, cmd("arb approve " + rid),
                 tip=run["change_reason"])
    card = board.cards.get(rid) or {}
    if card.get("text"):
        warn = "" if card.get("matches") else \
            '<div class="changed">审核单已变化，请重新 arb submit</div>'
        body = (f'{warn}<details class="more"><summary>审核单</summary>'
                f"<pre>{_e(card['text'])}</pre>"
                f'<div class="card-actions">{cmd("arb reject " + rid, alt=True)}</div></details>')
    else:
        body = (f'<div class="card-actions">无法重算审核单：{_e(card.get("error") or "未知原因")}'
                f" {cmd('arb card ' + rid)} {cmd('arb reject ' + rid, alt=True)}</div>")
    return f'<div class="item">{line}{body}</div>'


def _finished_item(board: Board, run: dict[str, Any]) -> str:
    text = _sep(_e(_idea_name(board, run)), _e(_clip(run["change_reason"], 40)))
    when = f'<span class="when">{_e(age(run["finished_at"], board.loaded_at))}</span>'
    return ('<div class="item">' + _line(
        f'<span class="id">{_e(run["id"])}</span>{pill(run["status"])}', text, when,
        tip=run.get("summary") or run["change_reason"]) + "</div>")


def _lesson_item(lesson: dict[str, Any]) -> str:
    src = (f' <span class="faint">{_e(lesson["source_id"])}</span>'
           if lesson.get("source_id") else "")
    return (f'<div class="item"><div class="line"><span class="id">{_e(lesson["id"])}</span>'
            f'<span class="text" style="white-space:normal">{_e(lesson["text"])}{src}</span></div>'
            f'<div class="card-actions">{cmd("arb lesson accept " + lesson["id"])}'
            f"{cmd('arb lesson reject ' + lesson['id'], alt=True)}</div></div>")


def todo_kind(todo: dict[str, Any]) -> str:
    """'' for a plain todo; `写命令 svd` / `改命令 svd` for command todos."""
    kind = todo.get("kind") or "plain"
    if kind == "plain":
        return ""
    try:
        name = json.loads(todo.get("payload_json") or "{}").get("name", "?")
    except ValueError:
        name = "?"
    return ("改命令 " if kind == "command_revise" else "写命令 ") + str(name)


def _todo_item(board: Board, todo: dict[str, Any], with_idea: bool = True) -> str:
    kind = todo_kind(todo)
    tag = f'<span class="tag">{_e(kind)}</span> ' if kind else ""
    text = _sep(_e(todo["idea"]) if with_idea else "", tag + _e(_clip(todo["text"], 120)))
    when = f'<span class="when">{_e(age(todo["created_at"], board.loaded_at))}</span>'
    return ('<div class="item">' + _line(f'<span class="id">{_e(todo["id"])}</span>', text, when,
                                         tip=todo["text"]) + "</div>")


def _notification(t: datetime | None, msg: str) -> str:
    return ('<div class="item">' + _line(f'<span class="when">{_e(fmt_time(t))}</span>',
                                         _e(msg), tip=msg) + "</div>")


def inbox(board: Board) -> str:
    awaiting = board.awaiting()
    finished = board.finished_unseen()
    notes = [_notification(t, msg) for t, msg in board.notifications]
    groups = [
        _group("待批准的 run", [_awaiting_item(board, r) for r in awaiting]),
        _group("新结束的 run", [_finished_item(board, r) for r in finished],
               tip="你上次给这个 idea 写笔记之后结束；写一条笔记后不再列出"),
        _group("待确认的经验", [_lesson_item(l) for l in board.lessons]),
        _group("等 Claude 的待办", [_todo_item(board, t) for t in board.todos]),
        _group("最近通知", notes, newest=True),
    ]
    return _section("inbox", _head("待你处理", board.needs_you(), hot=True), *groups)


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


def tree_svg(board: Board, show_dropped: bool = True, selected: str | None = None) -> str:
    """One lane per idea on a time axis, parent→child connectors, run ticks; the
    idea's confidence (or merged / dropped) as a short label at the end of its row."""
    rows = [(i, d) for i, d in tree_order(board) if show_dropped or i["status"] != "dropped"]
    if not rows:
        return ('<div class="muted">还没有 idea · '
                f'{cmd("arb idea new NAME --hypothesis … --metric …")}</div>')
    now = board.loaded_at
    t0 = min(parse_ts(i["created_at"]) or now for i, _ in rows)
    t1 = now
    if (t1 - t0).total_seconds() < 3600:
        t0 = t1 - timedelta(hours=1)
    pad = (t1 - t0) * 0.03
    t0, t1 = t0 - pad, t1 + pad
    width, left, right, top, row_h = 780, 112, 112, 30, 34
    height = top + row_h * len(rows) + 6
    span = (t1 - t0).total_seconds()
    plot_r = width - right

    def x(ts: str | datetime | None) -> float:
        dt = parse_ts(ts) if isinstance(ts, str) or ts is None else ts
        dt = dt or now
        return round(left + (plot_r - left) * (dt - t0).total_seconds() / span, 1)

    y_of = {i["id"]: top + row_h * k + row_h / 2 for k, (i, _) in enumerate(rows)}
    parts = [f'<svg class="tree" xmlns="http://www.w3.org/2000/svg" '
             f'viewBox="0 0 {width} {height}" role="img" '
             'aria-label="idea 树：时间轴上的 idea 与 run">']
    # selected lane under everything
    for i, _ in rows:
        if i["name"] == selected:
            parts.append(f'<rect x="0" y="{y_of[i["id"]] - row_h / 2 + 2}" width="{width}" '
                         f'height="{row_h - 4}" rx="7" style="fill:var(--arb-accent-soft)"/>')
    # faint time grid with a few labels
    nx = x(now)
    for t, label in time_ticks(t0, t1, target=6):
        tx = x(t.astimezone(timezone.utc))
        parts.append(f'<line x1="{tx}" y1="{top - 4}" x2="{tx}" y2="{height - 4}" '
                     'style="stroke:var(--arb-grid)" stroke-width="1"/>')
        if abs(tx - nx) > 40:
            parts.append(f'<text x="{tx}" y="{top - 12}" font-size="12.5" text-anchor="middle" '
                         f'class="t-faint">{_e(label)}</text>')
    parts.append(f'<line x1="{nx}" y1="{top - 4}" x2="{nx}" y2="{height - 4}" '
                 'style="stroke:var(--arb-faint)" stroke-opacity=".6" stroke-dasharray="2 3"/>')
    parts.append(f'<text x="{nx}" y="{top - 12}" font-size="12.5" text-anchor="middle" '
                 'class="t-faint">现在</text>')
    # connectors (under the lanes): one style
    for i, _ in rows:
        parent = board.idea_by_id(i["parent_id"])
        if parent is None or parent["id"] not in y_of:
            continue
        fork = board.run(i.get("from_run"))
        x1 = x(fork["created_at"]) if fork else x(i["created_at"])
        y1, y2, x2 = y_of[parent["id"]], y_of[i["id"]], x(i["created_at"])
        mid = (y1 + y2) / 2
        via = f"从 {fork['id']} 分出" if fork else "分支"
        parts.append(f'<path d="M{x1},{y1} C{x1},{mid} {x2},{mid} {x2},{y2}" fill="none" '
                     'style="stroke:var(--arb-faint)" stroke-width="1.25">'
                     f"<title>{_e(parent['name'])} → {_e(i['name'])}（{_e(via)}）</title></path>")
    for i, depth in rows:
        y = y_of[i["id"]]
        dim = ' opacity=".42"' if i["status"] == "dropped" else ""
        sel = i["name"] == selected
        parts.append(f"<g{dim}>")
        indent = 12 * min(depth, 4)
        name = _clip(i["name"], max(6, (left - 16 - indent) // 8))
        cls = "t-ink" if sel or i["status"] == "open" else ""
        weight = ' font-weight="600"' if sel else ""
        parts.append(f'<text x="{8 + indent}" y="{y + 5}" font-size="13.5" class="{cls}"'
                     f"{weight}>{_e(name)}"
                     f"<title>{_e(i['name'])} · {_e(IDEA_STATUS.get(i['status'], i['status']))}"
                     f" · {_e(i['hypothesis'])}</title></text>")
        xs, xe = x(i["created_at"]), x(i["closed_at"] or now)
        parts.append(f'<rect x="{xs - 5}" y="{y - 7}" width="{max(xe - xs, 0) + 10:.1f}" '
                     f'height="14" rx="7" style="fill:var(--arb-lane)"/>')
        parts.append(f'<circle cx="{xs}" cy="{y}" r="3" style="fill:var(--arb-faint)"/>')
        for r in board.runs_of(i["id"]):
            label = RUN_STATUS.get(r["status"], (r["status"], ""))[0]
            rx = x(r["created_at"])
            color = TICK_COLOR.get(r["status"], "var(--arb-tick)")
            if r["status"] in HOLLOW:
                style = f"fill:var(--arb-surface);stroke:{color};stroke-width:1.25"
            else:
                style = f"fill:{color}"
            tip = (f"{r['id']} · {label} · {_resources(r)} · "
                   f"{fmt_su(r['actual_cost']) if r['actual_cost'] is not None else '—'} · "
                   f"{_clip(r['change_reason'], 80)}")
            parts.append(f'<rect x="{rx - 2.5}" y="{y - 8}" width="5" height="16" rx="2.5" '
                         f'style="{style}"><title>{_e(tip)}</title></rect>')
        conf_label, conf_cls = CONFIDENCE.get(i["confidence"], (i["confidence"], "guess"))
        status = {"merged": "已合并", "dropped": "已放弃"}.get(i["status"])
        lx = plot_r + 14
        parts.append(f'<text x="{lx}" y="{y + 4.5}" font-size="12.5">'
                     f'<tspan style="fill:var(--arb-c-{conf_cls})">{_e(conf_label)}</tspan>'
                     + (f'<tspan class="t-faint"> · {_e(status)}</tspan>' if status else "")
                     + f"<title>{_e(i['name'])} · 置信度 {_e(conf_label)} · 创建于 "
                     f"{_e(fmt_time(i['created_at']))}</title></text>")
        parts.append("</g>")
    parts.append("</svg>")
    return "".join(parts)


def tree(board: Board, show_dropped: bool = True, selected: str | None = None,
         controls: str = "") -> str:
    dropped = [i["name"] for i in board.ideas if i["status"] == "dropped"]
    hidden = ("" if show_dropped or not dropped else
              f'<div class="note-line">已隐藏 {len(dropped)} 个已放弃的 idea：'
              f"{_e('、'.join(dropped))}</div>")
    tip = ('<div class="note-line"><span style="color:var(--arb-bad)">▮</span> 失败　'
           '<span style="color:var(--arb-warn)">▮</span> 待批准</div>')
    body = tree_svg(board, show_dropped, selected)
    return _section("tree", _head("idea 树", controls=controls),
                    f'<div class="chart">{body}</div>', tip if board.ideas else "", hidden)


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


def _dl(facts: list[tuple[str, str]], cls: str = "facts") -> str:
    """`facts`: (label, html value); each pair in its own <div> (valid in a <dl>)."""
    return f'<dl class="{cls}">' + "".join(f"<div><dt>{_e(k)}</dt><dd>{v}</dd></div>"
                                           for k, v in facts) + "</dl>"


def runs_table(board: Board, idea: dict[str, Any]) -> str:
    """run · 状态 · 改动 · 资源 · SU · wandb; each row opens to the rest."""
    runs = board.runs_of(idea["id"])
    if not runs:
        return '<div class="muted">还没有 run</div>'
    head = ('<div class="rh"><span>run</span><span>状态</span><span>改动</span>'
            '<span>资源</span><span class="r">SU</span><span>wandb</span></div>')
    rows = []
    for r in reversed(runs):
        n_out, names = out_files(board, r["id"])
        wandb = (f'<a href="{_e(r["wandb_url"])}" target="_blank" rel="noopener">wandb</a>'
                 if r.get("wandb_url") else '<span class="faint">—</span>')
        changes = changes_vs_parent(board, r)
        if changes:
            change = _e("，".join(changes))
            tip = "，".join(changes) + " · " + (r["change_reason"] or "")
        else:
            change = f'<span class="muted">{_e(_clip(r["change_reason"], 40))}</span>'
            tip = r["change_reason"] or ""
        if r["actual_cost"] is not None:
            su = _e(fmt_su(r["actual_cost"], unit=False))
        elif r["est_cost"] is not None:
            su = f'<span class="faint" title="估计上限">≤ {_e(fmt_su(r["est_cost"], unit=False))}</span>'
        else:
            su = '<span class="faint">—</span>'
        detail = [("原因", _e(r["change_reason"])),
                  ("父 run", _e(r.get("parent_run") or "—")),
                  ("分区", _e(r.get("partition") or "—")),
                  ("估计上限", f'<span class="num">{_e(fmt_su(r["est_cost"]))}</span>'),
                  ("实际", f'<span class="num">{_e(fmt_su(r["actual_cost"]))}</span>'),
                  ("创建", _e(fmt_time(r["created_at"])))]
        if n_out:
            detail.append(("输出", f"{n_out} 个 · " + _e("、".join(names))))
        rows.append(
            f'<details><summary class="rr"><span class="id">{_e(r["id"])}</span>'
            f"<span>{pill(r['status'])}</span>"
            f'<span class="c" title="{_e(tip)}">{change}</span>'
            f'<span class="c">{_e(_resources(r))}</span><span class="r">{su}</span>'
            f"<span>{wandb}</span></summary>"
            f'<div class="rd">{_dl(detail, "facts inline")}</div></details>')
    return f'<div class="runs">{head}{"".join(rows)}</div>'


def _note_item(note: dict[str, Any]) -> str:
    tags = []
    if note.get("run_id"):
        tags.append(note["run_id"])
    if note.get("confidence"):
        tags.append("置信度 → " + CONFIDENCE.get(note["confidence"], (note["confidence"],))[0])
    tag = f'<span class="faint"> · {_e(" · ".join(tags))}</span>' if tags else ""
    return (f'<div class="item"><div class="line"><span class="when">'
            f'{_e(fmt_time(note["created_at"]))}</span><span class="text" '
            f'style="white-space:normal">{_e(note["text"])}{tag}</span></div></div>')


def _summary_item(run: dict[str, Any]) -> str:
    return (f'<div class="item"><details><summary><b>{_e(run["id"])}</b> {pill(run["status"])} '
            f'<span class="muted">{_e(_clip(run["summary"], 70))}</span></summary>'
            f"<pre>{_e(run['summary'])}</pre></details></div>")


def _sub(title: str, count: int | None, body: str) -> str:
    c = f' <span class="count">{count}</span>' if count is not None else ""
    return f'<div class="sub"><h3>{_e(title)}{c}</h3>{body}</div>'


def idea_detail(board: Board, name: str | None, controls: str = "") -> str:
    idea = board.idea(name) if name else None
    if idea is None:
        return _section("detail", _head("idea 详情", controls=controls),
                        '<div class="muted">没有选中的 idea</div>')
    nb = notebook_path(board, idea["name"])
    if nb.exists():
        notebook = (f'<a href="{_e(vscode_link(nb))}" title="{_e(nb)}">{_e(nb.name)}</a> '
                    f"{cmd('marimo edit ' + shlex.quote(str(nb)))}")
    else:
        notebook = (f'<span class="muted">没有 notebook</span> '
                    f"{cmd('arb idea notebook ' + idea['name'])}")
    status = IDEA_STATUS.get(idea["status"], idea["status"])
    facts = [("假设", f'<span class="hyp">{_e(idea["hypothesis"])}</span>'),
             ("指标", f"<code>{_e(idea['metric'])}</code>"),
             ("置信度", confidence(idea["confidence"])
              + (f' <span class="faint">· {_e(status)}</span>' if idea["status"] != "open"
                 else "")),
             ("notebook", notebook)]
    parent = idea.get("parent_name")
    origin = (f"{_e(parent)}" + (f"（从 {_e(idea['from_run'])} 的快照分出）"
                                 if idea.get("from_run") else "")) if parent else "—"
    more = [("父 idea", origin),
            ("分支", f"<code>{_e(idea['branch'])}</code>"),
            ("worktree", f"<code>{_e(idea['worktree_path'])}</code>"
             + ("" if idea["status"] == "open" else ' <span class="muted">（已删除）</span>')),
            ("创建", _e(fmt_time(idea["created_at"], with_year=True)))]
    if idea.get("closed_at"):
        more.append(("关闭", _e(fmt_time(idea["closed_at"], with_year=True))))
    more.append(("已用", f'<span class="num">{_e(fmt_su(board.su_used(idea["id"])))}</span>'))
    runs = board.runs_of(idea["id"])
    notes = [_note_item(n) for n in board.human_notes(idea["id"])]
    summaries = [_summary_item(r) for r in reversed(runs) if r.get("summary")]
    todos = [_todo_item(board, t, with_idea=False) for t in board.todos
             if t["idea_id"] == idea["id"]]
    parts = [
        _head(f"idea 详情 · {idea['name']}", controls=controls),
        _dl(facts),
        f'<details class="more"><summary>更多</summary>{_dl(more, "facts small")}</details>',
        _sub("runs", len(runs), runs_table(board, idea)),
        _sub("笔记", len(notes), _newest(notes) or '<div class="faint">还没有笔记</div>'),
    ]
    if summaries:
        parts.append(_sub("run 总结", len(summaries), _newest(summaries)))
    if todos:
        parts.append(_sub("待办", len(todos), "".join(todos)))
    return _section("detail", *parts)


# ---------------------------------------------------------------- 算力与预算


def su_bars(board: Board) -> str:
    rows = [(i, board.su_used(i["id"])) for i in board.ideas]
    rows = sorted([(i, su) for i, su in rows if su > 0], key=lambda p: -p[1])
    if not rows:
        return '<div class="muted">还没有花费</div>'
    top = max(su for _, su in rows)
    width, label_w, value_w, row_h = 520, 88, 84, 32
    bar_w = width - label_w - value_w
    height = row_h * len(rows)
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
             f'class="bars" role="img" aria-label="各 idea 已用 SU">']
    for k, (i, su) in enumerate(rows):
        y = k * row_h + row_h / 2
        w = max(4.0, bar_w * su / top)
        dim = ' opacity=".5"' if i["status"] == "dropped" else ""
        parts.append(f"<g{dim}>")
        parts.append(f'<text x="0" y="{y + 4.5}" font-size="13" class="t-ink">'
                     f'{_e(_clip(i["name"], 12))}</text>')
        parts.append(f'<rect x="{label_w}" y="{y - 5}" width="{w:.1f}" height="10" rx="5" '
                     f'style="fill:var(--arb-accent)"><title>{_e(i["name"])}: '
                     f"{_e(fmt_su(su))}</title></rect>")
        parts.append(f'<text x="{label_w + w + 8:.1f}" y="{y + 4.5}" font-size="12" '
                     f'class="t-num">{_e(fmt_su(su))}</text>')
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
    ymax = max(peak, total if show_budget else 0) * 1.12 or 1
    width, height, left, right, top, bottom = 640, 180, 44, 16, 14, 26
    span = (t1 - t0).total_seconds()

    def x(t: datetime) -> float:
        return round(left + (width - left - right) * (t - t0).total_seconds() / span, 1)

    def y(v: float) -> float:
        return round(top + (height - top - bottom) * (1 - v / ymax), 1)

    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" '
             f'class="line" role="img" aria-label="累计 SU">']
    for v in nice_ticks(ymax):
        parts.append(f'<line x1="{left}" x2="{width - right}" y1="{y(v)}" y2="{y(v)}" '
                     'style="stroke:var(--arb-grid)"/>')
        parts.append(f'<text x="{left - 8}" y="{y(v) + 4}" font-size="12" text-anchor="end" '
                     f'class="t-faint t-num">{v:g}</text>')
    for t, label in time_ticks(t0, t1, target=6):
        tx = x(t.astimezone(timezone.utc))
        parts.append(f'<text x="{tx}" y="{height - 6}" font-size="12" text-anchor="middle" '
                     f'class="t-faint">{_e(label)}</text>')
    line = [f"M{x(t0)},{y(0)}"]
    for t, v, _ in points:
        line.append(f"H{x(t)}")
        line.append(f"V{y(v)}")
    line.append(f"H{x(t1)}")
    area = " ".join(line) + f" V{y(0)} H{x(t0)} Z"
    parts.append(f'<path d="{area}" style="fill:var(--arb-accent)" fill-opacity=".1"/>')
    parts.append(f'<path d="{" ".join(line)}" fill="none" style="stroke:var(--arb-accent)" '
                 'stroke-width="2" stroke-linejoin="round"/>')
    for t, v, run_id in points:
        parts.append(f'<circle cx="{x(t)}" cy="{y(v)}" r="6" style="fill:var(--arb-accent)" '
                     f'fill-opacity="0"><title>{_e(fmt_time(t))} · {_e(run_id)} · '
                     f"累计 {_e(fmt_su(v))}</title></circle>")
    if show_budget:
        parts.append(f'<line x1="{left}" x2="{width - right}" y1="{y(total)}" y2="{y(total)}" '
                     'style="stroke:var(--arb-muted)" stroke-dasharray="4 4" '
                     'stroke-width="1.25"/>')
        parts.append(f'<text x="{left + 6}" y="{y(total) - 6}" font-size="12" '
                     f'class="t-num">预算 {total:g} SU</text>')
    ex, ey = x(t1), y(peak)
    parts.append(f'<circle cx="{ex}" cy="{ey}" r="4" style="fill:var(--arb-accent);'
                 'stroke:var(--arb-surface)" stroke-width="2"/>')
    parts.append(f'<text x="{ex - 8}" y="{ey - 9}" font-size="12" text-anchor="end" '
                 f'font-weight="600" class="t-ink t-num">{_e(fmt_su(peak))}</text>')
    parts.append("</svg>")
    return "".join(parts)


def nice_ticks(vmax: float, n: int = 4) -> list[float]:
    if vmax <= 0:
        return [0]
    raw = vmax / n
    mag = 10 ** math.floor(math.log10(raw))
    step = next(m * mag for m in (1, 2, 2.5, 5, 10) if m * mag >= raw)
    return [round(k * step, 6) for k in range(int(vmax // step) + 1)]


def _job_item(board: Board, job: dict[str, Any]) -> str:
    run = job["run"]
    state = JOB_STATE.get(job["state"], job["state"])
    text = _sep(_e((board.idea_by_id(run["idea_id"]) or {}).get("name", "?")),
                f"s{job['segment']}", "作业 " + _e(job["job_id"] or "—"), _e(state),
                _e(human_duration(job["elapsed_s"]) if job["elapsed_s"] else "—"),
                f'<span class="num">{_e(fmt_su(job["billed_su"]))}</span>')
    when = f'<span class="when">{_e(age(job["updated_at"], board.loaded_at))}</span>'
    return ('<div class="item">' + _line(f'<span class="id">{_e(run["id"])}</span>', text, when)
            + "</div>")


def budget(board: Board) -> str:
    used = board.su_used()
    total = board.total_su
    if total > 0:
        pct = used / total * 100
        over = "over" if used > total else ""
        head = (f'<div class="budget-kpi"><span class="big">已用 {_e(fmt_su(used, unit=False))}'
                f"<small> / {total:,.0f} SU</small></span>"
                f'<div class="meter" title="{pct:.1f}%"><span class="{over}" '
                f'style="width:{min(pct, 100):.1f}%"></span></div>'
                f'<span class="muted">剩余 <b class="num">{_e(fmt_su(total - used))}</b></span>'
                "</div>")
    else:
        head = (f'<div class="budget-kpi"><span class="big">已用 {_e(fmt_su(used, unit=False))}'
                '<small> SU</small></span><span class="muted">未设项目预算 · '
                "<code>[budget] total_su</code></span></div>")
    charts = (f'<div class="charts2"><div><h3>各 idea 已用</h3>'
              f'<div class="chart">{su_bars(board)}</div></div>'
              f'<div><h3>累计花费</h3><div class="chart">{su_line(board)}</div></div></div>')
    jobs = board.active_jobs()
    jobs_html = (f'<div class="sub jobs"><h3>活跃作业 <span class="count">{len(jobs)}</span></h3>'
                 + "".join(_job_item(board, j) for j in jobs) + "</div>") if jobs else ""
    done = [r for r in board.runs if r["status"] in RUN_FINAL
            and r["est_cost"] is not None and r["actual_cost"] is not None]
    est_rows = [[f"<b>{_e(r['id'])}</b>",
                 _e((board.idea_by_id(r["idea_id"]) or {}).get("name", "?")),
                 pill(r["status"]), _e(_resources(r)),
                 _e(fmt_su(r["est_cost"], unit=False)), _e(fmt_su(r["actual_cost"], unit=False)),
                 _e(f"{r['actual_cost'] / r['est_cost'] * 100:.0f}%" if r["est_cost"] else "—")]
                for r in reversed(done)]
    est_html = (f'<details class="more sub"><summary>估计 vs 实际 · {len(done)} 个 run</summary>'
                + _table([("run", False), ("idea", False), ("状态", False), ("资源", False),
                          ("估计上限", True), ("实际", True), ("实际/估计", True)], est_rows)
                + "</details>") if done else ""
    return _section("budget", _head("算力与预算"), head, charts, jobs_html, est_html)


# ---------------------------------------------------------------- 种子园 (arb ui --home)

SEED_STATUS = {"thinking": ("思考中", "run"), "planted": ("已种下", "ok"),
               "dropped": ("已放弃", "slate")}
SHELL_SAFE = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
                       "/._-~")


def short_path(path: Path) -> str:
    """`~/arboria/seeds/x.py` under the home directory, else the full path."""
    try:
        return "~/" + str(Path(path).relative_to(Path.home()))
    except ValueError:
        return str(path)


def edit_command(path: Path) -> str:
    """`marimo edit <path>`, with `~` when the path needs no quoting."""
    short = short_path(path)
    if set(short) <= SHELL_SAFE:
        return f"marimo edit {short}"
    return f"marimo edit {shlex.quote(str(path))}"


def seed_pill(status: str) -> str:
    label, color = SEED_STATUS.get(status, (status, "slate"))
    return f'<span class="pill s-{color}">{_e(label)}</span>'


def garden_header(g: Garden, controls: str = "") -> str:
    def kpi(label: str, value: int) -> str:
        return (f'<div class="kpi"><div class="k">{_e(label)}</div>'
                f'<div class="v">{value}</div></div>')

    kpis = "".join(kpi(SEED_STATUS[k][0], g.count(k)) for k in ("thinking", "planted", "dropped"))
    updated = _e(UPDATED.format(time=local(g.loaded_at).strftime("%H:%M:%S")))
    ctl = f'<div class="controls">{controls}</div>' if controls else ""
    return (f'<section class="arb head"><div class="titlebar"><div>'
            f'<h1>Arboria 种子园<span class="proj">{_e(short_path(g.root))}</span></h1>'
            f'<div class="meta">{updated}</div></div>{ctl}</div>'
            f'<div class="kpis three">{kpis}</div></section>')


def _planted(seed: dict[str, Any]) -> str:
    return "、".join(f"{_e(Path(p['repo']).name)} · {_e(p['idea'])}" for p in seed["planted_to"])


def seed_card(g: Garden, seed: dict[str, Any]) -> str:
    nb = seed["notebook"]
    when = f'<span class="when" title="{_e(fmt_time(seed["last_edited"], True))}">' \
           f'{_e(age(seed["last_edited"], g.loaded_at))}</span>'
    parts = [f'<div class="seed-top"><span class="seed-name">{_e(seed["name"])}</span>'
             f'{seed_pill(seed["status"])}{when}</div>']
    if seed.get("gist"):
        parts.append(f'<div class="gist">{_e(seed["gist"])}</div>')
    if seed.get("hypothesis"):
        parts.append(f'<div class="seed-hyp"><span class="faint">假设</span> '
                     f'{_e(seed["hypothesis"])}</div>')
    if seed["planted_to"]:
        parts.append(f'<div class="seed-planted"><span class="faint">种在</span> '
                     f"{_planted(seed)}</div>")
    if seed.get("notebook_exists", True):
        foot = (f"{cmd(edit_command(nb), alt=True)}"
                f'<a class="vscode" href="{_e(vscode_link(nb))}" title="{_e(nb)}">VS Code</a>')
    else:
        foot = '<span class="faint">notebook 不在了</span>'
    parts.append(f'<div class="seed-foot">{foot}</div>')
    return f'<div class="seed s-{_e(seed["status"])}">' + "".join(parts) + "</div>"


def seeds_section(g: Garden) -> str:
    live = [s for s in g.seeds if s["status"] != "dropped"]
    dropped = g.with_status("dropped")
    if live:
        body = '<div class="seeds">' + "".join(seed_card(g, s) for s in live) + "</div>"
    else:
        body = ('<div class="muted">还没有种子 · '
                f'{cmd("arb idea new NAME " + chr(34) + "一句话想法" + chr(34))}</div>')
    more = ""
    if dropped:
        rows = "".join(f'<div class="item">' + _line(
            f'<span class="id">{_e(s["name"])}</span>', _e(s.get("gist") or ""),
            f'<span class="when">{_e(age(s["last_edited"], g.loaded_at))}</span>'
            + cmd(f"arb idea revive {s['name']}", alt=True)) + "</div>"
            for s in dropped)
        more = (f'<details class="more"><summary>已放弃 {len(dropped)} 个</summary>'
                f"{rows}</details>")
    return _section("garden", _head("种子", len(live)), body, more)


def garden_todos(g: Garden) -> str:
    items = []
    for t in g.todos:
        kind = todo_kind(t)
        tag = f'<span class="tag">{_e(kind)}</span> ' if kind else ""
        text = _sep(_e(t["seed"]), tag + _e(_clip(t["text"], 120)))
        when = f'<span class="when">{_e(age(t["created_at"], g.loaded_at))}</span>'
        items.append('<div class="item wrap">' + _line(
            f'<span class="id">{_e(t["id"])}</span>', text, when, tip=t["text"]) + "</div>")
    body = "".join(items) or '<div class="faint">没有</div>'
    return _section("garden-todos", _head("等 Claude 的待办", len(items)), body)


def garden_body(g: Garden) -> str:
    return (f'<div class="arb-grid garden-grid"><div class="arb-col">{seeds_section(g)}</div>'
            f'<div class="arb-col">{garden_todos(g)}</div></div>')


def garden_page(g: Garden) -> str:
    """The seed home as one standalone HTML fragment (same layout as the app)."""
    return wrap(garden_header(g), garden_body(g))
