"""The real marimo (dev dependency): template check, headless runs, auto re-run."""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

pytest.importorskip("marimo")

SRC = Path(__file__).parent.parent / "src"
HELPER = Path(__file__).parent / "marimo_session.py"

CELLS = '''

@app.cell
def _(lab):
    t = lab.resolve()
    r = lab.get("r001")
    r
    return (r,)


@app.cell
def _(lab, r):
    lab.svd(r, ask="对 attention 权重做 SVD，画前 20 个奇异值")
    return


@app.cell
def _(lab, r):
    lab.hello(r, "say hi")
    return


@app.cell
def _(lab):
    lab.commands()
    return


@app.cell
def _(lab):
    lab.hello
    return


if __name__ == "__main__":'''

HELLO = '''\
def hello(ctx, run, note=None):
    """Say hello to a run."""
    r = ctx.run(run)
    return ctx.mo.md(f"hello **{r.id}** of {ctx.idea.name} ({note})")
'''


def _env() -> dict[str, str]:
    env = dict(os.environ)  # the fixtures already removed Claude Code's variables
    env["PYTHONPATH"] = str(SRC) + os.pathsep + env.get("PYTHONPATH", "")
    return env


def _marimo(*args: str, cwd: Path, timeout: int = 180) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, "-m", "marimo", *args], cwd=cwd, env=_env(),
                          capture_output=True, text=True, timeout=timeout)


def _decoded(html: str) -> str:
    return re.sub(r"\\u([0-9a-fA-F]{4})", lambda m: chr(int(m.group(1), 16)), html)


def _notebook(idea: Path, initialized: Path, arb) -> Path:
    arb(idea, "run", "new", "--reason", "baseline")
    nb = idea / "notebook.py"
    text = nb.read_text()
    nb.write_text(text.replace('\n\nif __name__ == "__main__":', CELLS))
    return nb


def test_template_passes_marimo_check(idea: Path) -> None:
    res = _marimo("check", "--strict", "notebook.py", cwd=idea)
    assert res.returncode == 0, res.stdout + res.stderr
    assert "warning" not in (res.stdout + res.stderr).lower()


def test_headless_run_resolves_idea_and_renders(idea: Path, initialized: Path, arb) -> None:
    nb = _notebook(idea, initialized, arb)
    cmds = initialized / ".arboria" / "commands"
    cmds.mkdir()
    (cmds / "hello.py").write_text(HELLO)
    out = idea.parent / "out.html"
    res = _marimo("export", "html", str(nb), "-o", str(out), cwd=idea)
    assert res.returncode == 0, res.stdout + res.stderr
    html = _decoded(out.read_text())
    # lab.resolve() found the idea from the notebook; the Run handle rendered
    assert "<strong>r001</strong> · 草稿 · baseline" in html
    # the missing command became a todo and a placeholder (no exception)
    assert "命令 svd 还不存在，已交给 agent（t001）。" in html
    # the existing command ran, with its provenance underneath
    assert "hello <strong>r001</strong> of exp1 (say hi)" in html
    assert re.search(r"命令 hello · v[0-9a-f]{8} · 项目", html)
    # the command table and a command's help render as HTML
    assert "arb-commands" in html and "plot(runs, key, *, log=False)" in html
    assert "arb-help" in html and "lab.hello(run, note=None)" in html
    todos = arb(idea, "todo", "list").stdout
    assert "t001" in todos and "[command_new]" in todos


def test_cell_reruns_when_claude_writes_the_command(idea: Path, initialized: Path, arb) -> None:
    """A real edit-mode kernel: the placeholder cell re-runs once the file appears, and
    again when the command is revised."""
    nb = _notebook(idea, initialized, arb)
    cmd = initialized / ".arboria" / "commands" / "svd.py"
    (initialized / ".arboria" / "commands").mkdir()
    (initialized / ".arboria" / "commands" / "hello.py").write_text(HELLO)
    v1 = 'def svd(ctx, run, note=None):\n    """v1"""\n    return ctx.mo.md("SVD-V1 " + ctx.run(run).id)\n'
    v2 = v1.replace("V1", "V2")
    try:
        res = subprocess.run([sys.executable, str(HELPER), str(nb), str(cmd),
                              v1, "SVD-V1 r001", v2, "SVD-V2 r001"],
                             cwd=idea, env=_env(), capture_output=True, text=True, timeout=180)
    except subprocess.TimeoutExpired as exc:
        pytest.fail(f"marimo session did not finish: {exc}")
    if res.returncode == 77:
        pytest.skip(res.stdout.strip())
    assert res.returncode == 0, res.stdout + res.stderr
    assert "RERUN-OK SVD-V1 r001" in res.stdout and "RERUN-OK SVD-V2 r001" in res.stdout


INSPECT_CELLS = '''

@app.cell
def _(lab):
    lab.inspect("训练循环")
    return


if __name__ == "__main__":'''


