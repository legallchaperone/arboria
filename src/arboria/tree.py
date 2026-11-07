"""The research tree: project → ideas → runs, built deterministically from the ledger.

This is what Claude gets instead of a long status: `arb tree` (a subtree, a depth, a
character budget, or `--json`) on demand, and `index()` for the SessionStart hook, a
short view around the idea the session is in ("你在这里").

One line per node: its parent, what changed (`change_reason`), the result (the final value
of the idea's metric in the run's `metrics.jsonl`, fetched by `arb poll`) and the
conclusion with its source. The conclusion takes the first of CONCLUSION_SOURCES that is
there; `verdict` (the human's) and `ai_oneline` (the AI's) are not in the ledger yet, so
today it is the human's note on that run, else the run's AI-written summary.

Read only: the ledger, the config and the fetched metrics files. Ordering is by id, never
by time, so the text is stable from one session to the next (prompt cache).
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from typing import Any, Iterator

from . import analysis
from .config import project_config
from .db import Ledger
from .errors import ArbError
from .paths import RepoContext

INDEX_CHARS = 2400  # the hook's index (~1–1.5k tokens of mixed Chinese and code)
TREE_CHARS = 16_000  # `arb tree` default budget before finished subtrees fold
CHANGE_CLIP = 70
CONCLUSION_CLIP = 90
HYPOTHESIS_CLIP = 80
RECENT_RUNS = 5  # runs of the current idea in the index
FOLDED_RUNS = 3  # runs kept per open idea when the tree is over budget
TOP_RESULTS = 3

# Most trusted first. Only the last two exist in the ledger today (see the module doc).
CONCLUSION_SOURCES = ("verdict", "ai_oneline", "note", "summary")
SOURCE_LABEL = {"verdict": "人", "ai_oneline": "AI", "note": "人", "summary": "AI 摘要"}

_MIN_HINTS = ("↓", "lower", "minimi", "越低", "越小", "降低", "减少")
_MAX_HINTS = ("↑", "higher", "maximi", "越高", "越大", "提高", "提升")
_MAX_KEYS = ("acc", "f1", "auc", "bleu", "rouge", "reward", "score", "map", "iou", "recall",
             "precision", "win")
IDEA_STATUS_CN = {"open": "进行中", "merged": "已合并", "dropped": "已放弃"}


@dataclass
class RunNode:
    id: str
    idea: str
    parent: str | None  # parent_run, else the previous run of the same idea
    status: str
    change: str
    result: float | None = None
    delta: float | None = None  # result - parent's result, both on the same metric
    conclusion: str | None = None
    source: str | None = None  # one of CONCLUSION_SOURCES
    su: float | None = None


@dataclass
class IdeaNode:
    name: str
    status: str
    parent: str | None
    from_run: str | None
    hypothesis: str
    metric_text: str
    key: str | None  # the metrics.jsonl key its results are read from
    goal: str  # "min" or "max"
    runs: list[RunNode] = field(default_factory=list)
    children: list["IdeaNode"] = field(default_factory=list)
    lessons: list[str] = field(default_factory=list)

    @property
    def best(self) -> RunNode | None:
        scored = [r for r in self.runs if r.result is not None]
        if not scored:
            return None
        pick = min if self.goal == "min" else max
        return pick(scored, key=lambda r: r.result)

    def walk(self) -> Iterator["IdeaNode"]:
        yield self
        for c in self.children:
            yield from c.walk()


@dataclass
class Tree:
    project: str | None
    roots: list[IdeaNode]
    metric: str  # "[project] metric", "" when inferred per idea
    goal: str

    def ideas(self) -> Iterator[IdeaNode]:
        for r in self.roots:
            yield from r.walk()

    def find(self, name: str) -> IdeaNode | None:
        return next((i for i in self.ideas() if i.name == name), None)

    def path_to(self, name: str) -> list[IdeaNode]:
        def go(node: IdeaNode, acc: list[IdeaNode]) -> list[IdeaNode] | None:
            acc = [*acc, node]
            if node.name == name:
                return acc
            for c in node.children:
                found = go(c, acc)
                if found:
                    return found
            return None
        for r in self.roots:
            found = go(r, [])
            if found:
                return found
        return []


# -- building

def metric_key(text: str, keys: set[str]) -> str | None:
    """The metrics.jsonl key the idea's free-text metric names: the earliest mentioned,
    the longest on a tie ("val_loss" over "loss")."""
    low = text.lower()
    hits = [(low.find(k.lower()), -len(k), k) for k in keys if k and k.lower() in low]
    return min(hits)[2] if hits else None


def infer_goal(text: str, key: str | None) -> str:
    low = text.lower()
    if any(h in low for h in _MIN_HINTS):
        return "min"
    if any(h in low for h in _MAX_HINTS):
        return "max"
    k = (key or "").lower()
    if "loss" not in k and "err" not in k and any(m in k for m in _MAX_KEYS):
        return "max"
    return "min"


def _from_runs(ledger: Ledger) -> dict[str, str]:
    out: dict[str, str] = {}
    for e in ledger.all("SELECT subject, payload_json FROM events WHERE kind = 'idea_new' ORDER BY id"):
        try:
            fr = json.loads(e["payload_json"] or "{}").get("from_run")
        except ValueError:
            fr = None
        if isinstance(fr, str) and fr:
            out[e["subject"]] = fr
    return out


def _first_sentence(text: str) -> str:
    text = " ".join(text.split())
    m = re.search(r"[。！？!?]|\.(\s|$)", text)
    return text[: m.end()].strip() if m else text


def build(ctx: RepoContext, ledger: Ledger) -> Tree:
    try:
        cfg = project_config(ctx)
        project, metric, goal = cfg.project_id, cfg.metric, cfg.goal
    except ArbError:  # a broken arboria.toml must not hide the tree
        project, metric, goal = None, "", ""
    ideas = ledger.all("SELECT id, name, status, parent_id, hypothesis, metric FROM ideas ORDER BY id")
    runs = ledger.all("SELECT r.id, r.idea_id, r.parent_run, r.status, r.change_reason, r.summary,"
                      " r.actual_cost FROM runs r ORDER BY r.id")
    notes: dict[str, str] = {}
    for n in ledger.all("SELECT run_id, text FROM notes WHERE run_id IS NOT NULL"
                        " AND (author IS NULL OR author = 'human') ORDER BY id"):
        notes[n["run_id"]] = n["text"]  # the latest wins
    lessons: dict[str, list[str]] = {}
    for l in ledger.all("SELECT text, source_type, source_id FROM lessons"
                        " WHERE status = 'accepted' AND source_id IS NOT NULL ORDER BY id"):
        lessons.setdefault(l["source_id"], []).append(l["text"])
    from_run = _from_runs(ledger)
    state = ctx.state_dir

    by_idea: dict[int, list[dict[str, Any]]] = {}
    for r in runs:
        by_idea.setdefault(r["idea_id"], []).append(r)
    run_idea = {r["id"]: r["idea_id"] for r in runs}
    nodes: dict[int, IdeaNode] = {}
    results: dict[str, float | None] = {}
    for i in ideas:
        rows = by_idea.get(i["id"], [])
        series = {r["id"]: analysis.read_metrics(state, r["id"]) for r in rows}
        keys = {k for s in series.values() for k in s}
        key = metric or metric_key(i["metric"] or "", keys)
        node = IdeaNode(name=i["name"], status=i["status"], parent=None,
                        from_run=from_run.get(i["name"]), hypothesis=i["hypothesis"] or "",
                        metric_text=i["metric"] or "", key=key,
                        goal=goal or infer_goal(i["metric"] or "", key),
                        lessons=list(lessons.get(i["name"], [])))
        prev: str | None = None
        for r in rows:
            pts = series[r["id"]].get(key or "", [])
            result = pts[-1][1] if pts else None
            results[r["id"]] = result
            parent = r["parent_run"] or prev
            if r["id"] in notes:
                conclusion, source = notes[r["id"]], "note"
            elif r["summary"]:
                conclusion, source = _first_sentence(r["summary"]), "summary"
            else:
                conclusion, source = None, None
            node.runs.append(RunNode(
                id=r["id"], idea=i["name"], parent=parent, status=r["status"],
                change=r["change_reason"] or "", result=result,
                delta=(result - results[parent] if result is not None and parent
                       and results.get(parent) is not None
                       and run_idea.get(parent) == i["id"] else None),
                conclusion=conclusion, source=source, su=r["actual_cost"]))
            node.lessons.extend(lessons.get(r["id"], []))
            prev = r["id"]
        nodes[i["id"]] = node
    roots: list[IdeaNode] = []
    for i in ideas:
        node = nodes[i["id"]]
        parent = nodes.get(i["parent_id"]) if i["parent_id"] else None
        if parent is None:
            roots.append(node)
        else:
            node.parent = parent.name
            parent.children.append(node)
    return Tree(project=project, roots=roots, metric=metric, goal=goal)


# -- text

def _clip(text: str | None, n: int) -> str:
    return analysis.clip(text or "", n)


def fmt(v: float | None) -> str:
    return "—" if v is None else analysis.fmt_num(v)


def _delta(d: float | None) -> str:
    if d is None or d == 0:
        return ""
    return f"（{'+' if d > 0 else '−'}{analysis.fmt_num(abs(d))}）"


def run_line(r: RunNode, key: str | None) -> str:
    parts = [f"{r.id}" + (f" ← {r.parent}" if r.parent else ""), f"[{r.status}]"]
    if r.change:
        parts.append(f"改：{_clip(r.change, CHANGE_CLIP)}")
    if r.result is not None:
        parts.append(f"→ {key} {fmt(r.result)}{_delta(r.delta)}")
    line = " ".join(parts)
    if r.conclusion:
        line += f" · {SOURCE_LABEL[r.source or 'summary']}：{_clip(r.conclusion, CONCLUSION_CLIP)}"
    return line


def _goal_cn(goal: str) -> str:
    return "越低越好" if goal == "min" else "越高越好"


def idea_line(i: IdeaNode, here: str | None = None, *, folded: bool = False) -> str:
    head = f"◆ {i.name} [{IDEA_STATUS_CN.get(i.status, i.status)}]"
    if i.parent:
        head += f" ← {i.parent}" + (f"@{i.from_run}" if i.from_run else "")
    bits = [head]
    best = i.best
    if best is not None:
        bits.append(f"最好 {i.key} {fmt(best.result)}（{best.id}）")
    elif i.runs and not i.key:
        bits.append("没有结果（指标文字里找不到 metrics.jsonl 的键）")
    bits.append(f"{len(i.runs)} 个 run")
    if not folded and i.hypothesis:
        bits.append(f"假设：{_clip(i.hypothesis, HYPOTHESIS_CLIP)}")
    line = " · ".join(bits)
    if folded:
        below = sum(1 for _ in i.walk()) - 1
        if below:
            line += f" · 下面还有 {below} 个 idea"
        if i.lessons:
            line += f" · 经验：{_clip(i.lessons[-1], CONCLUSION_CLIP)}"
    if here and i.name == here:
        line += "  ← 你在这里"
    return line


def _render(tree: Tree, roots: list[IdeaNode], depth: int | None, here: str | None,
            fold_closed: bool, fold_runs: bool) -> str:
    on_path = {n.name for n in tree.path_to(here)} if here else set()
    lines: list[str] = []

    def go(node: IdeaNode, level: int) -> None:
        pad = "  " * level
        fold = fold_closed and node.status != "open" and node.name not in on_path
        if fold:
            lines.append(pad + idea_line(node, here, folded=True))
            return
        lines.append(pad + idea_line(node, here))
        runs = node.runs
        if fold_runs and node.name != here and len(runs) > FOLDED_RUNS:
            best = node.best
            keep = runs[-FOLDED_RUNS:]
            if best is not None and best not in keep:
                keep = [best, *keep]
            lines.append(f"{pad}    … 略去 {len(runs) - len(keep)} 个 run（`arb tree --idea {node.name}`）")
            runs = keep
        for r in runs:
            lines.append(f"{pad}    {run_line(r, node.key)}")
        for lesson in node.lessons:
            lines.append(f"{pad}    经验：{_clip(lesson, CONCLUSION_CLIP)}")
        if depth is not None and level + 1 >= depth:
            if node.children:
                lines.append(f"{pad}    … {len(node.children)} 个子 idea（加大 --depth）")
            return
        for c in node.children:
            go(c, level + 1)

    for r in roots:
        go(r, 0)
    return "\n".join(lines)


def header(tree: Tree) -> str:
    ideas = list(tree.ideas())
    n_runs = sum(len(i.runs) for i in ideas)
    open_ = sum(1 for i in ideas if i.status == "open")
    proj = f"项目 {tree.project} · " if tree.project else ""
    metric = (f"结果 = metrics.jsonl 的 {tree.metric}（{_goal_cn(tree.goal or 'min')}）"
              if tree.metric else "结果 = 各 idea 指标对应的 metrics.jsonl 键的最后一个值")
    return (f"[arboria] 研究树 · {proj}{len(ideas)} 个 idea（{open_} 个进行中）· {n_runs} 个 run\n"
            f"{metric}；结论：人 = 你对这个 run 的笔记，AI 摘要 = run summary 的第一句")


def render(tree: Tree, *, idea: str | None = None, depth: int | None = None,
           here: str | None = None, budget: int = TREE_CHARS) -> str:
    """The tree as text. Over `budget`, finished ideas off the current path fold to one
    line (result + lesson), then open ideas keep their best and last few runs."""
    if idea is not None:
        node = tree.find(idea)
        if node is None:
            raise ArbError(f"no idea named {idea!r} (see `arb idea list`)")
        roots = [node]
    else:
        roots = tree.roots
    head = header(tree)
    if not roots:
        return head + "\n还没有 idea。"
    body = ""
    for fold_closed, fold_runs in ((False, False), (True, False), (True, True)):
        body = _render(tree, roots, depth, here, fold_closed, fold_runs)
        if len(head) + 1 + len(body) <= budget:
            break
    text = head + "\n" + body
    if len(text) > budget:
        marker = "\n…（超出预算：用 `arb tree --idea NAME` 或 `--depth` 看一部分）"
        text = text[: max(budget - len(marker), 0)] + marker
    return text


def _scored(tree: Tree) -> list[tuple[IdeaNode, RunNode]]:
    """Runs with a result on the most used metric key, best first (ties by id)."""
    ideas = [i for i in tree.ideas() if i.key]
    if not ideas:
        return []
    counts: dict[str, int] = {}
    for i in ideas:
        counts[i.key] = counts.get(i.key, 0) + sum(1 for r in i.runs if r.result is not None)
    key = tree.metric or min(counts, key=lambda k: (-counts[k], k))
    rows = [(i, r) for i in ideas if i.key == key for r in i.runs if r.result is not None]
    if not rows:
        return []
    goal = tree.goal or rows[0][0].goal
    sign = 1 if goal == "min" else -1
    return sorted(rows, key=lambda ir: (sign * ir[1].result, ir[1].id))


def index(tree: Tree, here: str | None, budget: int = INDEX_CHARS) -> str:
    """The SessionStart view: where the session is, the current idea's recent runs, its
    siblings and children, and the best results. Everything else is `arb tree`."""
    lines = [header(tree), "全树 `arb tree`，子树 `arb tree --idea NAME`，结构化加 `--json`。"]
    node = tree.find(here) if here else None
    if node is None:
        lines.append("你在主工作区（不在某个 idea 里）。进行中的 idea：")
        open_ = [i for i in tree.ideas() if i.status == "open"]
        for i in open_[:12]:
            lines.append("- " + idea_line(i))
        if len(open_) > 12:
            lines.append(f"- … 另有 {len(open_) - 12} 个")
        if not open_:
            lines.append("- 没有")
    else:
        path = tree.path_to(node.name)
        lines.append("你在这里：" + " → ".join(
            p.name + (f"@{p.from_run}" if p.from_run and p is not path[0] else "") for p in path))
        lines.append(idea_line(node, here))
        lines.append(f"  指标：{_clip(node.metric_text, HYPOTHESIS_CLIP)}")
        shown = node.runs[-RECENT_RUNS:]
        if len(node.runs) > len(shown):
            lines.append(f"  … 更早的 {len(node.runs) - len(shown)} 个 run 见 `arb tree --idea {node.name}`")
        for r in shown:
            lines.append("  " + run_line(r, node.key))
        for lesson in node.lessons[-3:]:
            lines.append(f"  经验：{_clip(lesson, CONCLUSION_CLIP)}")
        parent = tree.find(node.parent) if node.parent else None
        siblings = [c for c in (parent.children if parent else tree.roots) if c is not node]
        if parent is not None:
            lines.append("上一层：" + idea_line(parent, folded=True))
        if siblings:
            lines.append("同一层：")
            lines.extend("- " + idea_line(s, folded=True) for s in siblings[:8])
            if len(siblings) > 8:
                lines.append(f"- … 另有 {len(siblings) - 8} 个")
        if node.children:
            lines.append("从这里分出的：")
            lines.extend("- " + idea_line(c, folded=True) for c in node.children[:8])
    top = _scored(tree)[:TOP_RESULTS]
    if top:
        lines.append("最好的结果：")
        for i, r in top:
            lines.append(f"- {i.name}/{r.id} {i.key} {fmt(r.result)} · 改：{_clip(r.change, CHANGE_CLIP)}"
                         + (f" · {SOURCE_LABEL[r.source or 'summary']}：{_clip(r.conclusion, 60)}"
                            if r.conclusion else ""))
    text = "\n".join(lines)
    if len(text) > budget:
        marker = "\n…（索引截断；`arb tree` 看全部）"
        text = text[: budget - len(marker)] + marker
    return text


# -- json

def _run_json(r: RunNode) -> dict[str, Any]:
    return {"id": r.id, "parent": r.parent, "status": r.status, "change_reason": r.change,
            "result": r.result, "delta": r.delta, "conclusion": r.conclusion,
            "conclusion_source": r.source, "su": r.su}


def _idea_json(i: IdeaNode, depth: int | None, level: int = 0) -> dict[str, Any]:
    best = i.best
    out: dict[str, Any] = {
        "name": i.name, "status": i.status, "parent": i.parent, "from_run": i.from_run,
        "hypothesis": i.hypothesis, "metric": i.metric_text, "metric_key": i.key,
        "goal": i.goal, "best": best.id if best else None,
        "runs": [_run_json(r) for r in i.runs], "lessons": i.lessons,
    }
    if depth is None or level + 1 < depth:
        out["children"] = [_idea_json(c, depth, level + 1) for c in i.children]
    else:
        out["children_omitted"] = len(i.children)
    return out


def to_json(tree: Tree, *, idea: str | None = None, depth: int | None = None,
            here: str | None = None) -> dict[str, Any]:
    if idea is not None:
        node = tree.find(idea)
        if node is None:
            raise ArbError(f"no idea named {idea!r} (see `arb idea list`)")
        roots = [node]
    else:
        roots = tree.roots
    return {"project": tree.project, "metric": tree.metric or None, "goal": tree.goal or None,
            "here": here, "conclusion_sources": list(CONCLUSION_SOURCES),
            "ideas": [_idea_json(r, depth) for r in roots]}
