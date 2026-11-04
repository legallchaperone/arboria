"""`lab.inspect`: the agent reads code for the human, and the answer lives in the notebook.

The human asks in a notebook cell (`lab.inspect("DMD 的训练循环", view="code")`); the
first call becomes a todo of kind `inspect` for the agent, the agent delivers with
`arb inspect done tNNN --file …`, the cell re-runs and from then on shows the cached
answer instantly (no tokens on re-run).

What an answer is bound to: the code it describes, as a commit (`code_ref`) and its tree.

- `run="r003"`: the run's snapshot commit (fixed; never stale).
- otherwise the idea's current code: the worktree exactly as it is, uncommitted and
  untracked files included, snapshotted read-only like a run's (snapshot.write_tree: a
  throw-away index, never the user's branch, index or files). The commit is made only
  when a todo is created and anchored at `refs/arboria/inspect/<todo>`. Later calls
  compare trees: when the worktree has changed, the cached answer is shown with a muted
  "代码已变" line and `.refresh()` asks again. Nothing regenerates by itself.

Views: `code` (source excerpts with the agent's notes), `pseudo` (pseudocode), `simple`
(simplified code), `map` (a Mermaid diagram). For every view the agent supplies only
*locations* of real code (path + line range at code_ref); arb extracts those lines from
git itself (`git cat-file blob <commit>:<path>`, read-only) when the answer is delivered
and stores them with it, so the code shown is exactly the repo's. Everything the agent
writes (title, summary, notes, pseudocode, Mermaid) is length-checked here and
HTML-escaped when rendered.
"""

from __future__ import annotations

import html
import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from . import gitutil, records, snapshot, tasks
from .db import INSPECT_VIEWS, LIMITS, Ledger, now, validate_choice, validate_text
from .errors import ArbError
from .paths import RepoContext

VIEW_LABELS = {"code": "代码摘录", "pseudo": "伪代码", "simple": "简化代码", "map": "架构图"}
REF_PREFIX = "refs/arboria/inspect/"
MAX_SPANS = 6
MAX_LINES = 400  # source lines over all spans
MAX_LINE_NOTES = 40
MAX_NODES = 30
MAX_FILE_BYTES = 2 * 1024 * 1024
SHORT = 7
KEYS = {
    "code": {"title", "summary", "spans"},
    "pseudo": {"title", "summary", "spans", "text"},
    "simple": {"title", "summary", "spans", "text", "lang"},
    "map": {"title", "summary", "spans", "mermaid", "nodes"},
}
SPAN_KEYS = {"id", "path", "start", "end", "note", "line_notes"}
ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,40}$")
LANG_RE = re.compile(r"^[a-z0-9+#-]{1,20}$")
MERMAID_KINDS = (
    "flowchart", "graph", "sequenceDiagram", "classDiagram", "classDiagram-v2", "stateDiagram",
    "stateDiagram-v2", "erDiagram", "journey", "gantt", "pie", "mindmap", "timeline",
    "block-beta", "architecture-beta", "C4Context", "C4Container", "C4Component",
    "quadrantChart", "requirementDiagram", "gitGraph", "sankey-beta", "xychart-beta",
    "packet-beta",
)
# HTML in labels: only line breaks. No init directives (they can loosen mermaid's
# security level), no click handlers or links.
_TAG_RE = re.compile(r"<\s*/?\s*[A-Za-z!?]")
_BR_RE = re.compile(r"<br\s*/?>", re.I)
_CTRL_RE = re.compile(r"[\x00-\x08\x0b-\x1f\x7f]")


def short(sha: str | None) -> str:
    return (sha or "")[:SHORT]


def ref_name(task_id: str) -> str:
    return REF_PREFIX + task_id


def _num(task_id: str) -> int:
    return int(task_id[1:]) if task_id[1:].isdigit() else 0


# ---------------------------------------------------------------- the code an answer is about


@dataclass(frozen=True)
class Code:
    """The code a question is about: `target` is "idea" or a run id."""

    target: str
    tree: str
    commit: str | None  # None: the idea's worktree, no commit made yet
    head: str | None = None
    worktree: Path | None = None

    @property
    def label(self) -> str:
        return "当前代码" if self.target == "idea" else f"run {self.target} 的快照"


