"""The arboria Skill (`arb install-claude`) and `--json` on the commands Claude writes with."""

from __future__ import annotations

import json
import re
from pathlib import Path

import typer

from arboria import skill
from arboria.cli import app

from .test_tree import _metrics  # noqa: F401


def _commands() -> dict[str, set[str]]:
    """Visible command path -> its option names."""
    out: dict[str, set[str]] = {}

    def walk(group, prefix=""):
        for name, cmd in group.commands.items():
            if getattr(cmd, "hidden", False):
                continue
            if hasattr(cmd, "commands"):
                walk(cmd, f"{prefix}{name} ")
            else:
                out[prefix + name] = {o for p in cmd.params for o in getattr(p, "opts", [])}
    walk(typer.main.get_command(app))
    return out


def test_skill_names_only_real_commands_and_options() -> None:
    cmds = _commands()
    mentioned = re.findall(r"`(arb [^`]+)`", skill.SKILL)
    assert len(mentioned) > 20
    for text in mentioned:
        words = text.split()[1:]
        path = next((" ".join(words[:n]) for n in (2, 1) if " ".join(words[:n]) in cmds), None)
        assert path is not None, f"the Skill names a command that does not exist: {text}"
        for opt in re.findall(r"(?<![\w-])(--[a-z][a-z-]*|-n)\b", text):
            assert opt in cmds[path], f"`{text}`: {path} has no {opt}"
    # nothing human-only is offered as Claude's to run
    for human in ("arb approve RUN`", "arb lesson accept", "arb key", "arb note ", "arb milestone new"):
        line = next((l for l in skill.SKILL.splitlines() if human in l), "")
        assert not line.startswith("| `"), human


def test_skill_frontmatter() -> None:
    head = skill.SKILL.split("---")[1]
    assert re.search(r"^name: arboria$", head, re.M)
    desc = re.search(r"^description: (.+)$", head, re.M)
    assert desc and len(desc.group(1)) <= 1024


def test_install_writes_and_respects_edits(arb, idea: Path, initialized: Path) -> None:
    out = arb(idea, "install-claude").stdout
    p = skill.path(idea)
    assert f"wrote skill {p}" in out and p.read_text() == skill.SKILL
    assert "skill" not in arb(idea, "install-claude").stdout  # already current
    p.write_text(p.read_text() + "\nmy own line\n")
    assert f"updated skill {p}" in arb(idea, "install-claude").stdout  # still ours
    p.write_text("my skill now\n")
    assert "left" in arb(idea, "install-claude").stdout and p.read_text() == "my skill now\n"


def test_json_on_claudes_write_commands(arb, idea: Path, initialized: Path) -> None:
    from arboria import lab

    new = json.loads(arb(initialized, "idea", "new", "exp2", "--hypothesis", "h", "--metric", "m",
                         "--parent", "exp1", "--json").stdout)
    assert new["name"] == "exp2" and new["parent"] == "exp1" and Path(new["worktree"]).is_dir()
    arb(idea, "run", "new", "--reason", "baseline")
    assert json.loads(arb(idea, "run", "summary", "r001", "fine.", "--json").stdout) == \
        {"run": "r001", "summary": "saved"}
    lab.use("exp1", repo=initialized)
    lab.todo("a")
    lab.todo("b")
    assert json.loads(arb(idea, "todo", "done", "t001", "--result", "ok", "--json").stdout)["status"] == "done"
    assert json.loads(arb(idea, "todo", "drop", "t002", "--json").stdout)["status"] == "dropped"
    lesson = json.loads(arb(idea, "lesson", "propose", "check the seed", "--json").stdout)
    assert lesson["lesson"] == "l001" and lesson["status"] == "proposed"
