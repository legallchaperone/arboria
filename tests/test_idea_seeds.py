"""Seeds under `arb idea`: where the command acts is fixed by the current directory.

Outside any git repo (or in the seed home): the seeds. Inside a repo without `arb init`:
the usual error, never a seed. Inside an initialized repo: the repo's ideas, as before.
Every seed write is the human's and refuses inside Claude Code, under both spellings.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from arboria.db import Ledger
from arboria.paths import Home

from .conftest import git


def _seeds(home: Path) -> dict[str, str]:
    if not (home / ".arboria" / "ledger.db").exists():
        return {}
    with Ledger(home / ".arboria") as ledger:
        return {r["name"]: r["status"] for r in ledger.all("SELECT name, status FROM seeds")}


# ---------------------------------------------------------------- routing


def test_outside_a_repo_idea_commands_work_on_seeds(arb, tmp_path: Path,
                                                    isolated_home: Path) -> None:
    h = Home(isolated_home)
    res = arb(tmp_path, "idea", "new", "rank-schedule", "LoRA 的秩随层变化",
              "--metric", "同参数量下的 val/loss")
    nb = h.notebook("rank-schedule")
    assert res.stdout.strip() == str(nb) and nb.is_file()
    assert "记成了种子" in res.stderr and "arb idea plant rank-schedule" in res.stderr
    assert "seeds/rank-schedule.py" in res.stderr
    assert _seeds(isolated_home) == {"rank-schedule": "thinking"}
    # list / show / set / drop / revive, as `arb seed …` did
    assert "rank-schedule" in arb(tmp_path, "idea", "list").stdout
    shown = json.loads(arb(tmp_path, "idea", "show", "rank-schedule", "--json").stdout)
    assert (shown["gist"], shown["metric"]) == ("LoRA 的秩随层变化", "同参数量下的 val/loss")
    arb(tmp_path, "idea", "set", "rank-schedule", "--hypothesis", "浅层要更高的秩")
    assert "假设：浅层要更高的秩" in arb(tmp_path, "idea", "show", "rank-schedule").stdout
    assert "dropped rank-schedule" in arb(tmp_path, "idea", "drop", "rank-schedule").stdout
    assert "rank-schedule" not in arb(tmp_path, "idea", "list").stdout
    assert "rank-schedule" in arb(tmp_path, "idea", "list", "--all").stdout
    arb(tmp_path, "idea", "revive", "rank-schedule")
    assert _seeds(isolated_home) == {"rank-schedule": "thinking"}
    # repo-only options say so
    assert "a seed has no parent" in arb(tmp_path, "idea", "new", "x", "--parent", "y",
                                         ok=False).stderr
    assert "not seeds" in arb(tmp_path, "idea", "drop", "rank-schedule", "--confidence",
                              "prelim", ok=False).stderr
    assert "not inside a git repository" in arb(tmp_path, "idea", "close", "--drop",
                                                ok=False).stderr


def test_inside_the_home_idea_commands_work_on_seeds(arb, tmp_path: Path,
                                                     isolated_home: Path) -> None:
    arb(tmp_path, "idea", "new", "aaa", "第一个")
    h = Home(isolated_home)
    arb(h.seeds_dir, "idea", "new", "bbb", "第二个")
    assert set(_seeds(isolated_home)) == {"aaa", "bbb"}
    assert "bbb" in arb(h.root, "idea", "list").stdout


def test_an_uninitialized_repo_never_gets_a_seed(arb, repo: Path, isolated_home: Path) -> None:
    for args in (["new", "x", "一句话"], ["new", "x", "--hypothesis", "h", "--metric", "m"],
                 ["list"], ["show", "x"], ["drop", "x"], ["set", "x", "--gist", "g"],
                 ["revive", "x"]):
        res = arb(repo, "idea", *args, ok=False)
        assert res.exit_code == 1 and "run `arb init` there first" in res.stderr, args
    assert not isolated_home.exists() and not (repo / ".arboria").exists()


def test_inside_a_repo_idea_commands_are_as_before(arb, initialized: Path, tmp_path: Path,
                                                   isolated_home: Path) -> None:
    res = arb(initialized, "idea", "new", "exp1", "--hypothesis", "warmup helps",
              "--metric", "val/loss")
    wt = Path(res.stdout.strip())
    assert wt == initialized.parent / "proj-exp1" and wt.is_dir()
    assert "notebook:" in res.stderr and "种子" not in res.stderr
    assert not isolated_home.exists()  # no seed home touched
    assert "missing --metric" in arb(initialized, "idea", "new", "e2", "--hypothesis", "h",
                                     ok=False).stderr
    assert "a one-line gist is for seeds" in arb(initialized, "idea", "new", "e2", "gist",
                                                 ok=False).stderr
    # show: the idea and its runs (from the worktree, by default the current idea)
    run = arb(wt, "run", "new", "--reason", "baseline").stdout.strip()
    out = arb(wt, "idea", "show").stdout
    assert out.startswith("exp1 · open · guess") and "假设：warmup helps" in out
    assert "分支：idea/exp1" in out and run in out and "baseline" in out
    data = json.loads(arb(initialized, "idea", "show", "exp1", "--json").stdout)
    assert data["name"] == "exp1" and [r["id"] for r in data["runs"]] == [run]
    # seed-only verbs point outside the repo
    for verb in ("set", "revive"):
        args = [verb, "exp1"] + (["--gist", "g"] if verb == "set" else [])
        err = arb(initialized, "idea", *args, ok=False).stderr
        assert f"arb idea {verb} 只用于种子" in err and f"arb seed {verb} exp1" in err
    # drop = close --drop
    out = arb(initialized, "idea", "drop", "exp1").stdout
    assert "dropped exp1: final snapshot" in out and "arboria/archive/exp1" in out
    assert git(initialized, "tag", "--list") == "arboria/archive/exp1" and not wt.exists()
    assert json.loads(arb(initialized, "idea", "list", "--json").stdout)[0]["status"] == "dropped"
    assert not isolated_home.exists()


def test_plant_from_inside_the_repo(arb, initialized: Path, tmp_path: Path,
                                    isolated_home: Path) -> None:
    arb(tmp_path, "idea", "new", "rank-schedule", "LoRA 的秩随层变化",
        "--hypothesis", "浅层要更高的秩", "--metric", "val/loss")
    res = arb(initialized, "idea", "plant", "rank-schedule")
    assert Path(res.stdout.strip()) == initialized.parent / "proj-rank-schedule"
    assert _seeds(isolated_home) == {"rank-schedule": "planted"}
    assert "rank-schedule" in arb(initialized, "idea", "list").stdout
    assert "which repo?" in arb(Home(isolated_home).root, "idea", "plant", "rank-schedule",
                                "--again", ok=False).stderr


def test_seed_alias_still_works_and_is_hidden(arb, tmp_path: Path, initialized: Path,
                                              isolated_home: Path) -> None:
    arb(initialized, "seed", "new", "aaa", "从仓库里也能写种子")  # the alias works anywhere
    assert "aaa" in arb(tmp_path, "idea", "list").stdout
    assert "aaa" in arb(initialized, "seed", "list").stdout
    assert " seed " not in arb(tmp_path, "--help").stdout
    out = arb(tmp_path, "idea", "--help").stdout
    for verb in ("new", "list", "show", "set", "drop", "revive", "plant", "close", "notebook"):
        assert f" {verb} " in out, verb


# ---------------------------------------------------------------- safety


SEED_WRITES = [["new", "x", "一句话"], ["set", "aaa", "--gist", "g"], ["drop", "aaa"],
               ["revive", "aaa"]]


@pytest.mark.parametrize("var", ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"])
def test_seed_route_refuses_inside_claude_code(arb, tmp_path: Path, initialized: Path,
                                               isolated_home: Path, monkeypatch, var) -> None:
    arb(tmp_path, "idea", "new", "aaa", "第一个", "--hypothesis", "h", "--metric", "m")
    before = _seeds(isolated_home)
    monkeypatch.setenv(var, "1")
    for args in SEED_WRITES:
        res = arb(tmp_path, "idea", *args, ok=False)
        assert res.exit_code == 1 and "只由人在自己的终端里做" in res.stderr, args
        assert f"arb idea {args[0]}" in res.stderr, args
    for args in SEED_WRITES:  # the alias as well
        res = arb(tmp_path, "seed", *args, ok=False)
        assert "只由人在自己的终端里做" in res.stderr and f"arb seed {args[0]}" in res.stderr
    res = arb(initialized, "idea", "plant", "aaa", ok=False)
    assert "只由人在自己的终端里做" in res.stderr and "arb idea plant aaa" in res.stderr
    assert _seeds(isolated_home) == before
    assert not (initialized.parent / "proj-aaa").exists()
    # reading seeds stays allowed
    assert "aaa" in arb(tmp_path, "idea", "list").stdout
    assert "第一个" in arb(tmp_path, "idea", "show", "aaa").stdout


def test_seed_new_inside_claude_code_does_not_create_the_home(arb, tmp_path: Path,
                                                              isolated_home: Path,
                                                              monkeypatch) -> None:
    monkeypatch.setenv("CLAUDECODE", "1")
    assert "只由人" in arb(tmp_path, "idea", "new", "x", ok=False).stderr
    assert not isolated_home.exists()


def test_repo_route_inside_claude_code_is_as_before(arb, initialized: Path,
                                                    isolated_home: Path, monkeypatch) -> None:
    monkeypatch.setenv("CLAUDECODE", "1")
    res = arb(initialized, "idea", "new", "exp1", "--hypothesis", "h", "--metric", "m")
    assert Path(res.stdout.strip()).is_dir()
    assert "exp1" in arb(initialized, "idea", "list").stdout
    assert "exp1 · open" in arb(initialized, "idea", "show", "exp1").stdout
    assert "dropped exp1" in arb(initialized, "idea", "drop", "exp1").stdout
    assert not isolated_home.exists()