def current_code(ledger: Ledger, idea: dict[str, Any], run: str | None = None) -> Code:
    """What `lab.inspect` is about right now (read-only: no commit, no ref)."""
    if run is not None:
        row = records.get_run(ledger, run)
        return Code(row["id"], row["tree_hash"], row["snapshot_commit"])
    worktree = Path(idea["worktree_path"])
    if not worktree.is_dir():
        raise ArbError(f"idea {idea['name']} has no worktree ({worktree}); ask about a run "
                       "instead: lab.inspect(..., run=\"rNNN\")")
    tree, head, _ = snapshot.write_tree(worktree)
    return Code("idea", tree, None, head, worktree)


def _commit(ledger: Ledger, code: Code) -> str:
    """A commit for `code` (an earlier run's with the same tree when there is one)."""
    if code.commit is not None:
        return code.commit
    assert code.worktree is not None
    found = snapshot.find_existing_commit(ledger.conn, code.tree, code.worktree)
    return found or snapshot.commit_tree(code.worktree, code.tree, code.head,
                                         "arboria inspect snapshot")


# ---------------------------------------------------------------- the notebook side


def validate_question(question: Any) -> str:
    if not isinstance(question, str):
        raise ArbError("lab.inspect 的第一个参数是一句问题，例如 lab.inspect(\"训练循环\")")
    q = validate_text("question", question)
    return " ".join(q.split())


def _payload_key(p: dict[str, Any]) -> tuple:
    return (p.get("question"), p.get("view"), p.get("target"))


def latest(ledger: Ledger, idea: dict[str, Any], question: str, view: str, target: str
           ) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    """(newest answer, newest todo) for this question: the answer is an `inspections` row
    with `result` parsed; the todo is any status."""
    rows = ledger.all("SELECT * FROM inspections WHERE idea_id = ? AND question = ? AND view = ?"
                      " AND target = ?", (idea["id"], question, view, target))
    answer = max(rows, key=lambda r: _num(r["task_id"]), default=None)
    if answer is not None:
        answer = dict(answer, result=json.loads(answer["result_json"]))
    todos = [t for t in ledger.all(tasks.SELECT + " WHERE t.idea_id = ? AND t.kind = 'inspect'",
                                   (idea["id"],))
             if _payload_key(tasks.payload(t)) == (question, view, target)]
    todo = max(todos, key=lambda t: _num(t["id"]), default=None)
    return answer, todo


@dataclass
class State:
    """What `lab.inspect` shows: an answer (maybe stale), a pending todo, or a dropped one."""

    question: str
    view: str
    code: Code
    answer: dict[str, Any] | None = None  # inspections row + parsed `result`
    todo: dict[str, Any] | None = None  # the open (pending) or dropped todo shown
    created: bool = False

    @property
    def status(self) -> str:
        if self.todo is not None and self.todo["status"] == "open":
            return "pending"
        if self.todo is not None and self.todo["status"] == "dropped":
            return "dropped"
        return "stale" if self.stale else "fresh"

    @property
    def stale(self) -> bool:
        return self.answer is not None and self.answer["tree"] != self.code.tree


def todo_text(question: str, view: str, code: Code, commit: str) -> str:
    head = f"读代码（{VIEW_LABELS[view]}）：{question}"
    tail = (f"\n代码：{code.label} @ {short(commit)}"
            "\n约定见 `arb inspect guide`；用 `arb inspect done tNNN --file …` 交付")
    return head + tail


