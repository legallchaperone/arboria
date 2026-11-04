"""`arb submit --from/--set`, [run] command, idempotent re-submits, wandb ids, config.yaml."""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

import pytest

from arboria import config, db, monitor, slurm, submission
from arboria.db import Ledger
from arboria.errors import ArbError
from arboria.paths import locate

from .conftest import git
from .test_submit import _bump, _ledger, _run, _submit, cluster, jn  # noqa: F401  (fixture)


def _set_toml(main: Path, old: str, new: str) -> None:
    toml = main / "arboria.toml"
    text = toml.read_text()
    assert old in text, old
    toml.write_text(text.replace(old, new))


def _req(main: Path, run_id: str) -> dict:
    return json.loads(_run(main, run_id)["request_json"])


def _subj(main: Path) -> list[str]:
    return git(main / ".arboria", "log", "--format=%s").splitlines()


# ---------------------------------------------------------------- [run] command


def test_submit_without_cmd_uses_run_command(cluster, idea: Path, initialized: Path, arb) -> None:
    out = arb(idea, "submit", "--reason", "x", "--gpus", "1", "--time", "1h",
              "--set", "lr=3e-4", "--set", "model.depth=12").stdout
    assert "命令 python train.py lr=3e-4 model.depth=12" in out
    req = _req(initialized, "r001")
    assert req["command"] == ["python", "train.py", "lr=3e-4", "model.depth=12"]
    assert (req["command_source"], req["base_command"], req["override_style"]) == \
        ("config", ["python", "train.py"], "hydra")
    assert req["overrides"] == [["lr", "3e-4"], ["model.depth", "12"]]

    _set_toml(initialized, 'override_style = "hydra"', 'override_style = "argparse"')
    out = arb(idea, "submit", "--reason", "x", "--gpus", "1", "--time", "1h",
              "--set", "lr=1e-3").stdout
    assert "命令 python train.py --lr 1e-3" in out
    res = arb(idea, "submit", "--reason", "x", "--gpus", "1", "--time", "1h",
              "--set", "+trainer.devices=2", ok=False)
    assert "hydra syntax" in res.stderr
    for bad in ("nokey", "=1", "a b=1"):
        res = arb(idea, "submit", "--reason", "x", "--gpus", "1", "--time", "1h", "--set", bad,
                  ok=False)
        assert res.exit_code == 1 and ("invalid --set" in res.stderr
                                       or "invalid override key" in res.stderr), bad


def test_overrides_pure() -> None:
    assert slurm.render_overrides("hydra", [("lr", "1"), ("+x", "y")]) == ["lr=1", "+x=y"]
    assert slurm.render_overrides("tyro", [("lr", "1")]) == ["--lr", "1"]
    assert slurm.check_override("flag", True) == ("flag", "true")
    assert slurm.check_override("x", None) == ("x", "null")
    assert slurm.check_override("lr", 1e-3) == ("lr", "0.001")
    with pytest.raises(ArbError, match="single line"):
        slurm.check_override("x", "a\nb")
    with pytest.raises(ArbError, match="string or number"):
        slurm.check_override("x", [1, 2])
    assert slurm.merge_overrides([("a", "1"), ("b", "2")], [("b", "3"), ("c", "4")]) == \
        [("a", "1"), ("b", "3"), ("c", "4")]


@pytest.mark.parametrize("body,msg", [
    ("[run]\ncommand = 3\n", "command must be a string"),
    ("[run]\ncommand = 'python \"x'\n", "cannot be split"),
    ("[run]\noverride_style = 'click'\n", "override_style must be one of"),
    ("[wandb]\nentity = 'a/b'\n", "wandb name"),
    ("wandb = 1\n", r"\[wandb\] must be a table"),
])
def test_run_and_wandb_config_validation(repo: Path, body: str, msg: str) -> None:
    (repo / "arboria.toml").write_text(body)
    with pytest.raises(ArbError, match=msg):
        config.project_config(locate(repo))


