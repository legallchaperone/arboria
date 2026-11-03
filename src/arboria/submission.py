"""`arb submit` / `arb approve` / `arb reject`: the human approval gate in front of Slurm.

- `submit` is purely local: snapshot, request, card, an open Decision. No backend.
- `approve` is the only path that reaches `Backend.submit`, and `_guarded_submit`
  re-checks the ledger for a human approval Decision right before every sbatch.
- The approval binds to the card hash; a changed request or profile is refused.
"""

from __future__ import annotations

import fcntl
import json
import secrets
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterator

from . import backends, clusters, gitutil, human, records, slurm
from .backends import Backend, BackendError
from .config import ProjectConfig, project_config, require_project_id
from .db import RUN_NOT_REUSABLE, Ledger, now, validate_text
from .errors import ArbError
from .paths import RepoContext
from .cards import (  # noqa: F401  (re-exported: submission.request_of etc.)
    CARD_CHANGED, LEGACY_RUN, REVIEW_PLACEHOLDER, Card, build_card, replan, request_of,
    run_project, su_used,
)
from .slurm import Ask, Plan

QUESTION = "批准这个 run 上集群吗？"
OPTIONS = ["approve", "reject", "revise"]
EXISTS = "已存在：{run}"
NO_COMMAND = ("no command: put it after `--` (arb submit ... -- python train.py) or set "
              "[run] command in arboria.toml")


# ---------------------------------------------------------------- helpers


@contextmanager
def run_lock(ctx: RepoContext, run_id: str) -> Iterator[None]:
    """Per-run lock held across remote calls (the ledger lock never is)."""
    lock_dir = ctx.state_dir / "locks"
    lock_dir.mkdir(parents=True, exist_ok=True)
    with open(lock_dir / f"{run_id}.lock", "a+") as fh:
        try:
            fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ArbError(f"another arb process is working on {run_id}; try again shortly") from None
        try:
            yield
        finally:
            fcntl.flock(fh, fcntl.LOCK_UN)


def check_same_project(ctx: RepoContext, run: dict[str, Any]) -> str:
    """The current project id, refusing runs that were submitted under another one."""
    current = require_project_id(ctx)
    project = run_project(run)
    if project is None:
        raise ArbError(LEGACY_RUN.format(run=run["id"]))
    if project != current:
        raise ArbError(f"run {run['id']} was submitted for project {project}, but "
                       f"{ctx.config_path} [project] id is {current}; refused")
    return current


def approval_decision(ledger: Ledger, run_id: str) -> dict[str, Any] | None:
    return ledger.one(
        "SELECT * FROM decisions WHERE subject_type = 'run' AND subject_id = ?"
        " AND choice = 'approve' AND decided_by = 'human' ORDER BY id DESC LIMIT 1",
        (run_id,),
    )


def _open_run_decision(ledger: Ledger, run_id: str) -> dict[str, Any] | None:
    return ledger.one(
        "SELECT * FROM decisions WHERE subject_type = 'run' AND subject_id = ?"
        " AND choice IS NULL ORDER BY id DESC LIMIT 1",
        (run_id,),
    )


def jobs_of(ledger: Ledger, run_id: str) -> list[dict[str, Any]]:
    return ledger.all("SELECT * FROM jobs WHERE run_id = ? ORDER BY segment", (run_id,))


# ---------------------------------------------------------------- card


def card_for(ctx: RepoContext, ledger: Ledger, run_id: str) -> tuple[Card, bool]:
    """The card recomputed now, and whether it still matches the stored hash."""
    run = records.get_run(ledger, run_id)
    idea = ledger.one("SELECT * FROM ideas WHERE id = ?", (run["idea_id"],))
    profile, plan = replan(run)
    card = build_card(ctx, ledger, run, idea, profile, plan)
    return card, card.card_hash == run["card_hash"]


# ---------------------------------------------------------------- submit


@dataclass(frozen=True)
class CommandSpec:
    """How the run's command was built (stored in request_json)."""

    command: list[str]
    base: list[str]
    source: str  # explicit (`-- CMD`) | config ([run] command)
    overrides: list[tuple[str, str]]
    style: str

    def request_extra(self) -> dict[str, Any]:
        return {"command_source": self.source, "base_command": self.base,
                "overrides": [list(kv) for kv in self.overrides], "override_style": self.style}


