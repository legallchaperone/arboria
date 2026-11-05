"""The built-in analyses behind `lab.plot`, `lab.diff` and `lab.spend` (see lab.py).

Everything here reads: the ledger, the repo's objects (`git diff --stat` between two
snapshot commits never touches a branch, the index or a worktree) and data `lab` hands
in (wandb history, fetched by `lab` in the human's kernel). Nothing writes, nothing
talks to the network. The results render calmly: one HTML view for marimo / Jupyter
(ledger text escaped), and plain text everywhere else.
"""

from __future__ import annotations

import html
import json
import math
import unicodedata
from dataclasses import dataclass, field
from typing import Any, Iterable

from . import gitutil, slurm
from .cards import request_of
from .errors import ArbError

MUTED = "opacity:.62"
CODE = "padding:1px 6px;border-radius:4px;background:rgba(127,127,127,.12)"
DASH = "—"


def esc(text: Any) -> str:
    return html.escape("" if text is None else str(text), quote=True)


def clip(text: str, n: int) -> str:
    text = " ".join(str(text).split())
    return text if len(text) <= n else text[: n - 1] + "…"


def width(text: str) -> int:
    """Terminal width: CJK characters take two columns."""
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in text)


def pad(text: str, n: int) -> str:
    return text + " " * max(0, n - width(text))


# ---------------------------------------------------------------- a calm table


@dataclass
class View:
    """One title line, a borderless table and a few muted lines under it.

    `styles` gives each column a look: "" plain, "muted", "code" or "num" (right-aligned).
    A muted first column ("group") is only shown where it changes."""

    title: str | None
    rows: list[tuple[str, ...]]
    styles: tuple[str, ...] = ()
    head: tuple[str, ...] | None = None
    foot: list[str] = field(default_factory=list)
    pre: str | None = None
    group: bool = False  # the first column labels groups of rows
    titles: list[tuple[str, ...]] | None = None  # per-cell tooltips (full text)
    css_class: str = "arb-view"

    def _style(self, i: int) -> str:
        return self.styles[i] if i < len(self.styles) else ""

    def output(self) -> list[dict[str, Any]]:
        """The app notebook's structured form (arboria.output): a table, then text."""
        out: list[dict[str, Any]] = []
        if self.rows:
            width = max(len(r) for r in self.rows)
            head = list(self.head) if self.head else [""] * width
            out.append({"kind": "table", "columns": head, "rows": [list(r) for r in self.rows],
                        "total": len(self.rows), **({"title": self.title} if self.title else {})})
        elif self.title:
            out.append({"kind": "text", "text": self.title})
        if self.pre:
            out.append({"kind": "text", "text": self.pre})
        if self.foot:
            out.append({"kind": "text", "text": "\n".join(self.foot)})
        return out

    def html(self) -> str:
        out = []
        if self.title:
            out.append(f"<div>{esc(self.title)}</div>")
        if self.rows:
            cells = []
            if self.head:
                cells.append("<tr>" + "".join(
                    f'<th style="text-align:{"right" if self._style(i) == "num" else "left"};'
                    f'padding:2px 16px 2px 0;font-weight:500;{MUTED}">{esc(h)}</th>'
                    for i, h in enumerate(self.head)) + "</tr>")
            last = None
            for r, row in enumerate(self.rows):
                tds = []
                for i, cell in enumerate(row):
                    text = cell
                    if i == 0 and self.group:
                        text = "" if cell == last else cell
                    style, inner = self._style(i), esc(text)
                    tip = self.titles[r][i] if self.titles and i < len(self.titles[r]) else ""
                    attr = f' title="{esc(tip)}"' if tip and tip != text else ""
                    if style == "code" and text:
                        inner = f'<code style="{CODE};font-size:.92em">{inner}</code>'
                    css = "padding:2px 16px 2px 0;vertical-align:baseline;border:0"
                    if i < len(row) - 1:
                        css += ";white-space:nowrap"
                    if style == "muted":
                        css += f";{MUTED}"
                    if style == "num":
                        css += ";text-align:right;font-variant-numeric:tabular-nums"
                    tds.append(f'<td style="{css}"{attr}>{inner}</td>')
                last = row[0] if row else None
                cells.append("<tr>" + "".join(tds) + "</tr>")
            out.append('<table style="border-collapse:collapse;margin:2px 0;font-size:.95em;'
                       'width:auto">' + "".join(cells) + "</table>")
        if self.pre:
            out.append('<pre style="font-size:12px;line-height:1.5;margin:4px 0 0;'
                       f'padding:8px 10px;border-radius:6px;background:rgba(127,127,127,.08);'
                       f'white-space:pre-wrap">{esc(self.pre)}</pre>')
        for line in self.foot:
            out.append(f'<div style="{MUTED};font-size:.9em">{esc(line)}</div>')
        return f'<div class="{self.css_class}" style="line-height:1.7">' + "".join(out) + "</div>"

    def text(self) -> str:
        lines = [self.title] if self.title else []
        rows = ([self.head] if self.head else []) + list(self.rows)
        if rows:
            n = max(len(r) for r in rows)
            widths = [max(width(r[i]) if i < len(r) else 0 for r in rows) for i in range(n)]
            last = None
            for k, row in enumerate(rows):
                cells = list(row)
                if self.group and cells and (k > 0 or not self.head):
                    shown = "" if cells[0] == last else cells[0]
                    last = cells[0]
                    cells[0] = shown
                parts = []
                for i, cell in enumerate(cells):
                    if self._style(i) == "num":
                        parts.append(" " * (widths[i] - width(cell)) + cell)
                    elif i == len(cells) - 1:
                        parts.append(cell)
                    else:
                        parts.append(pad(cell, widths[i]))
                lines.append("  ".join(parts).rstrip())
        if self.pre:
            lines += ["  " + line for line in self.pre.splitlines()]
        lines += self.foot
        return "\n".join(lines)


