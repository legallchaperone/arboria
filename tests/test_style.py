"""The Arboria fonts (Noto Sans SC for prose, Maple Mono CN for code): shipped in the
package, embedded in the stylesheet next to every notebook and in the dashboard's, never
in the backup repo."""

from __future__ import annotations

import ast
import os
import stat
from pathlib import Path

import pytest

from arboria import notebooks, style
from arboria.paths import Home

from .conftest import SMALL_FONT_FACE, git

REAL_FONT_FACE = style.font_face  # before the autouse `small_font` fixture patches it


def _nb(main: Path, name: str = "exp1") -> Path:
    return main / ".arboria" / "notebooks" / f"{name}.py"


def _cells(source: str) -> list[str]:
    tree = ast.parse(source)
    return [ast.get_source_segment(source, n, padded=True) for n in tree.body
            if isinstance(n, ast.FunctionDef)]


# ---------------------------------------------------------------- the fonts


def test_font_files_ship_in_the_package() -> None:
    for _, name, _ in style.FONT_FILES:
        data = (style.FONTS_DIR / name).read_bytes()
        assert data[:4] == b"wOF2" and len(data) > 3_000_000, name  # full CJK, not a subset
    for lic in style.LICENSE_FILES:
        assert "SIL OPEN FONT LICENSE Version 1.1" in lic.read_text(), lic
    assert "Reserved Font Name 'Source'" in style.LICENSE_FILES[0].read_text()
    notice = style.NOTICE_FILE.read_text()
    for name in ("Noto Sans SC 2.004", "Maple Mono CN v7.9", "OFL-NotoSansSC.txt",
                 "OFL-MapleMono.txt"):
        assert name in notice, name
    assert style.NOTEBOOK_CSS.is_file() and style.DASHBOARD_CSS.is_file()


def test_font_face_embeds_both_families_as_data_uris() -> None:
    css = REAL_FONT_FACE()
    assert css.count("@font-face") == 3 and css.count('src: url("data:font/woff2;base64,') == 3
    assert css.count('font-family: "Maple Mono CN"') == 2
    assert css.count('font-family: "Noto Sans SC"') == 1
    assert "font-weight: 100 900" in css  # the variable Noto Sans SC: every weight
    assert "font-weight: 400" in css and "font-weight: 700" in css
    assert "http" not in css  # nothing to fetch


def test_stylesheets_split_prose_and_code_with_fallbacks() -> None:
    for path in (style.NOTEBOOK_CSS, style.DASHBOARD_CSS):
        css = path.read_text()
        assert '"Noto Sans SC", "PingFang SC"' in css and "sans-serif;" in css, path
        assert '"Maple Mono CN", ui-monospace' in css and "monospace;" in css, path
        assert '"calt" 1, "cv01" 1' in css and "cv65" not in css, path
        assert "url(" not in css and "@import" not in css, path
    nb = style.NOTEBOOK_CSS.read_text()
    for var, stack in (("--marimo-text-font", "sans"), ("--marimo-heading-font", "sans"),
                       ("--font-sans", "sans"), ("--marimo-monospace-font", "mono")):
        assert f"{var}: var(--arboria-{stack})" in nb, var
    assert ".cm-content" in nb  # the code editor gets the code font and its features
    dash = style.DASHBOARD_CSS.read_text()
    assert "font-family: var(--arb-sans)" in dash and ".arb code.cmd" in dash
    assert ".arb .kpi .v, .arb td.num" in dash  # numbers in the code font


def test_ensure_writes_only_when_the_content_changes(tmp_path: Path) -> None:
    path = tmp_path / "arboria.css"
    assert style.ensure(path, style.NOTEBOOK_CSS)
    text = path.read_text()
    assert text.startswith(style.MARK) and SMALL_FONT_FACE in text
    assert text.endswith(style.NOTEBOOK_CSS.read_text())
    mtime = path.stat().st_mtime_ns
    assert not style.ensure(path, style.NOTEBOOK_CSS) and path.stat().st_mtime_ns == mtime
    path.write_text("/* stale */\n")
    assert style.ensure(path, style.NOTEBOOK_CSS) and path.read_text() == text


# ---------------------------------------------------------------- notebooks


def test_new_idea_notebook_names_the_stylesheet(idea: Path, initialized: Path) -> None:
    nb = _nb(initialized)
    assert 'app = marimo.App(width="medium", css_file="arboria.css")' in nb.read_text()
    css = nb.parent / "arboria.css"
    assert css.is_file() and SMALL_FONT_FACE in css.read_text()
    # marimo reads css_file next to the path it opened, the worktree's notebook.py link
    link = idea / "arboria.css"
    assert link.is_symlink() and link.resolve() == css.resolve()
    exclude = (initialized / ".git" / "info" / "exclude").read_text()
    assert exclude.count("\n/arboria.css\n") == 1
    assert git(idea, "status", "--porcelain", "--untracked-files=all") == ""


def test_marimo_finds_the_stylesheet_through_the_worktree_link(idea: Path) -> None:
    pytest.importorskip("marimo")
    from marimo._session.notebook import read_css_file

    css = read_css_file("arboria.css", filename=str(idea / "notebook.py"))
    assert css is not None and SMALL_FONT_FACE in css


def test_seed_notebook_names_the_stylesheet(arb, tmp_path: Path, isolated_home: Path) -> None:
    arb(tmp_path, "seed", "new", "aaa", "一个想法")
    h = Home(isolated_home)
    assert 'css_file="arboria.css"' in h.notebook("aaa").read_text()
    assert SMALL_FONT_FACE in (h.seeds_dir / "arboria.css").read_text()
    # the backup repo versions notebook copies only, never the stylesheet
    assert all(not f.endswith(".css") for f in git(h.state_dir, "ls-files").splitlines())


