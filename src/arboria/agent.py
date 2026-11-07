"""The background agent: one `claude -p` per todo, started by the human (ADR 0004 P4).

`arb agent run [tNNN]` hands one todo (plain, 读代码 or command) to the user's own `claude`,
non-interactively, in the idea's worktree, and records what it cost. Nobody watches it, so
it gets less than an interactive session, not more:

- Lean flags (P0 ⑦): `--strict-mcp-config --disable-slash-commands --setting-sources ""`,
  so none of the user's MCP servers, skills or settings load; what it may do comes only from
  `--settings` and `--allowedTools` built here.
- Tools: Read/Grep/Glob, and Bash only for the arb commands this todo needs (its own
  `arb todo done tNNN` / `arb inspect done tNNN`, not another todo's). It cannot edit code
  or submit runs; a command todo may write only the command file in the payload.
- Sandbox: on, with no excluded commands, so even arb runs inside it. Writes: the main
  worktree's `.arboria/` and git dir (arb's ledger and refs), never the notebooks or the
  approval keys. No network (none of the allowed commands needs it).
- Human-only commands still refuse: claude sets CLAUDECODE for its Bash, and stdin is no TTY.
- Limits (`arboria.toml [agent]`): one at a time (a lock file), `daily_limit` runs per 24 h,
  `max_turns`, `timeout_minutes`. Background and interactive use share the subscription's
  five_hour window, so a run stops once `rate_limit_event` shows five_hour utilization at
  `pause_above`, and none starts again until that window resets.

Each run is one `agent_run` event (token counts, window utilization, the session id to
`claude --resume`), and its stream-json is kept in `.arboria/agent/` (not backed up).
Nothing here approves anything; arbd does not call it.
"""

from __future__ import annotations

import fcntl
import json
import os
import selectors
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Iterator

from . import commands as commands_mod
from . import human, tasks
from .config import AgentConfig
from .db import Ledger, now
from .errors import ArbError
from .paths import RepoContext

LOG_DIR = "agent"
LOCK_FILE = "agent.lock"
EVENT = "agent_run"
KINDS = ("plain", "inspect", "command_new", "command_revise")
WINDOW = "five_hour"
KEEP_ENV = ("CLAUDE_CONFIG_DIR",)
CLAUDE_ENV = "ARBORIA_CLAUDE"  # the claude executable (default: `claude` on PATH)

# Everything here only reads.
READ_COMMANDS = ("arb status", "arb tree", "arb todo list", "arb idea show", "arb idea list",
                 "arb lesson list", "arb commands list", "arb card")

PROMPT_HEAD = """\
你是 Arboria 的后台 agent，在 idea {idea} 的 worktree 里处理一条待办。没有人在看，不能提问；
做完就停。

待办 {id}（{kind}）：{text}

规矩：
- 只能用 Read / Grep / Glob 和下面列出的 arb 命令。不能改代码、不能提交 run、不能动 notebook。
- 先用 `arb status --json` 和 `arb tree --idea {idea} --json` 了解背景。
- 做不了（需要改代码、跑实验、问人）就不要交付，也不要 drop，用一两句话说明原因后停止。
"""

PROMPT_KIND = {
    "plain": """\
这是普通待办：读代码和账本回答。交付：`arb todo done {id} --result "…" --json`（≤600 字，写给人看，用中文）。
""",
    "inspect": """\
这是读代码待办：先运行 `arb inspect guide` 并照它做。代码用 `arb inspect source {id} PATH` 读，
答案先 `arb inspect check {id} --file - --json` 校验，再 `arb inspect done {id} --file - --json` 交付。
""",
    "command_new": """\
这是命令待办：先运行 `arb commands guide` 并照它做。只写这个文件：{path}
（用 Write / Edit；不能在终端里试运行）。写好后 `arb todo done {id} --result "…" --json`。
""",
}
PROMPT_KIND["command_revise"] = PROMPT_KIND["command_new"]


@dataclass
class Plan:
    task: dict[str, Any]
    idea: dict[str, Any]
    cwd: Path
    prompt: str
    allowed: list[str]
    settings: dict[str, Any]
    add_dirs: list[str] = field(default_factory=list)

    def argv(self, claude: str, cfg: AgentConfig) -> list[str]:
        tools = ["Read", "Grep", "Glob", "Bash"] + (["Write", "Edit"] if self.add_dirs else [])
        argv = [claude, "-p", self.prompt, "--output-format", "stream-json", "--verbose",
                "--max-turns", str(cfg.max_turns), "--tools", ",".join(tools),
                "--allowedTools", *self.allowed,
                "--strict-mcp-config", "--disable-slash-commands", "--setting-sources", "",
                "--settings", json.dumps(self.settings, ensure_ascii=False)]
        for d in self.add_dirs:
            argv += ["--add-dir", d]
        if cfg.model:
            argv += ["--model", cfg.model]
        return argv