def ask(ledger: Ledger, idea: dict[str, Any], question: str, view: str = "code",
        run: str | None = None, *, refresh: bool = False, create: bool = True) -> State:
    """Look up the answer to `question`, or ask the agent for one (a todo).

    - an answer for exactly this code: shown (`refresh` changes nothing);
    - an open todo for this question: that one (no duplicates when the cell re-runs);
    - an answer for older code: shown as stale, unless `refresh`;
    - the last todo was dropped (and no answer came after): shown, unless `refresh`;
    - otherwise a new todo, bound to the code as it is now.
    `create=False` never writes (returns the state with `todo=None` instead)."""
    question = validate_question(question)
    validate_choice("view", view, INSPECT_VIEWS)
    code = current_code(ledger, idea, run)
    answer, todo = latest(ledger, idea, question, view, code.target)
    state = State(question, view, code, answer)
    if answer is not None and not state.stale:
        return state
    if todo is not None and todo["status"] == "open":
        state.todo = todo
        return state
    newer_drop = (todo is not None and todo["status"] == "dropped"
                  and (answer is None or _num(todo["id"]) > _num(answer["task_id"])))
    if not refresh and (answer is not None or newer_drop):
        state.todo = todo if newer_drop else None
        return state
    if not create:
        return state
    if idea["status"] != "open":
        raise ArbError(f"idea {idea['name']!r} is {idea['status']}: no new todos")
    commit = _commit(ledger, code)
    data = {"question": question, "view": view, "target": code.target, "idea": idea["name"],
            "code_ref": commit, "tree": code.tree}
    task = tasks.add(ledger, idea, todo_text(question, view, code, commit), "inspect", data)
    if code.target == "idea":  # keep the snapshot commit reachable (a run's already is)
        snapshot.update_ref(code.worktree or Path(idea["worktree_path"]),
                            ref_name(task["id"]), commit, f"arboria: inspect {task['id']}")
    state.todo, state.created = task, True
    return state


# ---------------------------------------------------------------- reading code at code_ref


def check_path(path: Any) -> str:
    if not isinstance(path, str) or not path or len(path) > 300:
        raise ArbError(f"path must be a repo-relative file path, got {path!r}")
    p = PurePosixPath(path)
    if p.is_absolute() or "\\" in path or any(part in ("", ".", "..") for part in path.split("/")):
        raise ArbError(f"path {path!r}: use a plain repo-relative path like src/train.py "
                       "(no leading /, no . or ..)")
    return path


def blob_lines(repo: Path, commit: str, path: str) -> list[str]:
    """The lines of `path` at `commit`, exactly as git stores them (read-only)."""
    check_path(path)
    kind = gitutil.run(["cat-file", "-t", f"{commit}:{path}"], repo, check=False)
    if kind.returncode != 0:
        raise ArbError(f"{path} does not exist at {short(commit)} (see `arb inspect source "
                       "tNNN` for the files)")
    if kind.stdout != "blob":
        raise ArbError(f"{path} is a {kind.stdout} at {short(commit)}, not a file")
    size = int(gitutil.out(["cat-file", "-s", f"{commit}:{path}"], repo))
    if size > MAX_FILE_BYTES:
        raise ArbError(f"{path} is too large to show ({size} bytes)")
    # bytes, not text mode: universal newlines would turn \r\n into \n
    proc = subprocess.run(["git", "cat-file", "blob", f"{commit}:{path}"], cwd=repo,
                          env=gitutil._env(None), capture_output=True, stdin=subprocess.DEVNULL)
    if proc.returncode != 0:
        raise ArbError(f"cannot read {path} at {short(commit)}: {proc.stderr.decode().strip()}")
    text = proc.stdout.decode("utf-8", errors="replace")
    if "\0" in text:
        raise ArbError(f"{path} is a binary file")
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    return lines


def list_files(repo: Path, commit: str) -> list[str]:
    out = gitutil.run(["ls-tree", "-r", "-z", "--name-only", commit], repo, strip=False).stdout
    return [p for p in out.split("\0") if p]


# ---------------------------------------------------------------- the agent's answer


def _text(field: str, value: Any, *, required: bool = False) -> str | None:
    if value is not None and not isinstance(value, str):
        raise ArbError(f"{field} must be a string")
    return validate_text(field, value, required=required)


