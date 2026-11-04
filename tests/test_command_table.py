"""The command library as the notebook's centre: signatures (ast only), `ask=`, the
command table, a command's help, `arb commands list` and `arb commands promote`."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from arboria import commands, lab
from arboria.db import Ledger
from arboria.errors import ArbError

from .conftest import git

SVD = '''\
def svd(ctx, run, *, k=20, log=False):
    """对 run 的权重做 SVD，画前 k 个奇异值。

    k：画几个；log：纵轴用对数。
    """
    return {"run": ctx.run(run).id, "k": k, "log": log}
'''

OLD_STYLE = '''\
def old(ctx, run, note=None, *, k=20):
    """旧写法：说明作为位置参数传进来。"""
    return {"run": run, "note": note, "k": k}
'''


@pytest.fixture(autouse=True)
def config_dir(tmp_path_factory, monkeypatch) -> Path:
    cfg = tmp_path_factory.mktemp("arbcfg")
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(cfg))
    lab.use(None)
    yield cfg
    lab.use(None)


def _write(folder: Path, name: str, text: str) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.py"
    path.write_text(text)
    return path


def _project(main: Path) -> Path:
    return main / ".arboria" / "commands"


def _tasks(main: Path) -> list[dict]:
    with Ledger(main / ".arboria") as l:
        return l.all("SELECT * FROM tasks ORDER BY id")


# ---------------------------------------------------------------- signatures (ast only)


def test_describe_reads_the_signature_without_running(tmp_path: Path) -> None:
    path = _write(tmp_path, "svd", SVD + "\nraise SystemExit('never run')\n")
    d = commands.describe(path, "svd")
    assert d.problem is None and d.summary == "对 run 的权重做 SVD，画前 k 个奇异值。"
    assert d.signature("svd") == "svd(run, *, k=20, log=False)"
    assert [p.kind for p in d.params] == ["positional", "keyword", "keyword"]
    assert d.accepts_positional(1) and not d.accepts_positional(2)

    _write(tmp_path, "many", 'def many(ctx, a, /, b=1, *rest, c, d="x" * 40, **kw):\n'
           '    """x"""\n')
    m = commands.describe(tmp_path / "many.py", "many")
    assert m.signature("many") == "many(a, b=1, *rest, c, d='x' * 40, **kw)"
    assert m.accepts_positional(9)
    _write(tmp_path, "long", 'def long(ctx, x="' + "y" * 50 + '"):\n    """x"""\n')
    assert commands.describe(tmp_path / "long.py", "long").signature("long") == \
        "long(x='yyyyyyyyyyyyyyyyyyyyyy…)"
    assert commands.format_signature("f", m.params, limit=20) == "f(a, b=1, *rest, …)"

    _write(tmp_path, "clash", 'def clash(ctx, run, ask=None):\n    """x"""\n')
    assert "'ask' is lab's own" in commands.describe(tmp_path / "clash.py", "clash").problem
    _write(tmp_path, "noctx", 'def noctx():\n    """x"""\n')
    assert "ctx as its first" in commands.describe(tmp_path / "noctx.py", "noctx").problem
    broken = commands.describe(_write(tmp_path, "broken", "def broken(:\n"), "broken")
    assert broken.params is None and broken.signature("broken") == "broken(…)"
    assert broken.accepts_positional(5)  # unknown: never drop anything


def test_fit_args_drops_only_a_trailing_string_that_does_not_fit(tmp_path: Path) -> None:
    d = commands.describe(_write(tmp_path, "svd", SVD), "svd")
    assert commands.fit_args(d, ("r001", "做 SVD")) == ("r001",)
    assert commands.fit_args(d, ("r001",)) == ("r001",)
    assert commands.fit_args(d, ("r001", 5)) == ("r001", 5)  # not a string: TypeError later
    old = commands.describe(_write(tmp_path, "old", OLD_STYLE), "old")
    assert commands.fit_args(old, ("r001", "做 SVD")) == ("r001", "做 SVD")


# ---------------------------------------------------------------- ask=


def test_ask_is_the_description_and_never_reaches_the_command(idea: Path, initialized: Path,
                                                              arb, monkeypatch) -> None:
    monkeypatch.chdir(idea)
    arb(idea, "run", "new", "--reason", "baseline")
    p = lab.svd("r001", k=5, ask="对 attention 权重做 SVD")
    assert isinstance(p, lab.Pending) and p.task_id == "t001"
    (task,) = _tasks(initialized)
    data = json.loads(task["payload_json"])
    assert data["description"] == "对 attention 权重做 SVD"
    assert data["args"] == ["'r001'"] and data["kwargs"] == {"k": "5"}  # no ask in the shape
    assert data["call"] == "lab.svd('r001', k=5)"
    assert task["text"].startswith("写 notebook 命令 svd（项目）：对 attention 权重做 SVD")
    with pytest.raises(ArbError, match="ask= 是一句说明"):
        lab.svd("r001", ask=3)

    _write(_project(initialized), "svd", SVD)
    assert lab.svd("r001", k=5, ask="对 attention 权重做 SVD") == \
        {"run": "r001", "k": 5, "log": False}
    # an older notebook's positional description is dropped when the signature has no room
    assert lab.svd("r001", "对 attention 权重做 SVD", k=3)["k"] == 3
    # ... and passed through when it has (commands written for the old convention)
    _write(_project(initialized), "old", OLD_STYLE)
    assert lab.old("r001", "说明")["note"] == "说明"
    assert lab.old("r001", ask="说明")["note"] is None


def test_scope_and_ask_are_reserved_kwargs() -> None:
    assert set(commands.RESERVED_KWARGS) == {"scope", "ask"}
    for name in ("plot", "diff", "spend", "commands", "runs"):
        with pytest.raises(ArbError, match="already part of lab"):
            commands.validate_name(name)
    assert set(commands.BUILTINS) <= commands.reserved_names()
    assert all(callable(getattr(lab, n)) for n in commands.BUILTINS)


# ---------------------------------------------------------------- the table and the help


def test_commands_table_lists_builtins_and_the_library(idea: Path, initialized: Path,
                                                       config_dir: Path) -> None:
    lab.use("exp1", repo=initialized)
    empty = lab.commands()
    assert [r["kind"] for r in empty] == ["builtin"] * len(commands.BUILTINS)
    assert "还没有自定义命令" in str(empty)

    _write(_project(initialized), "svd", SVD)
    _write(config_dir / "commands", "loss_curve",
           'def loss_curve(ctx, *runs, smooth=0):\n    """画 <loss> & 平滑。"""\n')
    _write(config_dir / "commands", "svd", 'def svd(ctx):\n    """全局的，被覆盖。"""\n')
    _write(_project(initialized), "nodoc", "def nodoc(ctx):\n    return 1\n")
    table = lab.commands()
    by = {r["name"]: r for r in table}
    assert [r["name"] for r in table][: len(commands.BUILTINS)] == list(commands.BUILTINS)
    assert [(r["name"], r["kind"]) for r in table][len(commands.BUILTINS):] == [
        ("nodoc", "project"), ("svd", "project"), ("loss_curve", "global")]
    assert by["plot"]["signature"] == "plot(runs, key, *, log=False)"
    assert by["run"]["short"].endswith("…)") and len(by["run"]["short"]) <= lab.SIG_CLIP
    assert by["run"]["signature"].startswith("run(*, reason, from_=None, set=None")
    assert by["svd"]["signature"] == "svd(run, *, k=20, log=False)"
    assert by["svd"]["version"] == commands._sha(_project(initialized) / "svd.py")[:8]
    assert table.data is table.items and len(table) == len(commands.BUILTINS) + 3

    text = str(table)
    lines = text.splitlines()
    assert lines[0].startswith("内置  runs(raw=False)")
    assert lines[1].startswith("      get(run_id)")  # the kind only where it changes
    assert any(line.startswith("项目  nodoc()") and "⚠ no docstring" in line for line in lines)
    svd_line = next(line for line in lines if "svd(run" in line)
    assert f"v{by['svd']['version']}" in svd_line and "对 run 的权重做 SVD" in svd_line
    assert lines[-1].startswith("全局  loss_curve(*runs, smooth=0)")
    assert "还没有自定义命令" not in text and repr(table) == text

    page = table._repr_html_()
    assert "画 &lt;loss&gt; &amp; 平滑。" in page and "<loss>" not in page
    assert page.count("<tr>") == len(table) and "内置" in page and "全局" in page
    assert page.count(">内置<") == 1  # group label shown once


def test_typing_a_command_shows_its_help(idea: Path, initialized: Path, config_dir: Path,
                                         tmp_path: Path, monkeypatch) -> None:
    lab.use("exp1", repo=initialized)
    assert str(lab.svd) == ('lab.svd 还不存在：调用 lab.svd(..., ask="想要什么")，'
                            "就会交给 Claude 写进命令库")
    assert "还不存在" in lab.svd._repr_html_()
    path = _write(_project(initialized), "svd", SVD.replace("SVD", "<b>SVD</b>"))
    version = commands._sha(path)[:8]
    assert repr(lab.svd) == (
        f"lab.svd(run, *, k=20, log=False)\n项目 · v{version} · {path}\n\n"
        "对 run 的权重做 <b>SVD</b>，画前 k 个奇异值。\n\nk：画几个；log：纵轴用对数。")
    page = lab.svd._repr_html_()
    assert "&lt;b&gt;SVD&lt;/b&gt;" in page and "<b>" not in page
    assert "lab.svd(run, *, k=20, log=False)" in page and f"v{version}" in page
    assert lab.svd.help()["signature"] == "svd(run, *, k=20, log=False)"
    assert lab.svd.info()["signature"] == "svd(run, *, k=20, log=False)"

    # outside any idea the help still answers from the global library
    lab.use(None)
    monkeypatch.chdir(tmp_path)
    _write(config_dir / "commands", "hello", 'def hello(ctx, who="you"):\n    """打招呼。"""\n')
    assert str(lab.hello).startswith("lab.hello(who='you')\n全局 · v")
    assert "还不存在" in str(lab.svd)


def test_the_help_never_runs_command_code(idea: Path, initialized: Path) -> None:
    lab.use("exp1", repo=initialized)
    flag = initialized / "ran.txt"
    _write(_project(initialized), "evil", f'open({str(flag)!r}, "w").write("x")\n'
           'def evil(ctx):\n    """看起来无害。"""\n')
    assert "看起来无害" in str(lab.evil) and "evil" in str(lab.commands())
    assert not flag.exists()


# ---------------------------------------------------------------- arb commands list / promote


def test_commands_list_shows_signatures(initialized: Path, config_dir: Path, arb) -> None:
    _write(_project(initialized), "svd", SVD)
    out = arb(initialized, "commands", "list").stdout
    line = next(line for line in out.splitlines() if line.startswith("svd"))
    assert "svd(run, *, k=20, log=False)" in line and "项目" in line and " v" in line
    assert "对 run 的权重做 SVD" in line
    assert "内置" in out and "plot" in out and "lab.commands()" in out
    data = json.loads(arb(initialized, "commands", "list", "--json").stdout)
    assert data[0]["signature"] == "svd(run, *, k=20, log=False)"