def _abs_rule(tool: str, path: Path, suffix: str = "") -> str:
    """A permission rule for an absolute path (Claude Code writes those as `//path`)."""
    return f"{tool}(/{Path(path).absolute()}{suffix})"


def pick(ctx: RepoContext, ledger: Ledger, task_id: str | None) -> dict[str, Any]:
    """The todo to hand over: the given one, or the oldest open one this agent can do (in
    the current idea when in an idea worktree)."""
    from .ideas import current_idea

    if task_id:
        task = tasks.get_task(ledger, task_id)
    else:
        here = current_idea(ctx, ledger)
        todo = [t for t in tasks.list_tasks(ledger, here["id"] if here else None)
                if t["idea_id"] is not None and t["kind"] in KINDS]
        if not todo:
            raise ArbError("no open todo for the background agent"
                           + (f" in idea {here['name']}" if here else "") + " (see `arb todo list`)")
        task = todo[0]
    if task["status"] != "open":
        raise ArbError(f"{task['id']} is {task['status']}, not open")
    if task["idea_id"] is None:
        raise ArbError(f"{task['id']} belongs to a seed; the background agent works only in idea worktrees")
    if task["kind"] not in KINDS:
        raise ArbError(f"{task['id']} is a {task['kind']} todo; the background agent does: {', '.join(KINDS)}")
    return task


def plan(ctx: RepoContext, ledger: Ledger, task: dict[str, Any]) -> Plan:
    from .approvekeys import APP_PUBLIC_KEY, keys_path

    idea = ledger.one("SELECT * FROM ideas WHERE id = ?", (task["idea_id"],))
    if idea is None or idea["status"] != "open":
        raise ArbError(f"{task['id']}: its idea is not open")
    cwd = Path(idea["worktree_path"])
    if not cwd.is_dir():
        raise ArbError(f"{task['id']}: the worktree of idea {idea['name']} is missing ({cwd})")
    tid, kind = task["id"], task["kind"]
    payload = tasks.payload(task)
    path = ""
    add_dirs: list[str] = []
    allowed = [f"Bash({c}:*)" for c in READ_COMMANDS]
    if kind == "plain":
        allowed.append(f"Bash(arb todo done {tid}:*)")
    elif kind == "inspect":
        allowed += [f"Bash(arb inspect {c}:*)" for c in ("guide",)]
        allowed += [f"Bash(arb inspect {c} {tid}:*)" for c in ("source", "check", "done")]
    else:
        target = Path(str(payload.get("path") or ""))
        dirs = [commands_mod.project_dir(ctx.state_dir).absolute(), commands_mod.global_dir().absolute()]
        if not target.is_absolute() or target.suffix != ".py" or target.absolute().parent not in dirs:
            raise ArbError(f"{tid}: the command file {target} is not in a commands folder")
        target.parent.mkdir(parents=True, exist_ok=True)
        path = str(target.absolute())
        add_dirs.append(str(target.parent.absolute()))
        allowed += ["Bash(arb commands guide:*)", _abs_rule("Edit", target),
                    f"Bash(arb todo done {tid}:*)"]
    state = ctx.state_dir.absolute()
    protected = [state / "notebooks", cwd.absolute() / "notebook.py",
                 keys_path().expanduser().absolute(), APP_PUBLIC_KEY.expanduser().parent]
    settings = {
        "permissions": {"deny": [_abs_rule("Edit", p, "/**" if p.name == "notebooks" else "")
                                 for p in protected]},
        "sandbox": {
            "enabled": True,
            "autoAllowBashIfSandboxed": False,
            "allowUnsandboxedCommands": False,
            "filesystem": {"allowWrite": [str(state), str(ctx.common_dir.absolute())],
                           "denyWrite": [str(p) for p in protected]},
        },
    }
    prompt = PROMPT_HEAD.format(idea=idea["name"], id=tid, kind=kind, text=task["text"])
    prompt += PROMPT_KIND[kind].format(id=tid, path=path)
    if payload and kind != "plain":
        prompt += "\n待办的 payload：" + json.dumps(payload, ensure_ascii=False) + "\n"
    prompt += "\n可以用的 arb 命令：" + "、".join(f"`{a[5:-3]}`" for a in allowed if a.startswith("Bash(")) + "\n"
    return Plan(task=task, idea=idea, cwd=cwd, prompt=prompt, allowed=allowed,
                settings=settings, add_dirs=add_dirs)


