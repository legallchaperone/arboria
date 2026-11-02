"""`arb submit` / `arb approve` / `arb reject`: the human approval gate in front of Slurm.

- `submit` is purely local: snapshot, request, card, an open Decision. No backend.
- `approve` is the only path that reaches `Backend.submit`, and `_guarded_submit`
  re-checks the ledger for a human approval Decision right before every sbatch.
- The approval binds to the card hash; a changed request or profile is refused.
"""

from __future__ import annotations

import fcntl
import json
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterator

from . import backends, clusters, records, slurm
from .backends import Backend, BackendError
from .clusters import Profile
from .config import project_config, require_project_id
from .db import Ledger, now, validate_text
from .errors import ArbError
from .paths import RepoContext
from .slurm import Ask, Plan

QUESTION = "批准这个 run 上集群吗？"
OPTIONS = ["approve", "reject", "revise"]
CARD_CHANGED = "审核单已变化，请重新 arb submit"
LEGACY_RUN = ("run {run} 是在加入项目 id 之前提交的（request 里没有 project），作业名和远程目录"
              "可能与其他仓库冲突；请重新 arb submit")
REVIEW_PLACEHOLDER = "审稿（第 2 步加入，暂无）"


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


def request_of(run: dict[str, Any]) -> dict[str, Any]:
    if not run.get("request_json"):
        raise ArbError(f"run {run['id']} was not created by `arb submit`")
    return json.loads(run["request_json"])


def run_project(run: dict[str, Any]) -> str | None:
    """The project id the run was submitted under (None for runs from before project ids)."""
    project = request_of(run).get("project")
    return None if project is None else slurm.check_project(project)


def replan(run: dict[str, Any]) -> tuple[Profile, Plan]:
    """Re-derive the plan from the stored request and the *current* profile."""
    project = run_project(run)
    if project is None:
        raise ArbError(LEGACY_RUN.format(run=run["id"]))
    ask = Ask.from_dict(request_of(run)["ask"])
    profile = clusters.load_profile(ask.machine)
    return profile, slurm.build_plan(profile, ask, project)


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


def su_used(ledger: Ledger, idea_id: int | None = None) -> float:
    if idea_id is None:
        row = ledger.one("SELECT COALESCE(SUM(billed_su), 0) AS s FROM jobs")
    else:
        row = ledger.one("SELECT COALESCE(SUM(j.billed_su), 0) AS s FROM jobs j"
                         " JOIN runs r ON r.id = j.run_id WHERE r.idea_id = ?", (idea_id,))
    return round(float(row["s"]), 2)


# ---------------------------------------------------------------- card


@dataclass
class Card:
    run_id: str
    idea: str
    checks: list[tuple[bool, str]] = field(default_factory=list)
    resources: list[str] = field(default_factory=list)
    estimate: list[str] = field(default_factory=list)
    budget: list[str] = field(default_factory=list)
    card_hash: str = ""
    est_su: float = 0.0

    def text(self) -> str:
        lines = [f"审核单 {self.run_id}（idea {self.idea}）", "检查:"]
        lines += [f"  {'✓' if ok else '⚠'} {t}" for ok, t in self.checks]
        lines += ["资源:", *(f"  {t}" for t in self.resources)]
        lines += ["估计:", *(f"  {t}" for t in self.estimate)]
        lines += ["预算:", *(f"  {t}" for t in self.budget)]
        lines += [REVIEW_PLACEHOLDER, f"审核单哈希: {self.card_hash[:16]}"]
        return "\n".join(lines)

    def as_dict(self) -> dict[str, Any]:
        return {"run": self.run_id, "idea": self.idea,
                "checks": [{"ok": ok, "text": t} for ok, t in self.checks],
                "resources": self.resources, "estimate": self.estimate, "budget": self.budget,
                "card_hash": self.card_hash, "est_su": self.est_su}


