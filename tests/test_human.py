"""Approvals only in the human's own terminal; notes are human-only."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from arboria import human

from .test_submit import _decisions, _run, _submit, cluster  # noqa: F401  (fixture)

REFUSAL = "批准只能在你自己的终端里做：请在终端运行 arb approve r001"


@pytest.mark.parametrize("var", ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"])
def test_approve_refused_inside_claude_code(cluster, idea: Path, initialized: Path, arb,
                                            monkeypatch, var) -> None:
    _submit(arb, idea)
    monkeypatch.setenv(var, "1" if var == "CLAUDECODE" else "cli")
    res = arb(idea, "approve", "r001", ok=False)
    assert res.exit_code == 1 and REFUSAL in res.stderr
    assert "审核单" not in res.stdout  # refused before showing or asking anything
    assert _run(initialized)["status"] == "awaiting_approval"
    assert _decisions(initialized)[0]["choice"] is None
    assert cluster.calls() == []


def test_approve_refused_without_tty(cluster, idea: Path, initialized: Path, arb,
                                     monkeypatch) -> None:
    _submit(arb, idea)
    monkeypatch.setattr(human, "stdin_is_tty", lambda: False)
    res = arb(idea, "approve", "r001", ok=False)
    assert res.exit_code == 1 and REFUSAL in res.stderr
    assert _run(initialized)["status"] == "awaiting_approval" and cluster.calls() == []


@pytest.mark.parametrize("typed", ["", "r002", "yes", "R001"])
def test_approve_needs_the_typed_run_id(cluster, idea: Path, initialized: Path, arb,
                                        typed) -> None:
    _submit(arb, idea)
    res = arb(idea, "approve", "r001", input=typed + "\n", ok=False)
    assert res.exit_code == 1 and "不符，已取消" in res.stderr
    assert "审核单 r001" in res.stdout  # the human saw the card before typing
    assert "输入 r001 确认" in res.stderr
    assert _run(initialized)["status"] == "awaiting_approval"
    assert _decisions(initialized)[0]["choice"] is None and cluster.calls() == []
    res = arb(idea, "approve", "r001", input="", ok=False)  # end of input
    assert "nothing was changed" in res.stderr


def test_approval_records_provenance(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    res = arb(idea, "approve", "r001", input="r001\n")
    assert "approved r001 (decision d001)" in res.stdout
    [dec] = _decisions(initialized)
    assert (dec["choice"], dec["decided_by"]) == ("approve", "human")
    payload = json.loads(dec["payload_json"])
    assert payload["author"] == "human" and payload["pid"] == os.getpid()
    chain = payload["process_chain"]
    assert chain[0]["pid"] == os.getpid() and chain[0]["comm"]
    assert 1 <= len(chain) <= human.MAX_ANCESTORS + 1


def test_process_chain_is_best_effort(monkeypatch) -> None:
    chain = human.process_chain()
    assert chain[0]["pid"] == os.getpid()
    assert len(chain) <= human.MAX_ANCESTORS + 1
    monkeypatch.setattr(human, "_proc_parent", lambda pid: None)
    assert human.process_chain() == [{"pid": os.getpid(), "comm": None}]

    def boom(pid):
        raise RuntimeError("x")

    monkeypatch.setattr(human, "_proc_parent", boom)
    assert human.process_chain() == []  # never breaks an approval


def test_lesson_accept_refused_inside_claude_code(initialized: Path, arb, monkeypatch) -> None:
    arb(initialized, "lesson", "propose", "eval 每 500 步一次")
    monkeypatch.setenv("CLAUDECODE", "1")
    res = arb(initialized, "lesson", "accept", "l001", ok=False)
    assert "批准只能在你自己的终端里做：请在终端运行 arb lesson accept l001" in res.stderr
    monkeypatch.delenv("CLAUDECODE")
    monkeypatch.setattr(human, "stdin_is_tty", lambda: False)
    assert "请在终端运行 arb lesson accept l001" in arb(initialized, "lesson", "accept", "l001",
                                                         ok=False).stderr
    monkeypatch.setattr(human, "stdin_is_tty", lambda: True)
    res = arb(initialized, "lesson", "accept", "l001", input="l002\n", ok=False)
    assert "不符" in res.stderr and "l001: eval 每 500 步一次" in res.stdout
    lessons = json.loads(arb(initialized, "lesson", "list", "--json").stdout)
    assert lessons[0]["status"] == "proposed"
    arb(initialized, "lesson", "accept", "l001", input="l001\n")
    from arboria.db import Ledger

    with Ledger(initialized / ".arboria") as l:
        dec = l.one("SELECT * FROM decisions WHERE subject_type = 'lesson'")
    assert dec["choice"] == "accept" and json.loads(dec["payload_json"])["author"] == "human"


def test_note_refused_inside_claude_code(idea: Path, initialized: Path, arb,
                                         monkeypatch) -> None:
    monkeypatch.setenv("CLAUDECODE", "1")
    res = arb(idea, "note", "looks good", ok=False)
    assert res.exit_code == 1
    assert "arb todo done tNNN --result" in res.stderr and "arb run summary" in res.stderr
    # run summaries stay writable for Claude
    arb(idea, "run", "new", "--reason", "baseline")
    arb(idea, "run", "summary", "r001", "val/loss 2.3")
    monkeypatch.delenv("CLAUDECODE")
    arb(idea, "note", "human note")
    from arboria.db import Ledger

    with Ledger(initialized / ".arboria") as l:
        assert l.all("SELECT text, author FROM notes") == [{"text": "human note",
                                                           "author": "human"}]


def test_guard_still_the_only_submit_path_after_gate() -> None:
    """The approval gate sits in front of the unchanged _guarded_submit invariant."""
    src = (Path(__file__).parent.parent / "src" / "arboria" / "submission.py").read_text()
    body = src[src.index("def approve("):src.index("# ---------------------------------------------------------------- reject")]
    assert body.index("human.require_terminal") < body.index("human.confirm_typed") \
        < body.index('records.set_decision(ledger, dec_id, "approve", "human", prov)') \
        < body.index("submit_segments(")