def test_cell_reruns_when_the_agent_delivers_an_inspection(idea: Path, initialized: Path,
                                                            arb) -> None:
    """A real edit-mode kernel: `lab.inspect` leaves a todo; `arb inspect done` from a
    terminal makes the cell re-run and show the answer, code extracted by arb."""
    import json

    nb = idea / "notebook.py"
    nb.write_text(nb.read_text().replace('\n\nif __name__ == "__main__":', INSPECT_CELLS))
    answer = idea.parent / "answer.json"
    answer.write_text(json.dumps({"title": "ANSWER-TITLE", "spans": [
        {"path": "train.py", "start": 1, "end": 1, "line_notes": {"1": "NOTE-1"}}]}))
    deliver = json.dumps([sys.executable, "-c", "from arboria.cli import app; app()",
                          "inspect", "done", "t001", "--file", str(answer)])
    try:
        res = subprocess.run([sys.executable, str(HELPER), str(nb), "--exec",
                              deliver, "NOTE-1"],
                             cwd=idea, env=_env(), capture_output=True, text=True, timeout=180)
    except subprocess.TimeoutExpired as exc:
        pytest.fail(f"marimo session did not finish: {exc}")
    if res.returncode == 77:
        pytest.skip(res.stdout.strip())
    assert res.returncode == 0, res.stdout + res.stderr
    assert "RERUN-OK NOTE-1" in res.stdout
    todos = arb(idea, "todo", "list", "--all").stdout
    assert "t001" in todos and "ANSWER-TITLE" in todos


# ---------------------------------------------------------------- seeds

SEED_CELLS = '''

@app.cell(hide_code=True)
def _(mo):
    mo.md(r"""
    ## 为什么

    浅层的低秩更新也许需要**更高的秩**。
    """)
    return


@app.cell
def _(mo):
    from arboria import lab

    t = lab.resolve()
    try:
        lab.run(reason="x")
    except Exception as exc:
        msg = str(exc)
    mo.md(f"SEED={t.seed} · {msg}")
    return


@app.cell
def _():
    from arboria import lab as arb_lab

    first = arb_lab.todo("找三篇 <相关> 论文")
    first
    return arb_lab, first


@app.cell
def _(arb_lab, first):
    assert first.id == "t001"
    arb_lab.todos()
    return


if __name__ == "__main__":'''


def _seed(arb, tmp_path: Path, isolated_home: Path) -> Path:
    arb(tmp_path, "seed", "new", "rank-schedule", "LoRA 的秩随层变化",
        "--hypothesis", "浅层需要更高的秩", "--metric", "val/loss")
    return isolated_home / "seeds" / "rank-schedule.py"


def test_seed_template_passes_marimo_check(arb, tmp_path: Path, isolated_home: Path) -> None:
    nb = _seed(arb, tmp_path, isolated_home)
    res = _marimo("check", "--strict", str(nb), cwd=tmp_path)
    assert res.returncode == 0, res.stdout + res.stderr
    assert "warning" not in (res.stdout + res.stderr).lower()


def test_seed_notebook_runs_headless(arb, tmp_path: Path, isolated_home: Path) -> None:
    nb = _seed(arb, tmp_path, isolated_home)
    nb.write_text(nb.read_text().replace('\n\nif __name__ == "__main__":', SEED_CELLS))
    out = tmp_path / "seed.html"
    res = _marimo("export", "html", str(nb), "-o", str(out), cwd=tmp_path)
    assert res.returncode == 0, res.stdout + res.stderr
    html = _decoded(out.read_text())
    assert "LoRA 的秩随层变化" in html and "更高的秩" in html
    assert "SEED=rank-schedule · 这是一个种子（rank-schedule），还没种进仓库" in html
    # lab.todo / lab.todos(): calm lines, escaped, no JSON tree
    assert "已交给 agent（t001）：</span>" in html and "找三篇 &lt;相关&gt; 论文" in html
    assert "<code" in html and "等 agent" in html and "payload_json" not in html


def test_planted_notebook_passes_marimo_check(arb, tmp_path: Path, isolated_home: Path,
                                             initialized: Path) -> None:
    nb = _seed(arb, tmp_path, isolated_home)
    nb.write_text(nb.read_text().replace('\n\nif __name__ == "__main__":', SEED_CELLS))
    arb(initialized, "seed", "plant", "rank-schedule")
    planted = initialized / ".arboria" / "notebooks" / "rank-schedule.py"
    assert "种子里的思路" in planted.read_text()
    res = _marimo("check", "--strict", str(planted), cwd=initialized)
    assert res.returncode == 0, res.stdout + res.stderr
    assert "warning" not in (res.stdout + res.stderr).lower()
    out = tmp_path / "planted.html"
    res = _marimo("export", "html", str(planted), "-o", str(out), cwd=initialized)
    assert res.returncode == 0, res.stdout + res.stderr
    html = _decoded(out.read_text())
    assert "来自种子" in html and "种子里的思路" in html and "更高的秩" in html