def test_old_toml_without_new_tables(repo: Path) -> None:
    (repo / "arboria.toml").write_text('default_machine = ""\n')
    cfg = config.project_config(locate(repo))
    assert cfg.run.command == () and cfg.run.override_style == "hydra"
    assert not cfg.wandb.configured and cfg.wandb.run_url("x") is None


# ---------------------------------------------------------------- --from


def test_submit_from_reuses_snapshot_and_resources(cluster, idea: Path, initialized: Path,
                                                   arb) -> None:
    arb(idea, "submit", "--reason", "base", "--gpus", "2", "--time", "6h",
        "--partition", "ghx4-interactive", "--set", "lr=3e-4", "--set", "seed=1")
    parent = _run(initialized, "r001")
    (idea / "train.py").write_text("print('changed after r001')\n")  # not picked up by --from
    n_snap = git(initialized, "rev-list", "--all", "--count")
    out = arb(idea, "submit", "--from", "r001", "--set", "lr=1e-3", "--reason", "lower lr").stdout
    child = _run(initialized, "r002")
    assert child["snapshot_commit"] == parent["snapshot_commit"]
    assert child["tree_hash"] == parent["tree_hash"] and child["parent_run"] == "r001"
    assert git(initialized, "rev-list", "--all", "--count") == n_snap  # no new snapshot
    assert git(idea, "rev-parse", "refs/arboria/runs/r002") == parent["snapshot_commit"]
    req = _req(initialized, "r002")
    assert req["command"] == ["python", "train.py", "lr=1e-3", "seed=1"]  # lr replaced in place
    assert (req["gpus"], req["walltime_s"], req["partition"]) == (2, 6 * 3600, "ghx4-interactive")
    assert req["parent_run"] == "r001"
    assert "复用 r001 的快照" in out and "命令 python train.py lr=1e-3 seed=1" in out
    # explicit flags override the parent's
    arb(idea, "submit", "--from", "r002", "--gpus", "4", "--time", "1h", "--partition", "ghx4",
        "--reason", "more gpus")
    req = _req(initialized, "r003")
    assert (req["gpus"], req["walltime_s"], req["partition"]) == (4, 3600, "ghx4")
    assert req["command"] == ["python", "train.py", "lr=1e-3", "seed=1"]
    assert _subj(initialized)[0] == "arboria backup: decision d003; run_submit r003"


def test_submit_from_explicit_parent_command(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)  # -- python train.py --lr 3e-4
    _set_toml(initialized, 'command = "python train.py"', 'command = "python other.py"')
    arb(idea, "submit", "--from", "r001", "--set", "seed=2", "--reason", "seed")
    req = _req(initialized, "r002")
    assert req["command"] == ["python", "train.py", "--lr", "3e-4", "seed=2"]
    assert req["command_source"] == "explicit"
    # a parent built from [run] command takes the current [run] command
    arb(idea, "submit", "--reason", "cfg", "--gpus", "1", "--time", "1h", "--set", "a=1")
    arb(idea, "submit", "--from", "r003", "--set", "b=2", "--reason", "cfg child")
    assert _req(initialized, "r004")["command"] == ["python", "other.py", "a=1", "b=2"]
    # `-- CMD` with --from replaces the command (overrides start fresh)
    arb(idea, "submit", "--from", "r003", "--reason", "own cmd", "--", "python", "eval.py")
    assert _req(initialized, "r005")["command"] == ["python", "eval.py"]


def test_submit_from_refusals(cluster, idea: Path, initialized: Path, arb) -> None:
    arb(idea, "run", "new", "--reason", "draft")
    res = arb(idea, "submit", "--from", "r001", "--reason", "x", ok=False)
    assert "--gpus and --time are required" in res.stderr and "never submitted" in res.stderr
    out = arb(idea, "submit", "--from", "r001", "--gpus", "1", "--time", "1h", "--reason", "x").stdout
    assert "复用 r001 的快照" in out and _run(initialized, "r002")["parent_run"] == "r001"
    assert "no run 'r404'" in arb(idea, "submit", "--from", "r404", "--reason", "x",
                                  ok=False).stderr
    other = Path(arb(initialized, "idea", "new", "exp2", "--hypothesis", "h", "--metric", "m")
                 .stdout.strip())
    res = arb(other, "submit", "--from", "r002", "--reason", "x", ok=False)
    assert "belongs to another idea" in res.stderr and "lab.fork" in res.stderr