def _int(value: Any, what: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ArbError(f"{what} must be an integer line number, got {value!r}")
    return value


def check_mermaid(src: Any) -> str:
    text = _text("mermaid", src, required=True)
    assert text is not None
    if _CTRL_RE.search(text):
        raise ArbError("mermaid: control characters are not allowed")
    lines = [l.strip() for l in text.splitlines()]
    body = [l for l in lines if l and not l.startswith("%%")]
    first = body[0].split()[0] if body else ""
    if first not in MERMAID_KINDS:
        raise ArbError(f"mermaid must start with a diagram type ({', '.join(MERMAID_KINDS[:6])}, …), "
                       f"got {first!r}")
    if "%%{" in text:
        raise ArbError("mermaid: no %%{init}%% directives (theme and security are arb's)")
    for l in lines:
        if re.match(r"^click\b", l) or "javascript:" in l.lower() or "href" in l.lower():
            raise ArbError(f"mermaid: no click handlers or links ({l[:60]!r})")
    if _TAG_RE.search(_BR_RE.sub("", text)):
        raise ArbError("mermaid: no HTML in labels (only <br>); write plain text")
    return text


def validate_result(repo: Path, task: dict[str, Any], data: Any) -> dict[str, Any]:
    """Check the agent's answer for an inspect todo and return what is stored: the answer
    plus, per span, the real lines extracted from git at the todo's code_ref."""
    if task.get("kind") != "inspect":
        raise ArbError(f"todo {task['id']} is not a 读代码 todo (kind {task.get('kind')}); "
                       "use `arb todo done`")
    p = tasks.payload(task)
    view, commit = p["view"], p["code_ref"]
    if not isinstance(data, dict):
        raise ArbError("the answer must be a JSON object")
    extra = set(data) - KEYS[view]
    if extra:
        raise ArbError(f"unknown keys for view={view}: {', '.join(sorted(extra))} "
                       f"(allowed: {', '.join(sorted(KEYS[view]))})")
    out: dict[str, Any] = {"view": view, "question": p["question"], "code_ref": commit,
                           "tree": p["tree"], "target": p["target"]}
    out["title"] = _text("inspect_title", data.get("title"), required=True)
    out["summary"] = _text("inspect_summary", data.get("summary"))
    raw_spans = data.get("spans") or []
    if not isinstance(raw_spans, list):
        raise ArbError("spans must be a list")
    if view == "code" and not raw_spans:
        raise ArbError("view=code needs at least one span (the code to show)")
    if len(raw_spans) > MAX_SPANS:
        raise ArbError(f"{len(raw_spans)} spans (max {MAX_SPANS}): keep to the core")
    spans, ids, total, notes = [], set(), 0, 0
    cache: dict[str, list[str]] = {}
    for i, s in enumerate(raw_spans, 1):
        if not isinstance(s, dict):
            raise ArbError(f"spans[{i - 1}] must be an object")
        bad = set(s) - SPAN_KEYS
        if bad:
            raise ArbError(f"spans[{i - 1}]: unknown keys {', '.join(sorted(bad))}")
        sid = s.get("id", f"s{i}")
        if not isinstance(sid, str) or not ID_RE.match(sid) or sid in ids:
            raise ArbError(f"spans[{i - 1}].id {sid!r}: letters, digits, _ or -, unique")
        ids.add(sid)
        path = check_path(s.get("path"))
        start, end = _int(s.get("start"), f"{path} start"), _int(s.get("end"), f"{path} end")
        if path not in cache:
            cache[path] = blob_lines(repo, commit, path)
        lines = cache[path]
        if not 1 <= start <= end <= len(lines):
            raise ArbError(f"{path}:{start}-{end} is outside the file ({len(lines)} lines at "
                           f"{short(commit)})")
        total += end - start + 1
        if total > MAX_LINES:
            raise ArbError(f"the spans cover more than {MAX_LINES} lines: keep to the core "
                           "(shorter ranges, fewer spans)")
        line_notes: dict[str, str] = {}
        raw_notes = s.get("line_notes") or {}
        if not isinstance(raw_notes, dict):
            raise ArbError(f"{path}: line_notes must map line numbers to notes")
        for k, v in raw_notes.items():
            n = int(k) if isinstance(k, str) and k.isdigit() else None
            if n is None or not start <= n <= end:
                raise ArbError(f"{path}: line_notes key {k!r} is not a line in {start}-{end}")
            line_notes[str(n)] = _text("line_note", v, required=True)  # type: ignore[assignment]
        notes += len(line_notes)
        if notes > MAX_LINE_NOTES:
            raise ArbError(f"more than {MAX_LINE_NOTES} line notes: annotate only what matters")
        spans.append({"id": sid, "path": path, "start": start, "end": end,
                      "note": _text("span_note", s.get("note")),
                      "line_notes": dict(sorted(line_notes.items(), key=lambda kv: int(kv[0]))),
                      "lines": lines[start - 1:end]})
    out["spans"] = spans
    if view in ("pseudo", "simple"):
        out["text"] = _text("inspect_text", data.get("text"), required=True)
        if view == "simple":
            lang = data.get("lang", "python")
            if not isinstance(lang, str) or not LANG_RE.match(lang):
                raise ArbError(f"lang {lang!r}: a short language name like python")
            out["lang"] = lang
    if view == "map":
        out["mermaid"] = check_mermaid(data.get("mermaid"))
        nodes = data.get("nodes") or {}
        if not isinstance(nodes, dict) or len(nodes) > MAX_NODES:
            raise ArbError(f"nodes must map node ids to span ids (at most {MAX_NODES})")
        clean: dict[str, list[str]] = {}
        for node, refs in nodes.items():
            if not isinstance(node, str) or not ID_RE.match(node):
                raise ArbError(f"nodes: bad node id {node!r}")
            if not re.search(rf"(?<![A-Za-z0-9_-]){re.escape(node)}(?![A-Za-z0-9_-])",
                             out["mermaid"]):
                raise ArbError(f"nodes: {node!r} does not appear in the diagram")
            refs = [refs] if isinstance(refs, str) else refs
            if not isinstance(refs, list) or not refs or any(r not in ids for r in refs):
                raise ArbError(f"nodes[{node!r}] must list span ids from spans "
                               f"({', '.join(sorted(ids)) or 'none given'})")
            clean[node] = list(refs)
        out["nodes"] = clean
    return out


def load_answer(text: str) -> Any:
    try:
        return json.loads(text)
    except ValueError as exc:
        raise ArbError(f"the answer is not valid JSON: {exc}") from exc


def open_inspect_task(ledger: Ledger, task_id: str) -> dict[str, Any]:
    task = tasks.get_task(ledger, task_id)
    if task.get("kind") != "inspect":
        raise ArbError(f"todo {task_id} is not a 读代码 todo (kind {task.get('kind')})")
    return task


def deliver(ledger: Ledger, repo: Path, task_id: str, data: Any) -> dict[str, Any]:
    """Store the answer and close the todo (one transaction). Returns the stored answer."""
    task = open_inspect_task(ledger, task_id)
    if task["status"] != "open":
        raise ArbError(f"todo {task_id} is already {task['status']}")
    result = validate_result(repo, task, data)
    p = tasks.payload(task)
    summary = result["title"] + (f"：{result['summary']}" if result["summary"] else "")
    summary = summary if len(summary) <= LIMITS["result"] else summary[: LIMITS["result"] - 1] + "…"
    with ledger.write() as conn:
        ts = now()
        conn.execute("INSERT INTO inspections (task_id, idea_id, question, view, target, code_ref,"
                     " tree, result_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                     (task_id, task["idea_id"], p["question"], p["view"], p["target"],
                      p["code_ref"], p["tree"], json.dumps(result, ensure_ascii=False), ts))
        conn.execute("UPDATE tasks SET status = 'done', done_at = ?, result = ? WHERE id = ?",
                     (ts, summary, task_id))
        lines = sum(len(s["lines"]) for s in result["spans"])
        ledger.event("inspect_done", task_id, {"idea": task["idea"], "view": p["view"],
                                               "spans": len(result["spans"]), "lines": lines})
        ledger.event("todo_done", task_id, {"idea": task["idea"], "kind": "inspect",
                                            "result_chars": len(summary)})
    return result