class Shown:
    """Base for what the built-ins return: `.view()` decides the look."""

    def view(self) -> View:  # pragma: no cover - abstract
        raise NotImplementedError

    def _repr_html_(self) -> str:
        return self.view().html()

    def _arboria_(self) -> list[dict[str, Any]]:
        return self.view().output()

    def __str__(self) -> str:
        return self.view().text()

    __repr__ = __str__


# ---------------------------------------------------------------- lab.plot

# The reference categorical palette, in its fixed order (validated: adjacent pairs pass
# the colour-blind checks on light and dark surfaces); a ninth run is refused, not
# given a generated colour.
SERIES_COLORS = ("#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300",
                 "#6250d6", "#e34948")
MAX_POINTS = 1500  # per series; longer histories are thinned evenly (last point kept)


@dataclass
class Series:
    run: str
    points: list[tuple[float, float]]  # (step, value), finite values only


def series_from_rows(run: str, key: str, rows: Iterable[dict[str, Any]]) -> Series:
    pts = []
    for i, row in enumerate(rows):
        value = row.get(key)
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            continue
        if not math.isfinite(value):
            continue
        step = row.get("_step", i)
        step = step if isinstance(step, (int, float)) and not isinstance(step, bool) else i
        pts.append((float(step), float(value)))
    pts.sort(key=lambda p: p[0])
    return Series(run, pts)


def nice_ticks(lo: float, hi: float, n: int = 5) -> list[float]:
    if hi <= lo:
        hi = lo + (abs(lo) or 1.0)
    raw = (hi - lo) / max(1, n)
    mag = 10 ** math.floor(math.log10(raw))
    step = next(m * mag for m in (1, 2, 2.5, 5, 10) if m * mag >= raw)
    start = math.ceil(lo / step - 1e-9) * step
    ticks, t = [], start
    while t <= hi + step * 1e-9:
        ticks.append(round(t, 12))
        t += step
    return ticks


def fmt_num(v: float) -> str:
    if v == 0:
        return "0"
    a = abs(v)
    if a >= 1e6:
        return f"{v / 1e6:g}M"
    if a >= 1e4:
        return f"{v / 1e3:g}k"
    if a < 1e-3:
        return f"{v:.1e}"
    return f"{v:.4g}"


