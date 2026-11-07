"""The background agent (`arb agent run`): one `claude -p` per todo, run by a fake claude."""

from __future__ import annotations

import json
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

import pytest

from arboria import agent, human
from arboria.config import AgentConfig
from arboria.db import Ledger
from arboria.errors import ArbError
from arboria.paths import locate

FAKE = """#!{python}
import json, os, sys, time
out = os.environ["FAKE_OUT"]
json.dump({{"argv": sys.argv[1:], "cwd": os.getcwd(),
           "env": sorted(k for k in os.environ if k.startswith("CLAUDE"))}}, open(out, "w"))
mode = os.environ.get("FAKE_MODE", "ok")
def say(d):
    print(json.dumps(d), flush=True)
say({{"type": "system", "subtype": "init", "session_id": "sess-123", "model": "claude-haiku-5-5"}})
util = 0.9 if mode == "hot" else 0.2
say({{"type": "rate_limit_event", "rate_limit_info": {{"unifiedWindows": {{
    "five_hour": {{"utilization": util, "resetsAt": int(time.time()) + 3600}},
    "seven_day": {{"utilization": 0.5, "resetsAt": int(time.time()) + 86400}}}}}}}})
if mode in ("hot", "slow"):
    time.sleep(30)
say({{"type": "result", "subtype": "success", "is_error": mode == "error", "num_turns": 3,
     "total_cost_usd": 0.0123456, "permission_denials": [{{"tool_name": "Bash"}}],
     "usage": {{"input_tokens": 6, "output_tokens": 600, "cache_read_input_tokens": 16000,
               "cache_creation_input_tokens": 4000}}}})
"""


@pytest.fixture
def fake(tmp_path: Path, monkeypatch) -> Path:
    exe = tmp_path / "claude"
    exe.write_text(FAKE.format(python=sys.executable))
    exe.chmod(0o755)
    out = tmp_path / "fake.json"
    monkeypatch.setenv(agent.CLAUDE_ENV, str(exe))
    monkeypatch.setenv("FAKE_OUT", str(out))
    return out


def _todo(initialized: Path, text: str = "eval 用的是哪个 split？", **kw):
    from arboria import lab

    lab.use("exp1", repo=initialized)
    return lab.todo(text, **kw)


def _ctx(where: Path):
    return locate(where).require_initialized()


def test_plan_gives_only_what_this_todo_needs(idea: Path, initialized: Path) -> None:
    _todo(initialized)
    ctx = _ctx(idea)
    with Ledger(ctx.state_dir) as ledger:
        p = agent.plan(ctx, ledger, agent.pick(ctx, ledger, None))
    argv = p.argv("claude", AgentConfig(model="haiku", max_turns=7))
    assert p.cwd == idea and "t001" in p.prompt and "eval 用的是哪个 split" in p.prompt
    for flag in ("--strict-mcp-config", "--disable-slash-commands", "--setting-sources"):
        assert flag in argv
    assert argv[argv.index("--setting-sources") + 1] == ""
    assert argv[argv.index("--max-turns") + 1] == "7" and argv[argv.index("--model") + 1] == "haiku"
    assert argv[argv.index("--tools") + 1] == "Read,Grep,Glob,Bash"  # no Write/Edit
    assert "Bash(arb todo done t001:*)" in p.allowed
    assert not any("approve" in a or "submit" in a or "note" in a or "todo drop" in a for a in p.allowed)
    assert all(a.startswith("Bash(arb ") for a in p.allowed)
    box = p.settings["sandbox"]
    assert box["enabled"] and not box["autoAllowBashIfSandboxed"] and "excludedCommands" not in box
    state = str(initialized / ".arboria")
    assert state in box["filesystem"]["allowWrite"]
    assert f"{state}/notebooks" in box["filesystem"]["denyWrite"]
    assert str(idea / "notebook.py") in box["filesystem"]["denyWrite"]
    assert f"Edit(/{state}/notebooks/**)" in p.settings["permissions"]["deny"]
    assert json.loads(argv[argv.index("--settings") + 1]) == p.settings


def test_command_todo_may_write_only_its_file(idea: Path, initialized: Path) -> None:
    from arboria import lab

    lab.use("exp1", repo=initialized)
    lab.svd("r001", ask="画奇异值")  # no such command: a command todo
    ctx = _ctx(idea)
    with Ledger(ctx.state_dir) as ledger:
        task = agent.pick(ctx, ledger, None)
        p = agent.plan(ctx, ledger, task)
    target = Path(json.loads(task["payload_json"])["path"])
    assert task["kind"] == "command_new" and p.add_dirs == [str(target.parent)]
    assert f"Edit(/{target})" in p.allowed and "Bash(arb commands guide:*)" in p.allowed
    assert "Write,Edit" in p.argv("claude", AgentConfig())[p.argv("claude", AgentConfig()).index("--tools") + 1]