# ---------------------------------------------------------------- rendering

MUTED = "opacity:.62"
CODE = "padding:1px 6px;border-radius:4px;background:rgba(127,127,127,.12)"
ACCENT = "rgba(56,118,186,.75)"  # the agent's words: a calm blue rule on the left


def esc(text: Any) -> str:
    return html.escape("" if text is None else str(text), quote=True)


def span_label(s: dict[str, Any]) -> str:
    return f"{s['path']}:{s['start']}–{s['end']}"


def code_table(s: dict[str, Any]) -> str:
    """The real lines with numbers; the agent's line notes as separate rows under them."""
    notes = s.get("line_notes") or {}
    rows = []
    for i, line in enumerate(s["lines"], s["start"]):
        rows.append(f'<tr><td style="text-align:right;padding:0 12px 0 8px;{MUTED};'
                    f'user-select:none;vertical-align:top">{i}</td>'
                    f'<td style="white-space:pre;padding-right:12px">{esc(line) or " "}</td></tr>')
        note = notes.get(str(i))
        if note:
            rows.append(f'<tr class="arb-inspect-note"><td></td><td style="white-space:pre-wrap;'
                        f'font-family:var(--arboria-sans,system-ui,sans-serif);font-size:12.5px;'
                        f'padding:3px 10px;margin:2px 0;border-left:2px solid {ACCENT};'
                        f'background:rgba(56,118,186,.07)">'
                        f'<span style="{MUTED}">agent：</span>{esc(note)}</td></tr>')
    return ('<div style="overflow-x:auto;margin:4px 0 2px;border-radius:6px;'
            'background:rgba(127,127,127,.06);padding:6px 0">'
            '<table class="font-mono" style="border-collapse:collapse;font-size:12.5px;'
            f'line-height:1.6;margin:0">{"".join(rows)}</table></div>')