def build_command(cfg: ProjectConfig, explicit: list[str] | None,
                  overrides: list[tuple[str, str]], parent_request: dict[str, Any] | None
                  ) -> CommandSpec:
    """The command: `-- CMD`, else the parent's (explicit) base or `[run] command`, plus
    overrides (the parent's, updated by the new ones) in the configured style."""
    overrides = [slurm.check_override(k, v) for k, v in overrides]
    if explicit:
        base, source, ov = list(explicit), "explicit", overrides
        style = cfg.run.override_style
    elif parent_request is not None:
        # runs from before v4 always had an explicit `-- CMD` and no overrides
        source = parent_request.get("command_source", "explicit")
        ov = slurm.merge_overrides([tuple(kv) for kv in parent_request.get("overrides", [])],
                                   overrides)
        if source == "explicit":
            base = list(parent_request.get("base_command", parent_request["command"]))
            style = parent_request.get("override_style", cfg.run.override_style)
        else:
            base, style = list(cfg.run.command), cfg.run.override_style
    else:
        base, source, ov, style = list(cfg.run.command), "config", overrides, cfg.run.override_style
    if not base:
        raise ArbError(NO_COMMAND)
    return CommandSpec(command=base + slurm.render_overrides(style, ov), base=base,
                       source=source, overrides=ov, style=style)


def find_same_run(ledger: Ledger, idea_id: int, card_hash: str) -> dict[str, Any] | None:
    """An earlier run of this idea with the same card hash that is still usable."""
    marks = ", ".join("?" for _ in RUN_NOT_REUSABLE)
    return ledger.one(
        f"SELECT * FROM runs WHERE idea_id = ? AND card_hash = ? AND status NOT IN ({marks})"
        " ORDER BY id LIMIT 1", (idea_id, card_hash, *RUN_NOT_REUSABLE))


def submit(
    ctx: RepoContext,
    ledger: Ledger,
    idea: dict[str, Any],
    reason: str,
    command: list[str] | None = None,
    *,
    gpus: int | None = None,
    time: str | None = None,
    nodes: int | None = None,
    partition: str | None = None,
    machine: str | None = None,
    mem_gb: int | None = None,
    cpus: int | None = None,
    from_run: str | None = None,
    overrides: list[tuple[str, str]] | None = None,
    allow_large: bool = False,
    repeat: bool = False,
    warn: Callable[[str], None] | None = None,
) -> tuple[dict[str, Any], Card]:
    """Snapshot (or the parent run's snapshot) + request + card + open Decision.

    Never touches a backend. If this idea already has a usable run with the same card
    hash (same snapshot, command and resources), that run is returned with
    `existed = True` and nothing is written: re-executing a notebook cell is safe.
    `repeat=True` skips that and always creates a new run; a random nonce in the
    request makes its card hash unique."""
    reason = validate_text("change_reason", reason)
    project = require_project_id(ctx)
    cfg = project_config(ctx)
    parent = parent_req = None
    if from_run is not None:
        parent = records.get_run(ledger, from_run)
        if parent["idea_id"] != idea["id"]:
            raise ArbError(f"run {from_run} belongs to another idea; --from only reuses runs of "
                           f"{idea['name']} (use lab.fork to start a new idea from it)")
        parent_req = request_of(parent) if parent.get("request_json") else None
    pask = Ask.from_dict(parent_req["ask"]) if parent_req else None
    if gpus is None and pask:
        gpus = pask.gpus
    if time is None and pask:
        time = slurm.human_duration(pask.walltime_s)
    if gpus is None or time is None:
        raise ArbError("--gpus and --time are required" + (
            f" ({from_run} was never submitted, so there is nothing to default to)"
            if from_run else ""))
    machine = machine or (pask.machine if pask else None) or cfg.default_machine
    if not machine:
        raise ArbError("no machine: pass --machine or set default_machine in arboria.toml")
    profile = clusters.load_profile(machine)
    same_machine = pask is not None and pask.machine == machine
    spec = build_command(cfg, command, list(overrides or []), parent_req)
    ask = Ask(command=spec.command, gpus=gpus,
              nodes=nodes if nodes is not None else (pask.nodes if pask else 1),
              walltime_s=slurm.parse_walltime(time),
              partition=partition or (pask.partition if same_machine else None)
              or profile.default_partition,
              machine=machine,
              mem_gb=mem_gb if mem_gb is not None else (pask.mem_gb if same_machine else None),
              cpus=cpus if cpus is not None else (pask.cpus if same_machine else None),
              repeat=secrets.token_hex(4) if repeat else None)
    plan = slurm.build_plan(profile, ask, project)  # refuses before anything is written

    warnings: list[str] = []

    def collect(msg: str) -> None:
        warnings.append(msg)
        if warn:
            warn(msg)

    existing = None
    with ledger.write() as conn:
        if parent is not None:
            commit, tree = parent["snapshot_commit"], parent["tree_hash"]
            if not gitutil.ok(["cat-file", "-e", f"{commit}^{{commit}}"], ctx.main):
                raise ArbError(f"snapshot {commit[:12]} of {from_run} is missing from the repo")
        else:
            snap = records.take_snapshot(ledger, idea, allow_large=allow_large, warn=collect)
            commit, tree = snap.commit, snap.tree
        h = slurm.card_hash(commit, plan)
        existing = None if repeat else find_same_run(ledger, idea["id"], h)
        if existing is None:
            run_id = records.insert_run_at(ledger, idea, reason, commit, tree,
                                           parent_run=parent["id"] if parent else None)
            request = plan.request() | spec.request_extra() | {
                "parent_run": parent["id"] if parent else None}
            wandb_url = cfg.wandb.run_url(slurm.wandb_id(project, run_id))
            conn.execute(
                "UPDATE runs SET status = 'awaiting_approval', machine = ?, \"partition\" = ?,"
                " gpus = ?, walltime = ?, est_cost = ?, request_json = ?, card_hash = ?,"
                " wandb_url = ?, updated_at = ? WHERE id = ?",
                (machine, plan.partition, ask.gpus, slurm.human_duration(ask.walltime_s),
                 plan.est_su, json.dumps(request, ensure_ascii=False, sort_keys=True), h,
                 wandb_url, now(), run_id),
            )
            dec_id = records.insert_decision(ledger, "run", run_id, QUESTION, OPTIONS)
            ledger.event("run_submit", run_id, {"idea": idea["name"], "commit": commit,
                                                 "machine": machine, "card_hash": h,
                                                 "est_su": plan.est_su, "decision": dec_id,
                                                 "parent_run": parent["id"] if parent else None,
                                                 "repeat": ask.repeat})
    if existing is not None:
        dec = _open_run_decision(ledger, existing["id"])
        card = build_card(ctx, ledger, existing, idea, profile, plan, warnings)
        return existing | {"existed": True, "decision_id": dec["id"] if dec else None,
                           "request": request_of(existing)}, card
    run = records.get_run(ledger, run_id)
    card = build_card(ctx, ledger, run, idea, profile, plan, warnings)
    return run | {"existed": False, "decision_id": dec_id, "request": request_of(run)}, card


