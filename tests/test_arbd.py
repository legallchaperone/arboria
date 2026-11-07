"""arbd: the Mac app's core process (ADR 0004, P1)."""

from __future__ import annotations

import ast
import re
import importlib.util
import json
import socket
import threading
import urllib.request
from pathlib import Path

import pytest

from .test_submit import cluster  # noqa: F401  (fixture)

from arboria import arbd
from arboria.db import Ledger
from arboria.errors import ArbError

SRC = Path(arbd.__file__)


class FakeProc:
    def __init__(self) -> None:
        self.returncode: int | None = None
        self.terminated = False

    def poll(self) -> int | None:
        return self.returncode

    def terminate(self) -> None:
        self.terminated = True
        self.returncode = -15

    def wait(self, timeout: float | None = None) -> int:
        return self.returncode or 0

    def kill(self) -> None:
        self.returncode = -9


@pytest.fixture
def started(monkeypatch):
    calls: list[dict] = []

    def start(cmd, cwd, env):
        p = FakeProc()
        calls.append({"cmd": cmd, "cwd": cwd, "env": env, "proc": p})
        return p

    return calls, start


def _daemon(root: Path, start) -> arbd.Daemon:
    return arbd.Daemon(root, start=start, ready=lambda port, proc: None)


def test_project_lists_ideas_with_their_worktrees(idea: Path, initialized: Path, started) -> None:
    d = _daemon(idea, started[1])  # any worktree of the project will do
    p = d.project()
    assert p["root"] == str(initialized) and p["name"] == "proj"
    [i] = p["ideas"]
    assert i["name"] == "exp1" and i["status"] == "open"
    assert i["worktree"] == str(idea) and i["has_notebook"] is True
    assert i["hypothesis"] == "warmup helps" and i["run_count"] == 0


def test_notebook_open_starts_marimo_in_the_worktree(idea: Path, initialized: Path, started,
                                                     monkeypatch) -> None:
    calls, start = started
    monkeypatch.setenv("CLAUDECODE", "1")
    monkeypatch.setenv("CLAUDE_CODE_CHILD_SESSION", "x")
    d = _daemon(initialized, start)
    r = d.notebook_open("exp1")
    [c] = calls
    assert c["cwd"] == idea
    cmd = c["cmd"]
    assert cmd[1:4] == ["-m", "marimo", "edit"] and cmd[4] == str(idea / "notebook.py")
    assert "--headless" in cmd and cmd[cmd.index("--host") + 1] == "127.0.0.1"
    assert "--token-password" not in cmd  # the token is passed in a 0600 file, not argv
    tf = Path(cmd[cmd.index("--token-password-file") + 1])
    assert tf.stat().st_mode & 0o777 == 0o600
    port = int(cmd[cmd.index("--port") + 1])
    assert r["url"] == f"http://127.0.0.1:{port}/?access_token={tf.read_text()}"
    assert "CLAUDECODE" not in c["env"] and "CLAUDE_CODE_CHILD_SESSION" not in c["env"]
    # opening again reuses the running server
    assert d.notebook_open("exp1")["url"] == r["url"] and len(calls) == 1
    assert d.project()["ideas"][0]["notebook_running"] is True
    d.notebook_close("exp1")
    assert c["proc"].terminated and not tf.exists()


def test_least_recently_opened_notebook_is_stopped(initialized: Path, arb, started,
                                                   monkeypatch) -> None:
    calls, start = started
    monkeypatch.setattr(arbd, "MAX_NOTEBOOKS", 2)
    for n in ("a1", "a2", "a3"):
        arb(initialized, "idea", "new", n, "--hypothesis", "h", "--metric", "m")
    d = _daemon(initialized, start)
    d.notebook_open("a1"); d.notebook_open("a2"); d.notebook_open("a1"); d.notebook_open("a3")
    assert calls[1]["proc"].terminated  # a2 was the least recent
    assert not calls[0]["proc"].terminated and not calls[2]["proc"].terminated
    d.shutdown()
    assert all(c["proc"].terminated for c in calls)