def span_head(s: dict[str, Any], commit: str) -> str:
    return (f'<div style="margin-top:12px"><code style="{CODE}">{esc(span_label(s))}</code>'
            f' <span style="{MUTED};font-size:.9em">@ {esc(short(commit))}</span></div>')


def agent_note(text: str | None, label: str = "agent") -> str:
    if not text:
        return ""
    return (f'<div style="white-space:pre-wrap;margin:4px 0;padding:2px 10px;'
            f'border-left:2px solid {ACCENT}"><span style="{MUTED}">{esc(label)}：</span>'
            f"{esc(text)}</div>")


def _status_line(state: State) -> str:
    """The muted line about where the answer stands (empty when it is fresh)."""
    st = state.status
    if st == "pending":
        t = state.todo
        assert t is not None
        lead = "已交给 agent" if state.created else "等 agent"
        old = "，下面是上一版" if state.answer is not None else ""
        return f"{lead}（{t['id']}）：{VIEW_LABELS[state.view]} · {state.question}{old}。"
    if st == "dropped":
        t = state.todo
        assert t is not None
        why = f"：{t['result']}" if t.get("result") else ""
        return f"agent 没有做（{t['id']}{why}）。.refresh() 重新交给 agent"
    if st == "stale":
        assert state.answer is not None
        return (f"代码已变（答案基于 {short(state.answer['code_ref'])}，之后工作区改过），"
                ".refresh() 重新生成")
    return ""


def body_html(result: dict[str, Any], *, mermaid_inline: bool = True) -> tuple[str, str]:
    """(html before the diagram, html after it). For views other than map the second part
    is empty. With `mermaid_inline` the Mermaid source is shown as text (no marimo)."""
    commit = result["code_ref"]
    view = result["view"]
    out = []
    if result.get("summary"):
        out.append(f'<div style="white-space:pre-wrap;margin:6px 0">{esc(result["summary"])}</div>')
    after: list[str] = []
    if view == "code":
        for s in result["spans"]:
            out.append(span_head(s, commit) + agent_note(s.get("note")) + code_table(s))
        foot = (f"代码由 arb 从 git 取出（{short(commit)}），与仓库逐字一致；"
                "说明和批注是 agent 写的")
    else:
        if view in ("pseudo", "simple"):
            what = "伪代码" if view == "pseudo" else "简化代码"
            out.append(f'<div style="margin-top:10px;{MUTED};font-size:.9em">agent 写的{what}'
                       "（不是仓库里的代码）</div>"
                       '<pre class="font-mono" style="font-size:12.5px;line-height:1.6;'
                       'margin:4px 0;padding:10px 12px;border-radius:6px;white-space:pre-wrap;'
                       f'background:rgba(56,118,186,.06);border-left:2px solid {ACCENT}">'
                       f"{esc(result['text'])}</pre>")
        elif mermaid_inline:
            out.append(f'<div style="margin-top:10px;{MUTED};font-size:.9em">agent 画的架构图'
                       "（Mermaid 源码；在 marimo 里显示成图）</div>"
                       '<pre class="font-mono" style="font-size:12.5px;line-height:1.6;'
                       'margin:4px 0;padding:10px 12px;border-radius:6px;white-space:pre-wrap;'
                       f'background:rgba(127,127,127,.06)">{esc(result["mermaid"])}</pre>')
        target = after if view == "map" and not mermaid_inline else out
        nodes = result.get("nodes") or {}
        by_span = {}
        for node, refs in nodes.items():
            for r in refs:
                by_span.setdefault(r, []).append(node)
        if result["spans"]:
            target.append(f'<div style="margin-top:10px;{MUTED};font-size:.9em">对应的源码'
                          f"（arb 从 git 取出，{esc(short(commit))}）</div>")
        for s in result["spans"]:
            tag = (f' <span style="{MUTED};font-size:.9em">图中 {esc("、".join(by_span[s["id"]]))}'
                   "</span>") if s["id"] in by_span else ""
            target.append(f'<details style="margin:4px 0"><summary style="cursor:pointer">'
                          f'<code style="{CODE}">{esc(span_label(s))}</code>{tag}</summary>'
                          + agent_note(s.get("note")) + code_table(s) + "</details>")
        foot = ("图是 agent 画的" if view == "map" else f"上面的{VIEW_LABELS[view]}是 agent 写的") + \
            (f"；源码由 arb 从 git 取出（{short(commit)}）" if result["spans"] else "")
    (after if view == "map" and not mermaid_inline else out).append(
        f'<div style="{MUTED};font-size:.85em;margin-top:8px">{esc(foot)}</div>')
    return "".join(out), "".join(after)