# ---- limits ------------------------------------------------------------------------------

def _ts(text: str) -> datetime:
    return datetime.strptime(text, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


def history(ledger: Ledger, limit: int = 20) -> list[dict[str, Any]]:
    rows = ledger.all("SELECT ts, subject, payload_json FROM events WHERE kind = ? "
                      "ORDER BY id DESC LIMIT ?", (EVENT, limit))
    out = []
    for r in rows:
        try:
            p = json.loads(r["payload_json"] or "{}")
        except ValueError:
            p = {}
        out.append({"ts": r["ts"], "task": r["subject"], **(p if isinstance(p, dict) else {})})
    return out


def check_limits(ledger: Ledger, cfg: AgentConfig, at: datetime | None = None) -> None:
    at = at or datetime.now(timezone.utc)
    since = (at - timedelta(hours=24)).strftime("%Y-%m-%dT%H:%M:%SZ")
    row = ledger.one("SELECT COUNT(*) AS n FROM events WHERE kind = ? AND ts > ?", (EVENT, since))
    if (row["n"] if row else 0) >= cfg.daily_limit:
        raise ArbError(f"the background agent already ran {cfg.daily_limit} times in the last 24 hours "
                       "(`arboria.toml [agent] daily_limit`)")
    for h in history(ledger):
        w = (h.get("limits") or {}).get(WINDOW)
        if not isinstance(w, dict):
            continue
        util, resets = w.get("utilization"), w.get("resets_at")
        if isinstance(util, (int, float)) and util >= cfg.pause_above and isinstance(resets, int) \
                and resets > at.timestamp():
            when = datetime.fromtimestamp(resets).strftime("%H:%M")
            raise ArbError(f"paused: the subscription's {WINDOW} window is at {util:.0%} "
                           f"(pause_above {cfg.pause_above:.0%}); it resets at {when}")
        break  # only the latest reading counts


class _Lock:
    def __init__(self, state_dir: Path) -> None:
        self.path = Path(state_dir) / LOCK_FILE
        self.fh = None

    def __enter__(self) -> "_Lock":
        self.fh = open(self.path, "a")
        try:
            fcntl.flock(self.fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.fh.close()
            raise ArbError("the background agent is already running (one at a time)") from None
        return self

    def __exit__(self, *exc: Any) -> None:
        fcntl.flock(self.fh, fcntl.LOCK_UN)
        self.fh.close()


# ---- running ------------------------------------------------------------------------------

def claude_bin() -> str:
    exe = os.environ.get(CLAUDE_ENV) or shutil.which("claude")
    if not exe:
        raise ArbError("`claude` is not on PATH (install Claude Code, or set ARBORIA_CLAUDE)")
    return exe


def clean_env(env: dict[str, str] | None = None) -> dict[str, str]:
    """The environment for `claude`: without what a surrounding Claude Code session set
    (CLAUDECODE, CLAUDE_CODE_*, CLAUDE_AGENT_SDK_*, …; an inherited CLAUDE_CODE_CHILD_SESSION
    turns transcripts off, so `--resume` would not work). The user's CLAUDE_CONFIG_DIR stays."""
    env = dict(os.environ if env is None else env)
    return {k: v for k, v in env.items() if not k.startswith("CLAUDE") or k in KEEP_ENV}


@dataclass
class Tally:
    session: str = ""
    model: str = ""
    limits: dict[str, dict[str, Any]] = field(default_factory=dict)
    result: dict[str, Any] = field(default_factory=dict)

    def feed(self, line: str) -> None:
        try:
            d = json.loads(line)
        except ValueError:
            return
        if not isinstance(d, dict):
            return
        kind = d.get("type")
        if kind == "system" and d.get("subtype") == "init":
            self.session = str(d.get("session_id") or "")[:80]
            self.model = str(d.get("model") or "")[:80]
        elif kind == "rate_limit_event":
            info = d.get("rate_limit_info") or {}
            for name, w in (info.get("unifiedWindows") or {}).items():
                if isinstance(w, dict) and name in (WINDOW, "seven_day"):
                    util, resets = w.get("utilization"), w.get("resetsAt")
                    self.limits[name] = {
                        "utilization": float(util) if isinstance(util, (int, float)) else None,
                        "resets_at": int(resets) if isinstance(resets, (int, float)) else None}
        elif kind == "result":
            self.result = d

    def over(self, cfg: AgentConfig) -> bool:
        util = (self.limits.get(WINDOW) or {}).get("utilization")
        return isinstance(util, float) and util >= cfg.pause_above

    def summary(self) -> dict[str, Any]:
        r, u = self.result, self.result.get("usage") or {}
        tokens = {k: int(u[k]) for k in ("input_tokens", "output_tokens", "cache_read_input_tokens",
                                         "cache_creation_input_tokens") if isinstance(u.get(k), int)}
        cost = r.get("total_cost_usd")
        return {"session": self.session, "model": self.model, "limits": self.limits,
                "turns": r.get("num_turns") if isinstance(r.get("num_turns"), int) else None,
                "tokens": tokens,
                "list_cost_usd": round(cost, 4) if isinstance(cost, (int, float)) else None,
                "denials": len(r.get("permission_denials") or []),
                "ended": str(r.get("subtype") or "")[:40]}


def _stream(proc: subprocess.Popen, deadline: float) -> Iterator[str]:
    sel = selectors.DefaultSelector()
    sel.register(proc.stdout, selectors.EVENT_READ)
    buf = b""
    while True:
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError
        if not sel.select(timeout=min(left, 1.0)):
            if proc.poll() is not None:
                break
            continue
        chunk = os.read(proc.stdout.fileno(), 65536)
        if not chunk:
            break
        buf += chunk
        *lines, buf = buf.split(b"\n")
        for line in lines:
            yield line.decode("utf-8", "replace")
    if buf:
        yield buf.decode("utf-8", "replace")


def run(ctx: RepoContext, cfg: AgentConfig, task_id: str | None = None,
        echo=lambda s: None) -> dict[str, Any]:
    """Hand one todo to `claude -p` and record the outcome. Refuses inside Claude Code."""
    human.require_not_claude("arb agent run")
    with _Lock(ctx.state_dir):
        with Ledger(ctx.state_dir) as ledger:
            task = pick(ctx, ledger, task_id)
            check_limits(ledger, cfg)
            p = plan(ctx, ledger, task)
        argv = p.argv(claude_bin(), cfg)
        log_dir = ctx.state_dir / LOG_DIR
        log_dir.mkdir(exist_ok=True)
        stamp = now().replace(":", "").replace("-", "")
        log = log_dir / f"{task['id']}-{stamp}.jsonl"
        tally, status, started = Tally(), "done", time.monotonic()
        echo(f"{task['id']}: claude -p in {p.cwd} (log {log})")
        proc = subprocess.Popen(argv, cwd=p.cwd, env=clean_env(), stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        try:
            with open(log, "w", encoding="utf-8") as fh:
                for line in _stream(proc, started + cfg.timeout_minutes * 60):
                    fh.write(line + "\n")
                    tally.feed(line)
                    if tally.over(cfg):
                        status = "paused"
                        break
        except TimeoutError:
            status = "timeout"
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(10)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
        if status == "done" and (proc.returncode != 0 or tally.result.get("is_error")):
            status = "failed"
        with Ledger(ctx.state_dir) as ledger:
            after = tasks.get_task(ledger, task["id"])["status"]
            out = {"task": task["id"], "kind": task["kind"], "idea": p.idea["name"],
                   "status": status, "todo": after, "exit": proc.returncode,
                   "seconds": round(time.monotonic() - started), "log": str(log),
                   **tally.summary()}
            with ledger.write():
                ledger.event(EVENT, task["id"], {k: v for k, v in out.items() if k != "task"})
    return out


def describe(r: dict[str, Any]) -> str:
    """One line for a recorded run."""
    t = r.get("tokens") or {}
    w = (r.get("limits") or {}).get(WINDOW) or {}
    util = w.get("utilization")
    bits = [f"{r.get('task')} {r.get('status')}", f"todo {r.get('todo')}"]
    if t:
        bits.append(f"{t.get('input_tokens', 0) + t.get('cache_creation_input_tokens', 0) + t.get('cache_read_input_tokens', 0)} in"
                    f" / {t.get('output_tokens', 0)} out tokens")
    if isinstance(util, (int, float)):
        bits.append(f"{WINDOW} {util:.0%}")
    if r.get("session"):
        bits.append(f"claude --resume {r['session']}")
    return " · ".join(bits)