def fmt_step(v: float, top: float) -> str:
    """Axis labels for steps: all in k once the axis reaches 10k (2.5k, 5k, 7.5k, 10k)."""
    return f"{v / 1e3:g}k" if top >= 1e4 and abs(v) >= 1e3 else fmt_num(v)


def _thin(points: list[tuple[float, float]]) -> list[tuple[float, float]]:
    if len(points) <= MAX_POINTS:
        return points
    k = math.ceil(len(points) / MAX_POINTS)
    return points[::k] + ([points[-1]] if (len(points) - 1) % k else [])


@dataclass
class Plot(Shown):
    """What `lab.plot` returns: an inline SVG line chart, one line per run."""

    key: str
    series: list[Series]
    skipped: list[tuple[str, str]] = field(default_factory=list)  # (run, why)
    log: bool = False

    def _arboria_(self) -> dict[str, Any]:
        """A line chart the app draws itself (uPlot), every point kept."""
        note = "；".join(f"{run}：{why}" for run, why in self.skipped) or None
        return {"kind": "plot", "y": self.key, "x": "step", "log": self.log, "style": "line",
                "series": [{"name": s.run, "x": [p[0] for p in s.points],
                            "y": [p[1] for p in s.points]} for s in self.series],
                **({"note": note} if note else {})}

    @property
    def data(self) -> list[dict[str, Any]]:
        """Long format: run, _step, <key>."""
        return [{"run": s.run, "_step": x, self.key: y} for s in self.series for x, y in s.points]

    def _summary(self) -> list[str]:
        out = []
        for s in self.series:
            if s.points:
                x, y = s.points[-1]
                out.append(f"{s.run}：{len(s.points)} 个点，最后 {fmt_num(y)}（step {fmt_num(x)}）")
            else:
                out.append(f"{s.run}：没有 {self.key} 的数据")
        return out + [f"{run}：{why}" for run, why in self.skipped]

    def __str__(self) -> str:
        return "\n".join([self.key + ("（对数）" if self.log else ""), *self._summary()])

    __repr__ = __str__

    def svg(self, w: int = 640, h: int = 240) -> str:
        left, right, top, bottom = 52, 14, 10, 26
        pts = [p for s in self.series for p in s.points if not self.log or p[1] > 0]
        if not pts:
            return ""
        xs, ys = [p[0] for p in pts], [p[1] for p in pts]
        f = (lambda v: math.log10(v)) if self.log else (lambda v: v)
        x0, x1 = min(xs), max(xs)
        y0, y1 = f(min(ys)), f(max(ys))
        if x1 == x0:
            x1 = x0 + 1
        if self.log:
            yt = [float(t) for t in range(math.floor(y0), math.ceil(y1) + 1)]
            if len(yt) < 2:
                yt = nice_ticks(y0, y1, 4)
        else:
            margin = (y1 - y0) * 0.05 or abs(y0) * 0.05 or 1.0
            yt = nice_ticks(y0 - margin, y1 + margin, 4)
        lo, hi = min(yt[0], y0), max(yt[-1], y1)
        if hi == lo:
            hi = lo + 1
        xt = [t for t in nice_ticks(x0, x1, 5) if x0 <= t <= x1]

        def px(x: float) -> float:
            return left + (x - x0) / (x1 - x0) * (w - left - right)

        def py(y: float) -> float:
            return top + (1 - (y - lo) / (hi - lo)) * (h - top - bottom)

        parts = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="100%" '
                 f'style="max-width:{w}px;display:block;font-size:11px" role="img" '
                 f'aria-label="{esc(self.key)}">']
        for t in yt:
            if lo <= t <= hi:
                label = fmt_num(10 ** t) if self.log else fmt_num(t)
                parts.append(f'<line x1="{left}" x2="{w - right}" y1="{py(t):.1f}" '
                             f'y2="{py(t):.1f}" stroke="currentColor" stroke-opacity=".1"/>'
                             f'<text x="{left - 6}" y="{py(t) + 3.5:.1f}" text-anchor="end" '
                             f'fill="currentColor" fill-opacity=".55">{esc(label)}</text>')
        for t in xt:
            parts.append(f'<text x="{px(t):.1f}" y="{h - 8}" text-anchor="middle" '
                         f'fill="currentColor" fill-opacity=".55">{esc(fmt_step(t, xt[-1]))}</text>')
        parts.append(f'<line x1="{left}" x2="{w - right}" y1="{h - bottom}" y2="{h - bottom}" '
                     'stroke="currentColor" stroke-opacity=".25"/>')
        for i, s in enumerate(self.series):
            sp = [(x, y) for x, y in _thin(s.points) if not self.log or y > 0]
            if not sp:
                continue
            color = SERIES_COLORS[i]
            d = " ".join(f"{'M' if k == 0 else 'L'}{px(x):.1f},{py(f(y)):.1f}"
                         for k, (x, y) in enumerate(sp))
            lx, ly = sp[-1]
            tip = f"{s.run} · 最后 {fmt_num(ly)}（step {fmt_num(lx)}）"
            parts.append(f'<g><title>{esc(tip)}</title>'
                         f'<path d="{d}" fill="none" stroke="{color}" stroke-width="2" '
                         'stroke-linejoin="round" stroke-linecap="round"/>'
                         f'<circle cx="{px(lx):.1f}" cy="{py(f(ly)):.1f}" r="3" fill="{color}"/>'
                         '</g>')
        parts.append("</svg>")
        return "".join(parts)

    def _repr_html_(self) -> str:
        legend = "".join(
            f'<span style="margin-right:14px;white-space:nowrap">'
            f'<span style="display:inline-block;width:14px;height:2px;margin:0 6px 3px 0;'
            f'vertical-align:middle;background:{SERIES_COLORS[i]}"></span>{esc(s.run)}</span>'
            for i, s in enumerate(self.series))
        title = esc(self.key) + (f' <span style="{MUTED}">对数</span>' if self.log else "")
        out = [f'<div style="display:flex;flex-wrap:wrap;gap:4px 18px;align-items:baseline">'
               f'<strong style="font-weight:500">{title}</strong><span>{legend}</span></div>']
        chart = self.svg()
        out.append(chart or f'<div style="{MUTED}">没有可画的点</div>')
        for run, why in self.skipped:
            out.append(f'<div style="{MUTED};font-size:.9em">{esc(run)}：{esc(why)}</div>')
        return '<div class="arb-plot" style="line-height:1.7">' + "".join(out) + "</div>"