# ---------------------------------------------------------------- approve


@dataclass
class ApproveResult:
    run: dict[str, Any]
    decision_id: str | None  # None when resuming an earlier approval
    submitted: list[tuple[int, str, bool]]  # (segment, job id, adopted existing job)


def _guarded_submit(ledger: Ledger, backend: Backend, run_id: str, script: str, name: str,
                    dependency: str | None) -> str:
    """The single call site of Backend.submit. Refuses without a recorded human approval."""
    run = ledger.one("SELECT status FROM runs WHERE id = ?", (run_id,))
    if run is None or run["status"] != "approved" or approval_decision(ledger, run_id) is None:
        raise ArbError(f"refusing to submit {name}: run {run_id} has no human approval "
                       "Decision in the ledger")
    return backend.submit(script, name, dependency)


def _set_job(ledger: Ledger, run_id: str, segment: int, **fields: Any) -> None:
    sets = ", ".join(f"{k} = ?" for k in fields)
    with ledger.write() as conn:
        conn.execute(f"UPDATE jobs SET {sets}, updated_at = ? WHERE run_id = ? AND segment = ?",
                     (*fields.values(), now(), run_id, segment))
        if "job_id" in fields:
            ledger.event("job_submit", run_id, {"segment": segment, "job_id": fields["job_id"]})


def submit_segments(ctx: RepoContext, ledger: Ledger, run: dict[str, Any], plan: Plan,
                    backend: Backend) -> list[tuple[int, str, bool]]:
    """Ship the code once, then sbatch each missing segment idempotently."""
    run_id = run["id"]
    idea = ledger.one("SELECT name FROM ideas WHERE id = ?", (run["idea_id"],))["name"]
    paths = plan.env["paths"]
    rows = {j["segment"]: j for j in jobs_of(ledger, run_id)}
    if not any(j["job_id"] for j in rows.values()):
        backend.ship(ctx.main, run["snapshot_commit"], slurm.code_dir(paths, plan.project, run_id))
    done: list[tuple[int, str, bool]] = []
    prev: str | None = None
    for k in range(1, len(plan.segments) + 1):
        name = slurm.job_name(plan.project, run_id, k)
        row = rows.get(k)
        if row and row["job_id"]:
            prev = row["job_id"]
            continue
        if row is None:
            with ledger.write() as conn:
                conn.execute("INSERT INTO jobs (run_id, segment, job_name, state, updated_at)"
                             " VALUES (?, ?, ?, 'submitting', ?)", (run_id, k, name, now()))
        job_id = backend.find_job(name)
        adopted = job_id is not None
        if job_id is None:
            script = slurm.render_sbatch(plan, run_id, idea, k)
            job_id = _guarded_submit(ledger, backend, run_id, script, name, prev)
        _set_job(ledger, run_id, k, job_id=job_id, state="pending", submitted_at=now())
        done.append((k, job_id, adopted))
        prev = job_id
    return done