def build_card(ctx: RepoContext, ledger: Ledger, run: dict[str, Any], idea: dict[str, Any],
               profile: Profile, plan: Plan, warnings: list[str] | None = None) -> Card:
    card = Card(run["id"], idea["name"], card_hash=slurm.card_hash(run["snapshot_commit"], plan),
                est_su=plan.est_su)
    a = plan.ask
    card.checks.append((True, f"快照 {run['snapshot_commit'][:12]}（不含被 .gitignore 忽略的文件）"))
    card.checks += [(False, f"快照警告：{w}") for w in warnings or []]
    card.checks.append((True, f"账户 {plan.account}"))
    if plan.over_billed:
        card.checks.append((False, f"计费卡数 {plan.billed_gpus} > 申请卡数 {a.gpus}："
                                   f"内存或 CPU 超过 {a.gpus} 卡的份额，按 {plan.billed_gpus} 卡计费"))
    else:
        card.checks.append((True, f"计费卡数 = 申请卡数 = {plan.billed_gpus}"))
    if plan.charge != 1.0:
        card.checks.append((False, f"分区 {plan.partition} 按 {plan.charge:g} 倍计费"))
    if len(plan.segments) > 1:
        card.checks.append((True, f"超过分区上限 {slurm.human_duration(plan.max_walltime_s)}，"
                                  f"分 {len(plan.segments)} 段（≤ max_segments "
                                  f"{profile.max_segments}），训练代码必须能续训"))
    if profile.kind == "fake":
        card.checks.append((False, f"机器 {profile.name} 是 FakeSlurm（本地模拟，不是真集群）"))

    per = (f"{a.nodes} 节点 × {plan.gpus_per_node} GPU" if a.nodes > 1
           else f"{plan.gpus_per_node} GPU")
    card.resources += [
        f"机器 {profile.name}（{profile.arch or '?'}），分区 {plan.partition}，{per}",
        f"每节点 CPU {plan.cpus_per_node}，内存 {plan.mem_gb_per_node}G",
        f"时长 {slurm.human_duration(a.walltime_s)}"
        + ("" if len(plan.segments) == 1 else
           "，分段 " + " + ".join(slurm.slurm_time(s) for s in plan.segments)
           + "（--dependency=afterany 串联）"),
        f"命令 {' '.join(a.command)}",
        f"项目 {plan.project}：作业名 {slurm.job_name(plan.project, run['id'], 1)}"
        + ("" if len(plan.segments) == 1 else " …")
        + f"，远程目录 {slurm.run_dir(plan.env['paths'], plan.project, run['id'])}",
    ]
    card.estimate.append(
        f"上限 {plan.billed_gpus} 卡 × {a.walltime_s / 3600:.2f} h × {plan.charge:g} = "
        f"{plan.est_su:.2f} SU（按申请的时长算满）")

    total = project_config(ctx).total_su
    used = su_used(ledger)
    idea_used = su_used(ledger, idea["id"])
    if total > 0:
        left = total - used
        card.budget.append(f"项目已用 {used:.2f} / {total:g} SU，剩余 {left:.2f}")
        if plan.est_su > left:
            card.checks.append((False, f"估计上限 {plan.est_su:.2f} SU 超过项目剩余 {left:.2f} SU"))
    else:
        card.budget.append(f"项目已用 {used:.2f} SU（arboria.toml 未设 [budget] total_su）")
        card.checks.append((False, "未设项目预算（arboria.toml [budget] total_su = 0）"))
    card.budget.append(f"本 idea 已用 {idea_used:.2f} SU")
    return card


def card_for(ctx: RepoContext, ledger: Ledger, run_id: str) -> tuple[Card, bool]:
    """The card recomputed now, and whether it still matches the stored hash."""
    run = records.get_run(ledger, run_id)
    idea = ledger.one("SELECT * FROM ideas WHERE id = ?", (run["idea_id"],))
    profile, plan = replan(run)
    card = build_card(ctx, ledger, run, idea, profile, plan)
    return card, card.card_hash == run["card_hash"]


# ---------------------------------------------------------------- submit


