"""The app notebook's kernel (`python -m arboria.kernel`, ADR 0005) and its structured
outputs (`arboria.output`), through a real subprocess."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from arboria import analysis, output

SRC = Path(__file__).resolve().parents[1] / "src"


class Kernel:
    def __init__(self, cwd: Path):
        env = {**os.environ, "PYTHONPATH": str(SRC)}
        self.p = subprocess.Popen([sys.executable, "-m", "arboria.kernel"], cwd=cwd, env=env,
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, bufsize=1)
        self.n = 0
        self.ready = self._read()
        assert self.ready["event"] == "ready", self.ready

    def _read(self, timeout: float = 30) -> dict[str, Any]:
        box: list[str] = []
        t = threading.Thread(target=lambda: box.append(self.p.stdout.readline()))
        t.start()
        t.join(timeout)
        assert box and box[0], f"no event (stderr: {self.p.stderr.read() if self.p.poll() is not None else '…'})"
        return json.loads(box[0])

    def send(self, code: str, file: str | None = None, block: str = "ab12") -> int:
        self.n += 1
        self.p.stdin.write(json.dumps({"id": self.n, "op": "exec", "block": block,
                                       "code": code, "file": file}) + "\n")
        self.p.stdin.flush()
        return self.n

    def collect(self, rid: int, timeout: float = 30) -> tuple[list[dict], dict]:
        events = []
        while True:
            e = self._read(timeout)
            assert e.get("id") == rid, e
            if e["event"] == "done":
                return events, e
            events.append(e)

    def run(self, code: str, file: str | None = None) -> tuple[list[dict], dict]:
        return self.collect(self.send(code, file))

    def close(self) -> None:
        self.p.kill()
        self.p.wait()


@pytest.fixture
def kernel(tmp_path: Path):
    k = Kernel(tmp_path)
    yield k
    k.close()


def _outputs(events: list[dict]) -> list[dict]:
    return [e["data"] for e in events if e["event"] == "output"]


def _stdout(events: list[dict]) -> str:
    return "".join(e["text"] for e in events if e["event"] == "stream" and e["name"] == "stdout")


def test_values_streams_names_and_errors(kernel: Kernel) -> None:
    assert kernel.ready["lab"] is True
    events, done = kernel.run("x = 1\nprint('hi')\nx + 1")
    assert _stdout(events) == "hi\n"
    assert _outputs(events) == [{"kind": "text", "text": "2"}]
    assert done["ok"] and done["defines"] == ["x"] and done["uses"] == []
    events, done = kernel.run("def f(a):\n    return a * y\nz = f(x)")
    assert done["defines"] == ["f", "z"] and done["uses"] == ["y", "x"], done
    events, done = kernel.run("1 / 0")
    assert not done["ok"]
    err = _outputs(events)[0]
    assert err["kind"] == "error" and err["ename"] == "ZeroDivisionError"
    assert any("1 / 0" in line for line in err["traceback"]), err
    assert not any("kernel.py" in line for line in err["traceback"]), err
    # the namespace survives errors; exit() does not end the kernel
    events, done = kernel.run("raise SystemExit(3)")
    assert not done["ok"] and "exit(3)" in _outputs(events)[0]["evalue"]
    events, done = kernel.run("x")
    assert _outputs(events) == [{"kind": "text", "text": "1"}]


def test_tables_and_display(kernel: Kernel) -> None:
    events, _ = kernel.run("display('a', [{'run': 'r001', 'loss': 2.5}, {'run': 'r002', 'loss': float('nan')}])")
    out = _outputs(events)
    assert out[0] == {"kind": "text", "text": "a"}
    assert out[1] == {"kind": "table", "columns": ["run", "loss"],
                      "rows": [["r001", 2.5], ["r002", "nan"]], "total": 2}


def test_the_protocol_channel_cannot_be_broken(kernel: Kernel) -> None:
    events, done = kernel.run("import os, subprocess\nos.write(1, b'raw fd 1\\n')\n"
                              "subprocess.run(['echo', 'from a child'])\nprint('x' * 3_000_000)")
    assert done["ok"]
    text = _stdout(events)
    assert len(text) <= output.MAX_TEXT + 100 and "后面的没显示" in text
    events, done = kernel.run("2 + 2")
    assert _outputs(events) == [{"kind": "text", "text": "4"}]


def test_interrupt_and_carry_on(kernel: Kernel) -> None:
    rid = kernel.send("import time\nkept = 5\ntime.sleep(30)")
    time.sleep(1.0)
    kernel.p.send_signal(signal.SIGINT)
    events, done = kernel.collect(rid, timeout=10)
    assert not done["ok"] and _outputs(events)[0]["ename"] == "KeyboardInterrupt"
    kernel.p.send_signal(signal.SIGINT)  # idle: ignored
    time.sleep(0.2)
    events, done = kernel.run("kept")
    assert _outputs(events) == [{"kind": "text", "text": "5"}]


def test_slash_commands(kernel: Kernel) -> None:
    from arboria import kernel as k

    assert k.parse_slash("/plot r001 r002 val/loss log=True n=3 note='a b'") == (
        "plot", ["r001", "r002", "val/loss"], {"log": True, "n": 3, "note": "a b"})
    with pytest.raises(SyntaxError):
        k.parse_slash("/plot a=1 b")
    with pytest.raises(SyntaxError):
        k.slash_lines("/plot r1\nx = 1")
    assert k.slash_lines("# 注释\n/runs\n\n/spend") == ["/runs", "/spend"]
    assert k.slash_lines("x = 1") is None
    events, done = kernel.run("/nope_not_a_thing extra")
    assert not done["ok"] or _outputs(events), events  # lab answers (help or pending)


def test_lab_knows_the_idea_from_an_md_notebook(idea: Path, initialized: Path) -> None:
    nb = initialized / ".arboria" / "notebooks" / "exp1.md"
    nb.write_text("---\narboria: 1\nidea: exp1\n---\n")
    k = Kernel(initialized.parent)  # not inside the repo: the notebook decides
    try:
        events, done = k.run("lab.resolve().idea", file=str(nb))
        assert done["ok"], events
        assert _outputs(events) == [{"kind": "text", "text": "exp1"}]
        events, done = k.run("lab.svd(3, ask='前 20 个奇异值')", file=str(nb))
        assert done["ok"], events
        pending = _outputs(events)[0]
        assert pending["kind"] == "pending" and pending["what"] == "command", pending
        assert pending["name"] == "svd" and pending["task"].startswith("t")
        assert "自动重跑" in pending["text"]
        events, done = k.run("/todos", file=str(nb))
        assert done["ok"] and _outputs(events), events
    finally:
        k.close()


def test_outputs_are_checked() -> None:
    with pytest.raises(ValueError):
        output.check({"kind": "html", "text": "<b>x</b>"})
    with pytest.raises(ValueError):
        output.check({"kind": "text", "text": "a", "html": "<b>"})
    assert output.check({"kind": "text", "text": "x" * (output.MAX_TEXT + 5)})["truncated"]
    t = output.table(["a"], [[i] for i in range(output.MAX_ROWS + 10)])
    assert len(t["rows"]) == output.MAX_ROWS and t["total"] == output.MAX_ROWS + 10
    p = analysis.Plot("val/loss", [analysis.Series("r001", [(0.0, 3.0), (10.0, 2.5)])])
    got = output.to_outputs(p)
    assert got == [{"kind": "plot", "y": "val/loss", "x": "step", "log": False,
                    "style": "line", "series": [{"name": "r001", "x": [0.0, 10.0],
                                                  "y": [3.0, 2.5]}]}]

    class Broken:
        def _arboria_(self):
            return {"kind": "image", "mime": "image/svg+xml", "data": ""}

    with pytest.raises(ValueError):
        output.to_outputs(Broken())


def test_matplotlib_figures_become_png(kernel: Kernel) -> None:
    pytest.importorskip("matplotlib")
    events, done = kernel.run("import matplotlib.pyplot as plt\nplt.plot([1, 2, 3])")
    assert done["ok"], events
    kinds = [o["kind"] for o in _outputs(events)]
    assert kinds[-1] == "image", kinds