# ---------------------------------------------------------------- idempotency


def test_identical_submit_returns_existing_run(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    n_backups = len(_subj(initialized))
    res = _submit(arb, idea)
    assert res.stdout.startswith("已存在：r001\n") and "审核单 r001" in res.stdout
    assert _run(initialized, "r002") is None and len(_subj(initialized)) == n_backups
    with _ledger(initialized) as l:
        assert l.one("SELECT COUNT(*) AS n FROM decisions")["n"] == 1
    # the change reason is not part of the identity; resources and command are
    _submit(arb, idea)
    arb(idea, "submit", "--reason", "other", "--gpus", "4", "--time", "12h",
        "--", "python", "train.py", "--lr", "3e-4")
    assert _run(initialized, "r002") is None
    arb(idea, "submit", "--reason", "x", "--gpus", "2", "--time", "12h",
        "--", "python", "train.py", "--lr", "3e-4")
    assert _run(initialized, "r002")["gpus"] == 2
    # --from with nothing new: the parent itself
    res = arb(idea, "submit", "--from", "r001", "--reason", "same again")
    assert "已存在：r001" in res.stdout


def test_existing_run_after_approval_and_after_failure(cluster, idea: Path, initialized: Path,
                                                       arb) -> None:
    _submit(arb, idea)
    arb(idea, "approve", "r001")
    res = _submit(arb, idea)
    assert res.stdout.splitlines() == ["已存在：r001", "状态：queued"]
    assert len(cluster.calls("submit")) == 1
    arb(idea, "cancel", "r001")
    assert "r002" in _submit(arb, idea).stdout  # cancelled runs are not reused
    arb(idea, "reject", "r002")
    assert "r003" in _submit(arb, idea).stdout  # nor rejected ones
    data = json.loads(_submit(arb, idea, "--json").stdout)
    assert data["existed"] and data["run"]["id"] == "r003"


# ---------------------------------------------------------------- wandb


def test_wandb_id_exported_and_url_recorded(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    assert _run(initialized)["wandb_url"] is None  # not configured
    _set_toml(initialized, 'entity = ""', 'entity = "my-team"')
    _set_toml(initialized, 'project = ""', 'project = "lm"')
    _bump(idea)
    _submit(arb, idea)
    pid = cluster.project
    assert _run(initialized, "r002")["wandb_url"] == f"https://wandb.ai/my-team/lm/runs/{pid}-r002"
    arb(idea, "approve", "r002")
    script = cluster.jobs()["1001"]["script"]
    assert f"export ARBORIA_WANDB_ID={pid}-r002\n" in script
    assert slurm.wandb_id(pid, "r002") == f"{pid}-r002"
    st = json.loads(arb(idea, "status", "--json").stdout)
    assert st["runs"][1]["wandb_url"].endswith(f"{pid}-r002")


# ---------------------------------------------------------------- config.yaml


def _finish(cluster, arb, idea: Path, out_files: dict) -> None:
    cluster.script(jn(cluster, "r001", 1), run_s=60, out_files=out_files)
    _submit(arb, idea)
    arb(idea, "approve", "r001")
    cluster.advance(120)


def test_poll_stores_config_yaml(cluster, idea: Path, initialized: Path, arb, monkeypatch) -> None:
    monkeypatch.setattr(monitor, "_load_yaml", lambda text: {"lr": 0.0003, "model": {"d": 12}})
    _finish(cluster, arb, idea, {"config.yaml": "lr: 0.0003\nmodel:\n  d: 12\n"})
    out = arb(idea, "poll").stdout
    assert "saved config.yaml into the ledger" in out
    assert json.loads(_run(initialized)["config_json"]) == {"lr": 0.0003, "model": {"d": 12}}
    assert _subj(initialized)[0] == "arboria backup: run_config r001"


def test_config_yaml_without_pyyaml_and_too_big(tmp_path: Path, monkeypatch) -> None:
    def no_yaml(text):
        raise ImportError("no yaml")

    monkeypatch.setattr(monitor, "_load_yaml", no_yaml)
    p = tmp_path / "config.yaml"
    assert monitor.config_json_from(p) is None
    p.write_text("lr: 1\n")
    assert json.loads(monitor.config_json_from(p)) == {"_raw_yaml": "lr: 1\n"}
    monkeypatch.setattr(monitor, "_load_yaml", lambda text: {"lr": 1})
    assert json.loads(monitor.config_json_from(p)) == {"lr": 1}
    p.write_text("x" * (monitor.CONFIG_MAX_BYTES + 10))
    data = json.loads(monitor.config_json_from(p))
    assert data["_truncated"] and len(data["_raw_yaml"]) == monitor.CONFIG_MAX_BYTES
    assert data["_bytes"] == monitor.CONFIG_MAX_BYTES + 10


def test_poll_without_config_yaml_leaves_config_json(cluster, idea: Path, initialized: Path,
                                                     arb) -> None:
    _finish(cluster, arb, idea, {"metrics.json": "{}"})
    assert "config.yaml" not in arb(idea, "poll").stdout
    assert _run(initialized)["config_json"] is None


# ---------------------------------------------------------------- schema v4


def _db_at(path: Path, version: int) -> sqlite3.Connection:
    conn = db.connect(path)
    for script in db.MIGRATIONS[:version]:
        for stmt in db._split_sql(script):
            conn.execute(stmt)
    conn.execute(f"PRAGMA user_version = {version}")
    return conn


@pytest.mark.parametrize("version", [1, 2, 3])
def test_migration_to_v4_keeps_data(tmp_path: Path, version: int) -> None:
    state = tmp_path / "state"
    state.mkdir()
    conn = _db_at(state / "ledger.db", version)
    conn.execute("INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, created_at)"
                 " VALUES ('exp1', 'h', 'm', 'idea/exp1', '/w', 't')")
    conn.execute("INSERT INTO runs (id, idea_id, snapshot_commit, tree_hash, change_reason, status,"
                 " created_at, updated_at) VALUES ('r001', 1, 'c', 't', 'why', 'succeeded', 't', 't')")
    conn.execute("INSERT INTO notes (idea_id, run_id, text, created_at) VALUES (1, 'r001', 'n', 't')")
    conn.execute("INSERT INTO decisions (id, subject_type, subject_id, question, choice, decided_by,"
                 " created_at) VALUES ('d001', 'run', 'r001', 'q', 'approve', 'human', 't')")
    conn.close()
    with Ledger(state) as ledger:
        assert ledger.conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION == 7
        run = ledger.one("SELECT * FROM runs")
        assert (run["id"], run["status"], run["change_reason"], run["parent_run"]) == \
            ("r001", "succeeded", "why", None)
        note = ledger.one("SELECT * FROM notes")
        assert (note["text"], note["run_id"], note["author"]) == ("n", "r001", None)
        dec = ledger.one("SELECT * FROM decisions")
        assert (dec["choice"], dec["decided_by"], dec["payload_json"]) == ("approve", "human", None)
        assert ledger.conn.execute("PRAGMA foreign_key_check").fetchall() == []
        with ledger.write() as c:
            c.execute("INSERT INTO tasks (id, idea_id, text, created_at) VALUES ('t001', 1, 'x', 't')")
        assert ledger.one("SELECT status, done_at FROM tasks") == {"status": "open", "done_at": None}
        for bad in ("INSERT INTO tasks (id, idea_id, text, status, created_at)"
                    " VALUES ('t002', 1, 'x', 'maybe', 't')",
                    "INSERT INTO tasks (id, idea_id, text, status, created_at)"  # done needs done_at
                    " VALUES ('t003', 1, 'x', 'done', 't')",
                    "INSERT INTO tasks (id, idea_id, text, created_at) VALUES ('t004', 9, 'x', 't')",
                    "UPDATE runs SET parent_run = 'r999'"):
            with pytest.raises(sqlite3.IntegrityError):
                with ledger.write() as c:
                    c.execute(bad)


def test_submit_api_returns_existing_flag(cluster, idea: Path, initialized: Path) -> None:
    ctx = locate(idea)
    with _ledger(initialized) as l:
        i = l.one("SELECT * FROM ideas")
        run, _ = submission.submit(ctx, l, i, "x", ["python", "t.py"], gpus=1, time="1h")
        again, card = submission.submit(ctx, l, i, "y", ["python", "t.py"], gpus=1, time="1h")
    assert not run["existed"] and again["existed"] and again["id"] == run["id"]
    assert again["decision_id"] == run["decision_id"] and card.run_id == run["id"]


# ---------------------------------------------------------------- --repeat


def test_repeat_creates_a_new_run_bound_to_its_own_card(cluster, idea: Path, initialized: Path,
                                                        arb, monkeypatch) -> None:
    from arboria import lab

    lab.use(None)
    base = ["submit", "--reason", "seed variance", "--gpus", "1", "--time", "1h"]
    arb(idea, *base)
    assert "已存在：r001" in arb(idea, *base).stdout
    out = arb(idea, *base, "--repeat").stdout
    assert "已存在" not in out and "审核单 r002" in out
    assert "⚠ 重复运行（--repeat " in out
    arb(idea, *base, "--repeat")
    r1, r2, r3 = (_run(initialized, f"r00{i}") for i in (1, 2, 3))
    assert r1["snapshot_commit"] == r2["snapshot_commit"] == r3["snapshot_commit"]
    assert len({r1["card_hash"], r2["card_hash"], r3["card_hash"]}) == 3
    nonce = _req(initialized, "r002")["ask"]["repeat"]
    assert nonce and _req(initialized, "r001")["ask"]["repeat"] is None
    # the card is recomputed from the stored request: still matches, so it can be approved
    card = json.loads(arb(idea, "card", "r002", "--json").stdout)
    assert card["matches_submitted"] and card["card_hash"] == r2["card_hash"]
    arb(idea, "approve", "r002")
    assert _run(initialized, "r002")["status"] in ("approved", "queued")
    assert _run(initialized, "r001")["status"] == "awaiting_approval"
    # without --repeat, the original is still the one an identical submit returns
    assert "已存在：r001" in arb(idea, *base).stdout

    monkeypatch.chdir(idea)
    a = lab.run(reason="seed", gpus=1, time="1h")
    assert a.id == "r001" and a.existed
    b = lab.run(reason="seed", gpus=1, time="1h", repeat=True)
    assert b.id == "r004" and not b.existed
    c = lab.run(reason="seed", from_="r002", repeat=True)  # the parent's nonce is not reused
    assert _req(initialized, c.id)["ask"]["repeat"] not in (None, nonce)


def test_card_hash_without_repeat_is_unchanged(cluster) -> None:
    """Runs submitted before --repeat existed keep their hash: no `repeat` key unless set."""
    import dataclasses

    from arboria.clusters import load_profile

    ask = slurm.Ask(command=["python", "t.py"], gpus=1, nodes=1, walltime_s=3600,
                    partition="", machine="fake")
    old = {k: v for k, v in dataclasses.asdict(ask).items() if k != "repeat"}
    assert slurm.Ask.from_dict(old) == ask
    profile = load_profile("fake")
    plan = slurm.build_plan(profile, dataclasses.replace(ask, partition=profile.default_partition),
                            "proj-0000")
    assert "repeat" not in slurm.card_payload("c" * 40, plan)
    again = slurm.build_plan(profile, dataclasses.replace(plan.ask, repeat="ab12"), "proj-0000")
    assert slurm.card_payload("c" * 40, again)["repeat"] == "ab12"
    assert slurm.card_hash("c" * 40, plan) != slurm.card_hash("c" * 40, again)