def test_run_records_usage_and_cleans_env(fake: Path, idea: Path, initialized: Path,
                                          monkeypatch, arb) -> None:
    _todo(initialized)
    monkeypatch.setenv("CLAUDE_CODE_CHILD_SESSION", "1")  # inherited from somewhere
    out = agent.run(_ctx(idea), AgentConfig())
    seen = json.loads(fake.read_text())
    assert seen["cwd"] == str(idea) and seen["env"] == []
    assert seen["argv"][0] == "-p"
    assert out["status"] == "done" and out["todo"] == "open" and out["session"] == "sess-123"
    assert out["tokens"]["output_tokens"] == 600 and out["denials"] == 1 and out["turns"] == 3
    assert out["limits"]["five_hour"]["utilization"] == 0.2 and out["list_cost_usd"] == 0.0123
    assert Path(out["log"]).is_file() and Path(out["log"]).parent == initialized / ".arboria" / "agent"
    listed = json.loads(arb(idea, "agent", "list", "--json").stdout)
    assert listed[0]["task"] == "t001" and listed[0]["status"] == "done"
    line = arb(idea, "agent", "list").stdout
    assert "t001 done" in line and "claude --resume sess-123" in line and "five_hour 20%" in line
    assert "agent" not in (initialized / ".arboria" / ".gitignore").read_text()  # not backed up


def test_high_window_stops_the_run_and_the_next(fake: Path, idea: Path, initialized: Path,
                                                monkeypatch) -> None:
    _todo(initialized)
    monkeypatch.setenv("FAKE_MODE", "hot")
    t0 = time.monotonic()
    out = agent.run(_ctx(idea), AgentConfig())
    assert out["status"] == "paused" and time.monotonic() - t0 < 20  # did not wait for it
    monkeypatch.setenv("FAKE_MODE", "ok")
    with pytest.raises(ArbError, match="paused: .*five_hour.*90%"):
        agent.run(_ctx(idea), AgentConfig())
    # a higher threshold lets it run
    assert agent.run(_ctx(idea), AgentConfig(pause_above=0.95))["status"] == "done"


def test_limits_timeout_and_errors(fake: Path, idea: Path, initialized: Path, monkeypatch) -> None:
    _todo(initialized)
    monkeypatch.setenv("FAKE_MODE", "error")
    assert agent.run(_ctx(idea), AgentConfig())["status"] == "failed"
    with pytest.raises(ArbError, match="already ran 1 times"):
        agent.run(_ctx(idea), AgentConfig(daily_limit=1))
    ctx = _ctx(idea)
    with Ledger(ctx.state_dir) as ledger:  # a day later it may run again
        agent.check_limits(ledger, AgentConfig(daily_limit=1),
                           at=datetime.now(timezone.utc).replace(year=2099))
    with agent._Lock(ctx.state_dir), pytest.raises(ArbError, match="already running"):
        agent.run(ctx, AgentConfig())
    monkeypatch.setattr(agent.time, "monotonic", _Clock())
    monkeypatch.setenv("FAKE_MODE", "slow")
    assert agent.run(ctx, AgentConfig(timeout_minutes=1))["status"] == "timeout"


class _Clock:
    """monotonic() that jumps 50 s per call, so a 1-minute timeout fires quickly."""

    def __init__(self) -> None:
        self.t = time.monotonic()

    def __call__(self) -> float:
        self.t += 50
        return self.t


def test_refusals(idea: Path, initialized: Path, monkeypatch, arb) -> None:
    ctx = _ctx(idea)
    with pytest.raises(ArbError, match="no open todo"):
        agent.run(ctx, AgentConfig())
    _todo(initialized)
    monkeypatch.setenv("CLAUDECODE", "1")
    with pytest.raises(human.NotHumanError):
        agent.run(ctx, AgentConfig())
    monkeypatch.delenv("CLAUDECODE")
    arb(idea, "todo", "drop", "t001")
    with Ledger(ctx.state_dir) as ledger, pytest.raises(ArbError, match="dropped, not open"):
        agent.pick(ctx, ledger, "t001")


def test_dry_run_and_config(idea: Path, initialized: Path, arb) -> None:
    _todo(initialized)
    (initialized / "arboria.toml").write_text(
        (initialized / "arboria.toml").read_text() + '\n[agent]\nmodel = "sonnet"\nmax_turns = 12\n')
    shown = json.loads(arb(idea, "agent", "run", "--dry-run", "--json").stdout)
    assert shown["task"] == "t001" and shown["argv"][shown["argv"].index("--model") + 1] == "sonnet"
    assert shown["argv"][shown["argv"].index("--max-turns") + 1] == "12"
    (initialized / "arboria.toml").write_text(
        (initialized / "arboria.toml").read_text() + "pause_above = 2\n")
    assert "pause_above" in arb(idea, "agent", "run", "--dry-run", ok=False).output