def head_html(state: State) -> str:
    result = state.answer["result"] if state.answer else None
    title = result["title"] if result else state.question
    meta = f"读代码 · {VIEW_LABELS[state.view]}"
    if state.code.target != "idea":
        meta += f" · run {state.code.target}"
    if result and result["title"] != state.question:
        meta += f" · {state.question}"
    out = (f'<div><strong>{esc(title)}</strong> <span style="{MUTED};font-size:.9em">'
           f"{esc(meta)}</span></div>")
    line = _status_line(state)
    if line:
        color = "" if state.status != "stale" else "color:#b7791f;"
        out += f'<div style="{MUTED};{color}font-size:.9em;margin-top:2px">{esc(line)}</div>'
    return out


def wrap(inner: str) -> str:
    return f'<div class="arb-inspect" style="line-height:1.7">{inner}</div>'


def to_html(state: State) -> str:
    """The whole answer as one HTML block (Mermaid shown as source)."""
    inner = head_html(state)
    if state.answer is not None:
        inner += body_html(state.answer["result"])[0]
    return wrap(inner)


def to_text(state: State | None, result: dict[str, Any] | None = None) -> str:
    """Plain text: the CLI preview (`arb inspect check`) and repr outside a notebook."""
    lines: list[str] = []
    if state is not None:
        result = state.answer["result"] if state.answer else None
        lines.append(f"{result['title'] if result else state.question}  "
                     f"[读代码 · {VIEW_LABELS[state.view]}]")
        status = _status_line(state)
        if status:
            lines.append(status)
    elif result is not None:
        lines.append(f"{result['title']}  [读代码 · {VIEW_LABELS[result['view']]}]")
    if result is None:
        return "\n".join(lines)
    commit = result["code_ref"]
    if result.get("summary"):
        lines += ["", result["summary"]]
    if result["view"] in ("pseudo", "simple"):
        lines += ["", f"--- agent 写的{VIEW_LABELS[result['view']]} ---", result["text"]]
    if result["view"] == "map":
        lines += ["", "--- Mermaid ---", result["mermaid"]]
        for node, refs in (result.get("nodes") or {}).items():
            lines.append(f"  {node} → {', '.join(refs)}")
    width = max((len(str(s["end"])) for s in result["spans"]), default=1)
    for s in result["spans"]:
        lines += ["", f"[{s['id']}] {span_label(s)} @ {short(commit)}"]
        if s.get("note"):
            lines.append(f"  agent：{s['note']}")
        for i, line in enumerate(s["lines"], s["start"]):
            lines.append(f"{i:>{width}} │ {line}")
            note = (s.get("line_notes") or {}).get(str(i))
            if note:
                lines.append(f"{'':>{width}} ↳ agent：{note}")
    return "\n".join(lines)


# ---------------------------------------------------------------- telling the session