def make_plot(key: str, histories: list[tuple[str, list[dict[str, Any]] | ArbError]],
              *, log: bool = False) -> Plot:
    """`histories`: (run id, wandb rows or the error that kept them away), in order."""
    if len([h for h in histories if not isinstance(h[1], ArbError)]) > len(SERIES_COLORS):
        raise ArbError(f"一张图最多 {len(SERIES_COLORS)} 个 run；分成几张画")
    series, skipped = [], []
    for run, rows in histories:
        if isinstance(rows, ArbError):
            skipped.append((run, str(rows)))
        else:
            series.append(series_from_rows(run, key, rows))
    if not series and skipped:
        raise ArbError(skipped[0][1])
    return Plot(key, series, skipped, log)


# ---------------------------------------------------------------- lab.diff

DIFF_STAT_LINES = 12
ASK_FIELDS = (("machine", "机器"), ("partition", "分区"), ("gpus", "GPU"), ("nodes", "节点"),
              ("walltime_s", "时长"), ("mem_gb", "内存 G"), ("cpus", "CPU"))


def _req(run: dict[str, Any]) -> dict[str, Any]:
    try:
        return request_of(run)
    except (ArbError, ValueError):
        return {}


def _val(v: Any) -> str:
    if v is None:
        return DASH
    if isinstance(v, str):
        return v
    return json.dumps(v, ensure_ascii=False, separators=(", ", ": "))


def flatten(value: Any, prefix: str = "") -> dict[str, Any]:
    """Nested dicts as dotted keys (lists stay whole)."""
    if isinstance(value, dict) and value:
        out: dict[str, Any] = {}
        for k, v in value.items():
            out.update(flatten(v, f"{prefix}.{k}" if prefix else str(k)))
        return out
    return {prefix or "（整个）": value}