def submit(
    ctx: RepoContext,
    ledger: Ledger,
    idea: dict[str, Any],
    reason: str,
    command: list[str],
    *,
    gpus: int,
    time: str,
    nodes: int = 1,
    partition: str | None = None,
    machine: str | None = None,
    mem_gb: int | None = None,
    cpus: int | None = None,
    allow_large: bool = False,
    warn: Callable[[str], None] | None = None,
) -> tuple[dict[str, Any], Card]:
    """Snapshot + request + card + open Decision. Never touches a backend."""
    reason = validate_text("change_reason", reason)
    project = require_project_id(ctx)
    machine = machine or project_config(ctx).default_machine
    if not machine:
        raise ArbError("no machine: pass --machine or set default_machine in arboria.toml")
    profile = clusters.load_profile(machine)
    ask = Ask(command=list(command), gpus=gpus, nodes=nodes,
              walltime_s=slurm.parse_walltime(time),
              partition=partition or profile.default_partition, machine=machine,
              mem_gb=mem_gb, cpus=cpus)
    plan = slurm.build_plan(profile, ask, project)  # refuses before anything is written

    warnings: list[str] = []

    def collect(msg: str) -> None:
        warnings.append(msg)
        if warn:
            warn(msg)

    with ledger.write() as conn:
        run_id, snap = records.insert_run(ledger, idea, reason, allow_large=allow_large,
                                          warn=collect)
        h = slurm.card_hash(snap.commit, plan)
        conn.execute(
            "UPDATE runs SET status = 'awaiting_approval', machine = ?, \"partition\" = ?,"
            " gpus = ?, walltime = ?, est_cost = ?, request_json = ?, card_hash = ?,"
            " updated_at = ? WHERE id = ?",
            (machine, plan.partition, ask.gpus, slurm.human_duration(ask.walltime_s), plan.est_su,
             json.dumps(plan.request(), ensure_ascii=False, sort_keys=True), h, now(), run_id),
        )
        dec_id = records.insert_decision(ledger, "run", run_id, QUESTION, OPTIONS)
        ledger.event("run_submit", run_id, {"idea": idea["name"], "commit": snap.commit,
                                             "machine": machine, "card_hash": h,
                                             "est_su": plan.est_su, "decision": dec_id})
    run = records.get_run(ledger, run_id)
    card = build_card(ctx, ledger, run, idea, profile, plan, warnings)
    return run | {"decision_id": dec_id, "request": plan.request()}, card


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


def approve(ctx: RepoContext, ledger: Ledger, run_id: str) -> ApproveResult:
    """Record the human's approval (bound to the card), then submit. Resumable."""
    with run_lock(ctx, run_id):
        run = records.get_run(ledger, run_id)
        if run["status"] not in ("awaiting_approval", "approved"):
            raise ArbError(f"run {run_id} is {run['status']}; only awaiting_approval runs can be "
                           "approved")
        project = check_same_project(ctx, run)
        profile, plan = replan(run)
        if slurm.card_hash(run["snapshot_commit"], plan) != run["card_hash"]:
            raise ArbError(CARD_CHANGED)
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
                records.set_decision(ledger, dec_id, "approve", "human")
                conn.execute("UPDATE runs SET status = 'approved', updated_at = ? WHERE id = ?",
                             (now(), run_id))
                ledger.event("run_approve", run_id, {"decision": dec_id,
                                                      "card_hash": run["card_hash"]})
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
            cur = conn.execute("INSERT INTO notes (idea_id, run_id, text, created_at)"
                               " VALUES (?, ?, ?, ?)", (run["idea_id"], run_id, reason, now()))
            note_id = cur.lastrowid
        ledger.event("run_reject", run_id, {"decision": dec["id"] if dec else None,
                                             "note": note_id})
    return records.get_run(ledger, run_id)


def local_run_dir(ctx: RepoContext, run_id: str) -> Path:
    return ctx.state_dir / "runs" / run_id
