"""The approval card: what the human reads before `arb approve`, and its hash inputs.

Pure reads only (the ledger via `.one`, arboria.toml, clusters.toml); no writes, no
backend. `submission` builds and checks cards with it; the read-only dashboard
(`arb ui`) shows the same card text for runs awaiting approval.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Protocol

from . import clusters, slurm
from .clusters import Profile
from .config import project_config
from .errors import ArbError
from .paths import RepoContext
from .slurm import Ask, Plan

CARD_CHANGED = "审核单已变化，请重新 arb submit"
LEGACY_RUN = ("run {run} 是在加入项目 id 之前提交的（request 里没有 project），作业名和远程目录"
              "可能与其他仓库冲突；请重新 arb submit")


class Reader(Protocol):
    """What the card needs from a ledger: `Ledger` or the dashboard's read-only one."""

    def one(self, sql: str, params: tuple = ()) -> dict[str, Any] | None: ...


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


def su_used(ledger: Reader, idea_id: int | None = None) -> float:
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
        lines.append(f"审核单哈希: {self.card_hash[:16]}")
        return "\n".join(lines)

    def as_dict(self) -> dict[str, Any]:
        return {"run": self.run_id, "idea": self.idea,
                "checks": [{"ok": ok, "text": t} for ok, t in self.checks],
                "resources": self.resources, "estimate": self.estimate, "budget": self.budget,
                "card_hash": self.card_hash, "est_su": self.est_su}


def build_card(ctx: RepoContext, ledger: Reader, run: dict[str, Any], idea: dict[str, Any],
               profile: Profile, plan: Plan, warnings: list[str] | None = None) -> Card:
    card = Card(run["id"], idea["name"], card_hash=slurm.card_hash(run["snapshot_commit"], plan),
                est_su=plan.est_su)
    a = plan.ask
    card.checks.append((True, f"快照 {run['snapshot_commit'][:12]}（不含被 .gitignore 忽略的文件）"))
    if run.get("parent_run"):
        card.checks.append((True, f"复用 {run['parent_run']} 的快照，只改了命令参数和资源"))
    if a.repeat:
        card.checks.append((False, f"重复运行（--repeat {a.repeat}）：可能和已有的 run 完全相同，"
                                   "这是有意再跑一次"))
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