@dataclass
class RunDiff(Shown):
    """What `lab.diff` returns: only what differs between two runs."""

    a: str
    b: str
    rows: list[tuple[str, str, str, str]]  # (group, what, a, b)
    stat: str | None = None
    same_tree: bool = False

    @property
    def data(self) -> list[dict[str, str]]:
        return [{"项": g, "名": w, self.a: x, self.b: y} for g, w, x, y in self.rows]

    def __bool__(self) -> bool:
        return bool(self.rows)

    def view(self) -> View:
        if not self.rows:
            return View(f"{self.a} 和 {self.b} 没有不同（命令、资源、代码、config）", [],
                        css_class="arb-diff")
        rows = [(g, w, clip(x, 60), clip(y, 60)) for g, w, x, y in self.rows]
        foot = ["代码不同，但文件树一样（只是提交不同）"] if self.same_tree else []
        return View(f"{self.a} → {self.b} · {len(rows)} 处不同", rows,
                    styles=("muted", "", "", ""), head=("", "", self.a, self.b), group=True,
                    titles=[("", "", x, y) for _, _, x, y in self.rows], pre=self.stat,
                    foot=foot, css_class="arb-diff")


def run_diff(main: Any, a: dict[str, Any], b: dict[str, Any]) -> RunDiff:
    rows: list[tuple[str, str, str, str]] = []
    ra, rb = _req(a), _req(b)
    ba, bb = ra.get("base_command"), rb.get("base_command")
    if ba != bb and (ba or bb):
        rows.append(("命令", "", " ".join(ba or []) or DASH, " ".join(bb or []) or DASH))
    oa = {k: v for k, v in ra.get("overrides", [])}
    ob = {k: v for k, v in rb.get("overrides", [])}
    for k in [*oa, *(k for k in ob if k not in oa)]:
        if oa.get(k) != ob.get(k):
            rows.append(("--set", k, _val(oa.get(k)), _val(ob.get(k))))

    aa, ab = ra.get("ask") or {}, rb.get("ask") or {}
    for key, label in ASK_FIELDS:
        va, vb = aa.get(key, a.get(key)), ab.get(key, b.get(key))
        if va != vb:
            if key == "walltime_s":
                va = slurm.human_duration(va) if va else None
                vb = slurm.human_duration(vb) if vb else None
            rows.append(("资源", label, _val(va), _val(vb)))

    stat, same_tree = None, False
    if a["snapshot_commit"] != b["snapshot_commit"]:
        same_tree = a["tree_hash"] == b["tree_hash"]
        rows.append(("代码", "快照", a["snapshot_commit"][:10], b["snapshot_commit"][:10]))
        if not same_tree:
            try:
                text = gitutil.out(["diff", "--stat=80", "--no-color", a["snapshot_commit"],
                                    b["snapshot_commit"]], main)
            except ArbError as exc:
                text = f"git diff 失败：{exc}"
            lines = text.splitlines()
            if len(lines) > DIFF_STAT_LINES:
                lines = lines[: DIFF_STAT_LINES - 1] + [
                    f"…（另有 {len(lines) - DIFF_STAT_LINES} 个文件）", lines[-1]]
            stat = "\n".join(lines) or None

    ca = json.loads(a["config_json"]) if a.get("config_json") else None
    cb = json.loads(b["config_json"]) if b.get("config_json") else None
    if (ca is None) != (cb is None):
        missing = "还没拉回"
        rows.append(("config", "", missing if ca is None else f"{len(flatten(ca))} 项",
                     missing if cb is None else f"{len(flatten(cb))} 项"))
    elif ca is not None:
        fa, fb = flatten(ca), flatten(cb)
        for k in [*fa, *(k for k in fb if k not in fa)]:
            if fa.get(k, ...) != fb.get(k, ...):
                rows.append(("config", k, _val(fa.get(k)), _val(fb.get(k))))
    return RunDiff(a["id"], b["id"], rows, stat, same_tree)


# ---------------------------------------------------------------- lab.spend


def _gpu_h(row: dict[str, Any]) -> float:
    return (row["gpu_s"] or 0) / 3600


def _n(v: float) -> str:
    return f"{v:.2f}" if v else "0"