def push_text(task: dict[str, Any]) -> str:
    """The short message posted into the running Claude Code session for a new inspect
    todo (like commands.push_text). Built from validated fields only: the todo id, the
    view (a closed set) and the idea name, never the human's question."""
    view = validate_choice("view", tasks.payload(task).get("view"), INSPECT_VIEWS)
    return (f"[arboria] notebook 里有新的读代码待办 {task['id']}：{VIEW_LABELS[view]}"
            f"（idea {task['idea']}）。运行 arb inspect guide 查看约定，"
            "arb todo list --json 看问题；读完用 "
            f"arb inspect check {task['id']} --file - 预览，arb inspect done {task['id']} --file - 交付。")


# ---------------------------------------------------------------- the guide for the agent

GUIDE = """\
# 读代码待办（lab.inspect）：给 agent 的约定

人在 notebook 里写 `lab.inspect("问题", view=...)`，想让你讲一段代码。你交付的答案会缓存在
账本里，之后这个 cell 每次重跑都直接显示，不再花 token。所以答案要准、要短、只讲核心。

不管你是在人的会话里、作为子代理，还是被单独启动来做这一个待办，步骤都一样：所有东西
都在待办和下面这几个 `arb inspect` 命令里，在这个仓库的任何一个 worktree 里都能运行。

## 先看待办
`arb todo list --json`：kind 是 inspect，payload 里有 question（人的问题）、view（code /
pseudo / simple / map）、target（idea 或 run id）和 code_ref（代码所在的 commit）。

答案绑定在 code_ref 这个版本的代码上。读代码一律用
`arb inspect source tNNN PATH`（带行号，和 arb 取代码用的是同一个版本），
`arb inspect source tNNN` 列出这个版本里的文件。工作区里的文件可能已经改过，行号会对不上。

## 关键：代码由 arb 来取
你只给位置（path + 起止行号）和说明；arb 自己从 git 里取出那几行显示给人，所以人看到的
代码和仓库里逐字一致。不要把代码抄进说明里。行号要准：start、end 都包含在内，从 1 开始。

## 答案（JSON）
```json
{{
  "title": "DMD 的训练循环（≤80 字）",
  "summary": "两三句话讲清它在做什么（≤600 字）",
  "spans": [
    {{"id": "loop", "path": "train.py", "start": 120, "end": 168,
     "note": "这一段在做什么、为什么（≤600 字）",
     "line_notes": {{"131": "这一行的要点（≤200 字）"}}}}
  ]
}}
```
- 只讲核心：最多 {max_spans} 段、合计不超过 {max_lines} 行、批注合计最多 {max_notes} 条。
  日志、分布式、参数解析这类样板不要放进来。
- 批注写给人看，用人的语言（中文）；说“为什么”，不要逐行复述代码。
- view=code：spans 至少一段。
- view=pseudo：加 "text"（伪代码，≤8000 字）；spans 列出它对应的源码段（人可以展开看原代码）。
- view=simple：加 "text"（去掉日志、分布式、样板之后的简化代码，≤8000 字），可选 "lang"
  （默认 python）；spans 同上。
- view=map：加 "mermaid"（Mermaid 源码，≤8000 字，第一行是图的类型，如 `flowchart TD`），
  可选 "nodes"：{{"节点 id": ["span id", ...]}} 把图里的节点连到源码段。标签里只用纯文字
  （换行用 <br>），不要 `%%{{init}}%%`、`click`、链接或 HTML。
- 不认识的键会被拒绝。

## 先预览，再交付
```bash
arb inspect check tNNN --file - <<'EOF'
{{ ...答案... }}
EOF
```
`check` 只校验并打印人会看到的样子（取出的代码和你的批注），不保存。没问题再把同样的
内容交给 `arb inspect done tNNN --file -`（或 `--file answer.json`）。交付后 notebook 里的
cell 会自动重跑。读代码待办不用 `arb todo done`；做不了就 `arb todo drop tNNN --reason "..."`。

## 不要
- 不改 notebook，不改代码：读代码待办只读。
- 不编造：只讲 code_ref 里真有的东西；不确定就在说明里写“不确定”。
"""


def guide() -> str:
    return GUIDE.format(max_spans=MAX_SPANS, max_lines=MAX_LINES, max_notes=MAX_LINE_NOTES)