def test_a_dead_server_is_restarted(idea: Path, initialized: Path, started) -> None:
    calls, start = started
    d = _daemon(initialized, start)
    d.notebook_open("exp1")
    calls[0]["proc"].returncode = 1
    d.notebook_open("exp1")
    assert len(calls) == 2


def test_closed_idea_has_no_notebook_to_open(idea: Path, initialized: Path, arb, started) -> None:
    arb(initialized, "idea", "drop", "exp1")
    d = _daemon(initialized, started[1])
    assert d.project()["ideas"][0]["worktree"] is None
    reply = d.handle({"id": 1, "method": "notebook.open", "params": {"idea": "exp1"}})
    assert "no worktree" in reply["error"]["message"]
    assert "no idea named" in d.handle(
        {"id": 2, "method": "notebook.open", "params": {"idea": "nope"}})["error"]["message"]


def test_failed_start_cleans_up(idea: Path, initialized: Path, started) -> None:
    calls, start = started

    def never_ready(port, proc):
        raise ArbError("marimo did not answer")

    d = arbd.Daemon(initialized, start=start, ready=never_ready)
    reply = d.handle({"id": 1, "method": "notebook.open", "params": {"idea": "exp1"}})
    assert "did not answer" in reply["error"]["message"]
    assert calls[0]["proc"].terminated


def test_inbox_lists_what_needs_the_human(idea: Path, initialized: Path, arb, started) -> None:
    from arboria.db import Ledger
    from arboria.paths import locate

    d = _daemon(initialized, started[1])
    empty = d.inbox()
    assert empty["needs_you"] == 0 and empty["items"] == [] and empty["todos"] == []
    with Ledger(locate(initialized).state_dir) as ledger, ledger.write():
        ledger.conn.execute("INSERT INTO lessons (id, text, created_at) VALUES"
                            " ('l001', '<b>先看数据</b>', '2026-11-06T10:00:00Z')")
    got = d.inbox()
    assert got["needs_you"] == 1
    [item] = got["items"]
    assert item["kind"] == "lesson" and item["text"] == "<b>先看数据</b>"  # escaped by the page
    assert item["command"] == "arb lesson accept l001"


def test_dispatch_errors(initialized: Path, started) -> None:
    d = _daemon(initialized, started[1])
    assert d.handle({"id": 7, "method": "hello"})["result"]["protocol"] == arbd.PROTOCOL
    assert "unknown method" in d.handle({"id": 1, "method": "approve"})["error"]["message"]
    assert "missing param" in d.handle({"id": 1, "method": "notebook.open"})["error"]["message"]


def test_needs_an_initialized_project(repo: Path) -> None:
    with pytest.raises(ArbError):
        arbd.Daemon(repo)


def test_socket_round_trip(initialized: Path, started, tmp_path_factory) -> None:
    sock = Path("/tmp") / f"arbd-test-{id(started)}.sock"
    d = _daemon(initialized, started[1])
    server = arbd._Server(str(sock), arbd._handler(d))
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    try:
        with socket.socket(socket.AF_UNIX) as c:
            c.connect(str(sock))
            f = c.makefile("rwb")
            f.write(b'{"id": 1, "method": "project"}\n{"id": 2, "method": "nope"}\nnot json\n')
            f.flush()
            replies = [json.loads(f.readline()) for _ in range(3)]
        assert replies[0]["result"]["ideas"] == []
        assert replies[1]["id"] == 2 and "unknown method" in replies[1]["error"]["message"]
        assert "bad request" in replies[2]["error"]["message"]
    finally:
        server.shutdown(); server.server_close(); sock.unlink(missing_ok=True)


def test_serve_refuses_a_socket_path_too_long_for_macos(initialized: Path, tmp_path) -> None:
    with pytest.raises(ArbError, match="too long"):
        arbd.serve(tmp_path / ("x" * 120), initialized, stop_on_stdin_eof=False)


# ------------------------------------------------------------ guards (no approval path)