@dataclass
class Spend(Shown):
    """What `lab.spend` returns: SU and GPU hours per idea, or per run of one idea."""

    rows: list[dict[str, Any]]
    idea: str | None = None  # per-run view of this idea
    total_su: float = 0.0
    budget: float = 0.0  # 0 = none configured
    pending_su: float = 0.0  # upper bound of runs still awaiting approval
    others: int = 0  # ideas (or runs) with nothing spent, left out

    @property
    def data(self) -> list[dict[str, Any]]:
        return self.rows

    def view(self) -> View:
        if self.idea:
            head = ("run", "状态", "GPU·h", "SU")
            rows = [(r["run"], r["状态"], _n(r["gpu_h"]), r["su_text"]) for r in self.rows]
            title = f"{self.idea} · {self.total_su:.2f} SU"
        else:
            head = ("idea", "run", "GPU·h", "SU")
            rows = [(r["idea"], str(r["runs"]), _n(r["gpu_h"]), _n(r["su"])) for r in self.rows]
            title = (f"已用 {self.total_su:.2f} / {self.budget:g} SU，剩 "
                     f"{self.budget - self.total_su:.2f}" if self.budget
                     else f"已用 {self.total_su:.2f} SU（arboria.toml 没设 [budget] total_su）")
        foot = []
        if self.pending_su:
            foot.append(f"待批准的 run 最多还要 {self.pending_su:.2f} SU")
        if self.others:
            foot.append(f"另有 {self.others} 个 {'run' if self.idea else 'idea'} 还没花过算力")
        if not rows:
            return View(title, [], foot=foot or ["还没有上过集群的 run"], css_class="arb-spend")
        return View(title, rows, styles=("code" if self.idea else "", "muted" if self.idea
                                         else "num", "num", "num"),
                    head=head, foot=foot, css_class="arb-spend")


SPEND_SQL = """
SELECT r.id AS run, r.idea_id, r.status, r.gpus, r.est_cost, r.actual_cost,
       (SELECT SUM(j.elapsed_s) FROM jobs j WHERE j.run_id = r.id) AS elapsed_s,
       (SELECT SUM(j.billed_su) FROM jobs j WHERE j.run_id = r.id) AS su,
       (SELECT COUNT(*) FROM jobs j WHERE j.run_id = r.id) AS jobs
FROM runs r {where} ORDER BY r.id
"""


def spend(ledger: Any, *, budget: float = 0.0, idea: dict[str, Any] | None = None,
          status_label: Any = str) -> Spend:
    where, params = ("WHERE r.idea_id = ?", (idea["id"],)) if idea else ("", ())
    runs = ledger.all(SPEND_SQL.format(where=where), params)
    for r in runs:
        r["gpu_s"] = (r["elapsed_s"] or 0) * (r["gpus"] or 0)
        r["su"] = r["su"] or 0.0
    pending = sum(r["est_cost"] or 0 for r in runs if r["status"] == "awaiting_approval")
    if idea is not None:
        used = [r for r in runs if r["jobs"]]
        rows = [{"run": r["run"], "状态": status_label(r["status"]), "gpu_h": _gpu_h(r),
                 "su": r["su"], "su_text": _n(r["su"])} for r in used]
        return Spend(rows, idea["name"], round(sum(r["su"] for r in used), 2),
                     budget, round(pending, 2), len(runs) - len(used))
    names = {i["id"]: i["name"] for i in ledger.all("SELECT id, name FROM ideas")}
    per: dict[int, dict[str, Any]] = {}
    for r in runs:
        if not r["jobs"]:
            continue
        row = per.setdefault(r["idea_id"], {"idea": names.get(r["idea_id"], "?"), "runs": 0,
                                            "gpu_h": 0.0, "su": 0.0})
        row["runs"] += 1
        row["gpu_h"] += _gpu_h(r)
        row["su"] += r["su"]
    rows = sorted(per.values(), key=lambda r: (-r["su"], r["idea"]))
    for row in rows:
        row["su"] = round(row["su"], 2)
    total = round(sum(r["su"] for r in rows), 2)
    return Spend(rows, None, total, budget, round(pending, 2), len(names) - len(rows))