def test_fonts_never_land_in_the_backup_repo(idea: Path, initialized: Path, arb) -> None:
    state = initialized / ".arboria"
    arb(idea, "run", "new", "--reason", "backup please")
    files = git(state, "ls-files").splitlines()
    assert files == [".gitignore", "backup/ledger.sql", "notebooks/exp1.py"]
    assert "arboria.css" in git(state, "status", "--porcelain", "--ignored")


# ---------------------------------------------------------------- upgrading old notebooks

OLD = '''import marimo

__generated_with = "0.25.2"
app = marimo.App(width="medium")


@app.cell(hide_code=True)
def _(mo):
    mo.md(r"""
    # 旧的 notebook · marimo.App(width="medium")
    """)
    return


@app.cell
def _():
    import marimo as mo
    app = "not the app"
    return (mo,)


if __name__ == "__main__":
    app.run()
'''


@pytest.mark.parametrize("call, expected", [
    ('marimo.App(width="medium")', 'marimo.App(width="medium", css_file="arboria.css")'),
    ("marimo.App()", 'marimo.App(css_file="arboria.css")'),
    ('marimo.App(\n    width="medium",\n    app_title="看板 ✓",\n)',
     'marimo.App(\n    width="medium",\n    app_title="看板 ✓", css_file="arboria.css"\n)'),
    ('marimo.App(app_title="中文", width="full")',
     'marimo.App(app_title="中文", width="full", css_file="arboria.css")'),
])
def test_add_style_changes_only_the_app_call(call: str, expected: str) -> None:
    old = OLD.replace('marimo.App(width="medium")\n', call + "\n", 1)
    new, what = notebooks.add_style(old)
    assert what == "added"
    assert new == old.replace(call + "\n", expected + "\n", 1)
    assert _cells(new) == _cells(old)  # every cell byte-identical
    assert notebooks.add_style(new) == (new, "already")  # idempotent


def test_add_style_leaves_other_files_alone() -> None:
    own = OLD.replace('width="medium")', 'width="medium", css_file="mine.css")')
    assert notebooks.add_style(own) == (own, "own")
    for src in ("print('no app')\n", "def broken(:\n", OLD.replace("marimo.App", "App")):
        assert notebooks.add_style(src) == (src, "unknown")


def test_arb_idea_style_upgrades_old_idea_notebooks(idea: Path, initialized: Path, arb,
                                                     monkeypatch) -> None:
    nb = _nb(initialized)
    nb.write_text(nb.read_text().replace(', css_file="arboria.css"', ""))  # an older notebook
    os.chmod(nb, 0o640)
    before = nb.read_text()
    (nb.parent / "arboria.css").unlink()
    (idea / "arboria.css").unlink()
    (nb.parent / "own.py").write_text(OLD.replace('width="medium")', 'css_file="x.css")'))

    monkeypatch.setenv("CLAUDECODE", "1")  # it writes notebooks: the human's only
    res = arb(idea, "idea", "style", ok=False)
    assert "只由人" in res.stderr and 'css_file="arboria.css"' not in nb.read_text()
    monkeypatch.delenv("CLAUDECODE")

    out = arb(idea, "idea", "style").stdout
    assert "exp1: 加上了 Arboria 字体" in out and "own: 有自己的 css_file，没动" in out
    after = nb.read_text()
    assert after == before.replace('marimo.App(width="medium")',
                                   'marimo.App(width="medium", css_file="arboria.css")')
    assert _cells(after) == _cells(before)
    assert stat.S_IMODE(nb.stat().st_mode) == 0o640
    assert (nb.parent / "arboria.css").is_file() and (idea / "arboria.css").is_symlink()
    assert "exp1: 已经有了" in arb(initialized, "idea", "style").stdout  # idempotent
    assert nb.read_text() == after


def test_arb_idea_style_upgrades_seed_notebooks(arb, tmp_path: Path, isolated_home: Path) -> None:
    arb(tmp_path, "seed", "new", "aaa", "一个想法")
    h = Home(isolated_home)
    nb = h.notebook("aaa")
    nb.write_text(nb.read_text().replace(', css_file="arboria.css"', ""))
    (h.seeds_dir / "arboria.css").unlink()
    assert "aaa: 加上了 Arboria 字体" in arb(tmp_path, "idea", "style").stdout
    assert 'css_file="arboria.css"' in nb.read_text()
    assert (h.seeds_dir / "arboria.css").is_file()
    assert "aaa: 已经有了" in arb(tmp_path, "idea", "style").stdout


# ---------------------------------------------------------------- the dashboard


def test_dashboard_app_runs_next_to_a_stylesheet_with_the_font() -> None:
    from arboria.ui import APP, GARDEN

    for app in (APP, GARDEN):
        run = style.dashboard_app(app)
        assert run.is_symlink() and run.resolve() == app.resolve() and run != app
        css = (run.parent / "dashboard.css").read_text()
        assert SMALL_FONT_FACE in css and css.endswith(style.DASHBOARD_CSS.read_text())
    # the packaged stylesheet (also the standalone HTML's) stays free of the font
    assert "@font-face" not in style.DASHBOARD_CSS.read_text()


def test_dashboard_app_falls_back_to_the_package(tmp_path: Path, monkeypatch) -> None:
    from arboria.ui import APP

    blocker = tmp_path / "file"
    blocker.write_text("not a directory")
    monkeypatch.setenv("ARBORIA_CACHE_DIR", str(blocker))
    assert style.dashboard_app(APP) == APP