def _imported_modules(path: Path) -> set[str]:
    mods: set[str] = set()
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, ast.ImportFrom):
            mods.add((node.module or "").split(".")[0] if node.level == 0 else (node.module or ""))
            mods.update(a.name for a in node.names)
        elif isinstance(node, ast.Import):
            mods.update(a.name for a in node.names)
    return mods


def test_arbd_approves_only_with_a_signature() -> None:
    mods = _imported_modules(SRC)
    for forbidden in ("human", "lessons", "monitor", "backends"):
        assert forbidden not in mods, forbidden
    lines = SRC.read_text().splitlines()
    start = next(i for i, l in enumerate(lines) if l.startswith("from __future__"))
    code = "\n".join(l for l in lines[start:] if not l.lstrip().startswith("#"))
    for word in ("require_terminal", "_guarded_submit", "sbatch", "Backend", "submission.approve(",
                 "_record_and_submit", "set_decision"):
        assert word not in code, word
    # approve/accept appear only as command text for the human, or as the signed path
    for m in re.finditer(r"approve|accept", code):
        around = code[max(0, m.start() - 12):m.end() + 7]
        assert (code[max(0, m.start() - 12):m.start()].endswith(("arb ", "arb lesson "))
                or "approve_signed" in around or "approve.signed" in around
                or code[m.start():].startswith("approvekeys")), around  # reads the trusted list
    tree = ast.parse(SRC.read_text())
    table = next(n.value for n in ast.walk(tree) if isinstance(n, ast.AnnAssign)
                 and isinstance(n.target, ast.Name) and n.target.id == "methods")
    keys = [k.value for k in table.keys]
    assert "project" in keys and "notebook.open" in keys
    risky = [k for k in keys if re.search(r"approv|accept|sign|submit|cancel|reject", k)]
    assert risky == ["approve.signed"]
    # ... and its handler goes through submission.approve_signed (which verifies)
    fn = next(n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name == "approve_signed")
    calls = {ast.unparse(c.func) for c in ast.walk(fn) if isinstance(c, ast.Call)}
    assert "submission.approve_signed" in calls