def approve(ctx: RepoContext, ledger: Ledger, run_id: str,
            show: Callable[[str], None] | None = None) -> ApproveResult:
    """Record the human's approval (bound to the card), then submit. Resumable.

    Only in the human's own terminal (see `human`): refused inside Claude Code or
    without a TTY; the human sees the card (`show`) and types the run id."""
    command = f"arb approve {run_id}"
    human.require_terminal(command)
    with run_lock(ctx, run_id):
        run = records.get_run(ledger, run_id)
        if run["status"] not in ("awaiting_approval", "approved"):
            raise ArbError(f"run {run_id} is {run['status']}; only awaiting_approval runs can be "
                           "approved")
        project = check_same_project(ctx, run)
        profile, plan = replan(run)
        if slurm.card_hash(run["snapshot_commit"], plan) != run["card_hash"]:
            raise ArbError(CARD_CHANGED)
        if show is not None and run["status"] == "awaiting_approval":
            idea = ledger.one("SELECT * FROM ideas WHERE id = ?", (run["idea_id"],))
            show(build_card(ctx, ledger, run, idea, profile, plan).text())
        human.confirm_typed(run_id)
        prov = human.provenance()
        dec_id = None
        if run["status"] == "awaiting_approval":
            with ledger.write() as conn:
                current = conn.execute("SELECT status FROM runs WHERE id = ?", (run_id,)).fetchone()
                if current["status"] != "awaiting_approval":
                    raise ArbError(f"run {run_id} changed to {current['status']} meanwhile")
                dec = _open_run_decision(ledger, run_id)
                if dec is None:
                    raise ArbError(f"run {run_id} has no open approval decision")
                dec_id = dec["id"]
                records.set_decision(ledger, dec_id, "approve", "human", prov)
                conn.execute("UPDATE runs SET status = 'approved', updated_at = ? WHERE id = ?",
                             (now(), run_id))
                ledger.event("run_approve", run_id, {"decision": dec_id,
                                                      "card_hash": run["card_hash"],
                                                      "author": "human"})
        elif approval_decision(ledger, run_id) is None:
            raise ArbError(f"run {run_id} is approved but has no human approval Decision")
        run = records.get_run(ledger, run_id)
        try:
            backend = backends.get_backend(profile, project)
            submitted = submit_segments(ctx, ledger, run, plan, backend)
        except BackendError as exc:
            raise ArbError(f"{exc}\n{run_id} 已批准，但提交没有完成。处理好之后重新运行 "
                           f"`arb approve {run_id}` 继续（已提交的段不会重复提交）。") from exc
        job_ids = [j["job_id"] for j in jobs_of(ledger, run_id)]
        with ledger.write() as conn:
            conn.execute("UPDATE runs SET status = 'queued', updated_at = ? WHERE id = ?"
                         " AND status = 'approved'", (now(), run_id))
            ledger.event("run_queued", run_id, {"jobs": job_ids})
        return ApproveResult(records.get_run(ledger, run_id), dec_id, submitted)


# ---------------------------------------------------------------- reject


def reject(ctx: RepoContext, ledger: Ledger, run_id: str, reason: str | None = None) -> dict[str, Any]:
    """Decide the approval Decision as reject. No backend call."""
    run = records.get_run(ledger, run_id)
    if run["status"] != "awaiting_approval":
        raise ArbError(f"run {run_id} is {run['status']}; only awaiting_approval runs can be "
                       "rejected (use `arb cancel` for submitted runs)")
    reason = validate_text("note", reason, required=False)
    with ledger.write() as conn:
        dec = _open_run_decision(ledger, run_id)
        if dec:
            records.set_decision(ledger, dec["id"], "reject", "human")
        conn.execute("UPDATE runs SET status = 'rejected', updated_at = ? WHERE id = ?",
                     (now(), run_id))
        note_id = None
        if reason:
            note_id = records.insert_note(ledger, run["idea_id"], reason, run_id)
        ledger.event("run_reject", run_id, {"decision": dec["id"] if dec else None,
                                             "note": note_id})
    return records.get_run(ledger, run_id)


def local_run_dir(ctx: RepoContext, run_id: str) -> Path:
    return ctx.state_dir / "runs" / run_id
