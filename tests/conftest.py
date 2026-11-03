"""Shared fixtures: isolated git config and throw-away repositories."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest
from typer.testing import CliRunner

from arboria import human
from arboria.cli import app


def git(cwd: Path, *args: str, check: bool = True) -> str:
    proc = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and proc.returncode != 0:
        raise AssertionError(f"git {' '.join(args)} failed: {proc.stderr}")
    return proc.stdout.strip()


@pytest.fixture(autouse=True)
def isolated_git(tmp_path_factory: pytest.TempPathFactory, monkeypatch: pytest.MonkeyPatch) -> None:
    """Point git at a private global config with an identity and default branch `main`."""
    home = tmp_path_factory.mktemp("home")
    cfg = home / ".gitconfig"
    cfg.write_text(
        "[user]\n\tname = Test User\n\temail = test@example.com\n"
        "[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n"
    )
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(cfg))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    for var in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
        monkeypatch.delenv(var, raising=False)


@pytest.fixture(autouse=True)
def human_terminal(monkeypatch: pytest.MonkeyPatch) -> None:
    """Tests run as the human in a terminal: no Claude Code variables, stdin "is a TTY".

    (This suite itself may run inside Claude Code, which sets CLAUDECODE.) Tests of the
    refusals set the variables / patch `human.stdin_is_tty` back themselves."""
    for var in human.CLAUDE_ENV_VARS:
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setattr(human, "stdin_is_tty", lambda: True)


@pytest.fixture(autouse=True)
def no_real_claude_session(tmp_path_factory: pytest.TempPathFactory,
                           monkeypatch: pytest.MonkeyPatch) -> None:
    """Never reach a real Claude Code session or the user's config.

    This suite may run inside Claude Code, which exports its own inbox socket and token;
    the SessionStart hook (run in-process here) would record that socket and a notebook
    command todo would then be posted into the real session. Tests that exercise
    messaging use a fake Unix-socket server (tests/test_sessions.py)."""
    for var in ("CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN"):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(tmp_path_factory.mktemp("arboria-config")))


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    """A main worktree `proj` with one commit, a .gitignore and some files."""
    root = tmp_path / "proj"
    root.mkdir()
    git(root, "init", "-q", "-b", "main")
    (root / "train.py").write_text("print('train')\n")
    (root / "README.md").write_text("# proj\n")
    (root / ".gitignore").write_text("*.ckpt\noutputs/\n")
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "initial")
    return root.resolve()


class Arb:
    """Run `arb` in-process from a given directory."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch):
        self.runner = CliRunner()
        self.monkeypatch = monkeypatch

    def __call__(self, cwd: Path, *args: str, input: str | None = None, ok: bool = True):
        self.monkeypatch.chdir(cwd)
        if input is None and len(args) >= 2 and (
                args[0] == "approve" or args[:2] == ("lesson", "accept")):
            input = args[-1] + "\n"  # the human types the id to confirm
        result = self.runner.invoke(app, list(args), input=input, catch_exceptions=False)
        if ok and result.exit_code != 0:
            raise AssertionError(
                f"arb {' '.join(args)} exited {result.exit_code}\n"
                f"stdout: {result.stdout}\nstderr: {result.stderr}"
            )
        return result


@pytest.fixture
def arb(monkeypatch: pytest.MonkeyPatch) -> Arb:
    return Arb(monkeypatch)


@pytest.fixture
def initialized(repo: Path, arb: Arb) -> Path:
    arb(repo, "init")
    return repo


@pytest.fixture
def idea(initialized: Path, arb: Arb) -> Path:
    """An idea `exp1`; returns its worktree path."""
    res = arb(initialized, "idea", "new", "exp1", "--hypothesis", "warmup helps", "--metric", "val/loss")
    return Path(res.stdout.strip())


def snapshot_files(root: Path) -> dict[str, tuple[bytes, int]]:
    """Contents + mtime of every file under root except .git."""
    out = {}
    for p in sorted(root.rglob("*")):
        if ".git" in p.relative_to(root).parts or not p.is_file():
            continue
        out[str(p.relative_to(root))] = (p.read_bytes(), p.stat().st_mtime_ns)
    return out


os.environ.setdefault("COLUMNS", "200")