def test_signed_approval_end_to_end_on_fake_slurm(cluster, idea: Path, initialized: Path,
                                                   arb, monkeypatch) -> None:
    """P3 acceptance on FakeSlurm: pair a key (human, terminal), the app signs the card
    arbd hands it, arbd approves, FakeSlurm runs it, poll fetches out/, and the experiment
    page shows the curves and what changed since the parent."""
    import base64

    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec

    from arboria import approvekeys, human
    from .test_submit import _submit, jn

    key = ec.generate_private_key(ec.SECP256R1())  # stands in for the Secure Enclave
    pub = base64.b64encode(key.public_key().public_bytes(
        serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)).decode()
    fp = approvekeys.fingerprint(pub)
    monkeypatch.setattr(human, "read_confirmation", lambda prompt: fp)
    arb(initialized, "key", "trust", "--public-key", pub, "--label", "test Mac")
    assert oct(approvekeys.keys_path().stat().st_mode & 0o777) == "0o600"
    assert fp in arb(initialized, "key", "list").stdout

    metrics = "".join(json.dumps({"step": s, "val/loss": 3 - s / 100}) + "\n" for s in range(0, 50, 10))
    cluster.script(jn(cluster, "r001", 1), run_s=600, queue_s=0, billed=1.5,
                   out_files={"metrics.jsonl": metrics})
    _submit(arb, idea)
    d = arbd.Daemon(initialized)
    card = d.handle({"id": 1, "method": "card", "params": {"run": "r001"}})["result"]
    assert card["matches"] and "r001" in card["text"] and len(card["card_hash"]) == 64

    def sign(run: str, h: str, k=key) -> str:
        msg = approvekeys.message(card["project"], run, h)
        return base64.b64encode(k.sign(msg, ec.ECDSA(hashes.SHA256()))).decode()

    call = lambda params: d.handle({"id": 2, "method": "approve.signed", "params": params})
    # no signature, a wrong key, another hash or another run: nothing is approved
    stranger = ec.generate_private_key(ec.SECP256R1())
    for params in ({"run": "r001", "card_hash": card["card_hash"], "signature": ""},
                   {"run": "r001", "card_hash": card["card_hash"], "signature": sign("r001", card["card_hash"], stranger)},
                   {"run": "r001", "card_hash": "0" * 64, "signature": sign("r001", "0" * 64)},
                   {"run": "r001", "card_hash": card["card_hash"], "signature": sign("r002", card["card_hash"])}):
        assert "error" in call(params), params
    assert cluster.calls("submit") == []
    ok = call({"run": "r001", "card_hash": card["card_hash"], "signature": sign("r001", card["card_hash"])})
    assert ok["result"]["status"] == "queued", ok
    assert len(cluster.calls("submit")) == 1
    with Ledger(initialized / ".arboria") as ledger:
        dec = ledger.one("SELECT * FROM decisions WHERE subject_id = 'r001' AND choice = 'approve'")
    assert dec["decided_by"] == "human"
    assert f'"key": "{fp}"' in dec["payload_json"] and "Touch ID" in dec["payload_json"]
    # replaying the signature does nothing more
    assert "error" in call({"run": "r001", "card_hash": card["card_hash"],
                            "signature": sign("r001", card["card_hash"])})
    assert len(cluster.calls("submit")) == 1

    cluster.advance(700)
    arb(idea, "poll")
    page = d.handle({"id": 3, "method": "run", "params": {"run": "r001"}})["result"]
    assert page["run"]["status"] == "succeeded" and page["run"]["idea_metric"] == "val/loss"
    assert page["metrics"]["val/loss"][:2] == [[0.0, 3.0], [10.0, 2.9]]
    compute = d.handle({"id": 4, "method": "compute"})["result"]
    assert compute["used_su"] == 1.5 and compute["jobs"][0]["state"] == "completed"

    # a second run derived from it: the page diffs against the parent
    arb(idea, "submit", "--from", "r001", "--set", "lr=1e-3", "--reason", "higher lr")
    page2 = d.handle({"id": 5, "method": "run", "params": {"run": "r002"}})["result"]
    assert page2["base"] == "r001" and page2["base_metrics"]["val/loss"]
    assert any(row[1] == "lr" for row in page2["diff"]["rows"])
    runs = d.handle({"id": 6, "method": "runs", "params": {"idea": "exp1"}})["result"]["runs"]
    assert [r["id"] for r in runs] == ["r002", "r001"] and runs[0]["parent_run"] == "r001"
    assert d.handle({"id": 7, "method": "keys"})["result"] == {"trusted": [fp]}


def test_key_trust_is_human_only(monkeypatch, arb, initialized: Path) -> None:
    from arboria import human

    monkeypatch.setenv("CLAUDECODE", "1")
    res = arb(initialized, "key", "trust", "--public-key", "AAAA", ok=False)
    assert res.exit_code != 0
    monkeypatch.delenv("CLAUDECODE")
    monkeypatch.setattr(human, "stdin_is_tty", lambda: False)
    assert arb(initialized, "key", "trust", "--public-key", "AAAA", ok=False).exit_code != 0


# ------------------------------------------------------------ real marimo


@pytest.mark.skipif(importlib.util.find_spec("marimo") is None, reason="needs marimo")
def test_real_marimo_serves_the_notebook(idea: Path, initialized: Path) -> None:
    d = arbd.Daemon(initialized)
    try:
        url = d.notebook_open("exp1")["url"]
        jar = urllib.request.build_opener(urllib.request.HTTPCookieProcessor())
        with jar.open(url, timeout=10) as r:  # token -> session cookie -> the notebook
            assert r.status == 200 and "/auth/login" not in r.url
        port = int(url.split(":")[2].split("/")[0])
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=10) as r:
            assert "/auth/login" in r.url  # no token: the login page
    finally:
        d.shutdown()
