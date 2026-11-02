"""Runs, notes, decisions, status and export."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from arboria import records
from arboria.db import Ledger
from arboria.errors import ArbError

from .conftest import git


def _backup_log(main: Path) -> list[str]:
    return git(main / ".arboria", "log", "--format=%s").splitlines()


def test_run_new_ids_and_refs(idea: Path, initialized: Path, arb) -> None:
    assert arb(idea, "run", "new", "--reason", "baseline").stdout.strip() == "r001"
    (idea / "a.py").write_text("a\n")
    data = json.loads(arb(idea, "run", "new", "--reason", "add a", "--json").stdout)
    assert data["id"] == "r002" and data["status"] == "draft" and data["ref"] == "refs/arboria/runs/r002"
    assert git(initialized, "rev-parse", "refs/arboria/runs/r002") == data["snapshot_commit"]
    assert _backup_log(initialized)[0] == "arboria backup: run_new r002"
    sql = (initialized / ".arboria" / "backup" / "ledger.sql").read_text()
    assert "r002" in sql and "add a" in sql


def test_run_new_outside_idea_fails(initialized: Path, arb) -> None:
    res = arb(initialized, "run", "new", "--reason", "x", ok=False)
    assert res.exit_code == 1 and "not an idea worktree" in res.stderr


def test_run_reason_validated(idea: Path, arb) -> None:
    res = arb(idea, "run", "new", "--reason", "r" * 301, ok=False)
    assert "change_reason is too long" in res.stderr
    assert git(idea, "for-each-ref", "refs/arboria") == ""


def test_note_updates_confidence(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "run", "new", "--reason", "baseline")
    arb(idea, "note", "loss went down 3%", "--run", "r001", "--confidence", "prelim")
    arb(idea, "note", "second note")
    st = json.loads(arb(idea, "status", "--json").stdout)
    assert st["scope"] == "idea" and st["idea"]["confidence"] == "prelim"
    assert [n["text"] for n in st["notes"]] == ["second note", "loss went down 3%"]
    assert st["notes"][1]["run_id"] == "r001" and st["notes"][1]["confidence"] == "prelim"
    assert _backup_log(initialized)[0] == "arboria backup: note exp1"

    for args, msg in [(["n", "--run", "r999"], "no run"),
                      (["n", "--confidence", "maybe"], "invalid confidence"),
                      (["x" * 1001], "note is too long")]:
        res = arb(idea, "note", *args, ok=False)
        assert res.exit_code == 1 and msg in res.stderr
    res = arb(initialized, "note", "from main", ok=False)
    assert "not an idea worktree" in res.stderr


def test_note_run_of_other_idea_rejected(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "run", "new", "--reason", "baseline")
    other = Path(arb(initialized, "idea", "new", "exp2", "--hypothesis", "h", "--metric", "m").stdout.strip())
    res = arb(other, "note", "hi", "--run", "r001", ok=False)
    assert "belongs to another idea" in res.stderr


def test_status_idea_and_overview(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "run", "new", "--reason", "baseline")
    for i in range(7):
        arb(idea, "note", f"note {i}")
    st = json.loads(arb(idea, "status", "--json").stdout)
    assert st["idea"]["hypothesis"] == "warmup helps" and st["idea"]["metric"] == "val/loss"
    assert len(st["notes"]) == 5 and st["notes"][0]["text"] == "note 6"
    assert [r["id"] for r in st["runs"]] == ["r001"]
    text = arb(idea, "status").stdout
    assert "exp1" in text and "r001" in text and "note 6" in text and "note 1" not in text

    ov = json.loads(arb(initialized, "status", "--json").stdout)
    assert ov["scope"] == "overview"
    assert [i["name"] for i in ov["ideas"]] == ["exp1"]
    assert ov["ideas"][0]["run_count"] == 1 and ov["ideas"][0]["last_note"] == "note 6"
    brief = arb(initialized, "status", "--brief").stdout
    assert "1 open idea" in brief and "exp1" in brief


def test_decisions_show_in_status(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "run", "new", "--reason", "baseline")
    with Ledger(initialized / ".arboria") as ledger:
        d = records.open_decision(ledger, "run", "r001", "Submit r001 to the cluster?", ["yes", "no"])
        assert d["id"] == "d001"
    assert _backup_log(initialized)[0] == "arboria backup: decision d001"
    st = json.loads(arb(idea, "status", "--json").stdout)
    assert [x["id"] for x in st["open_decisions"]] == ["d001"]
    assert "Submit r001" in arb(idea, "status", "--brief").stdout
    with Ledger(initialized / ".arboria") as ledger:
        with pytest.raises(ArbError, match="invalid choice"):
            records.decide(ledger, "d001", "maybe", "human")
        records.decide(ledger, "d001", "yes", "human")
    assert json.loads(arb(idea, "status", "--json").stdout)["open_decisions"] == []


def test_run_summary(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "run", "new", "--reason", "baseline")
    arb(idea, "run", "summary", "r001", "val/loss 2.31 -> 2.25\nstable")
    res = arb(idea, "run", "summary", "r001", "\n".join(["l"] * 11), ok=False)
    assert "11 lines" in res.stderr
    md = arb(initialized, "export").stdout
    assert "> val/loss 2.31 -> 2.25" in md and "> stable" in md


def test_export_markdown(idea: Path, initialized: Path, arb, tmp_path: Path) -> None:
    arb(idea, "run", "new", "--reason", "baseline run")
    arb(idea, "note", "promising", "--confidence", "prelim")
    arb(initialized, "idea", "new", "exp2", "--hypothesis", "dropout hurts", "--metric", "acc")
    arb(initialized, "idea", "close", "exp1", "--drop")
    md = arb(initialized, "export").stdout
    assert md.startswith("# Arboria research log: proj")
    assert md.index("## exp2") < md.index("## exp1")  # newest first
    assert "## exp1 — dropped, confidence: prelim" in md
    assert "warmup helps" in md and "promising" in md and "baseline run" in md and "**r001**" in md
    out = tmp_path / "log.md"
    arb(initialized, "export", "--out", str(out))
    def body(s: str) -> list[str]:  # ignore the export timestamp line
        return [line for line in s.splitlines() if not line.startswith("_Exported")]

    assert body(out.read_text()) == body(md)
