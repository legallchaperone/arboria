"""Pure Slurm logic: walltime, DeltaAI billing, segment plans, sbatch scripts, card hash.

Nothing here touches the ledger, the filesystem or the network.
"""

from __future__ import annotations

import hashlib
import json
import math
import re
import shlex
from dataclasses import asdict, dataclass
from typing import TYPE_CHECKING, Any

from .errors import ArbError

if TYPE_CHECKING:  # pragma: no cover
    from .clusters import Profile

MIN_WALLTIME_S = 60
_UNITS_RE = re.compile(r"^(?:(\d+)d)?(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$")
_CLOCK_RE = re.compile(r"^(?:(\d+)-)?(\d+):(\d{2}):(\d{2})$")
_USER_VAR_RE = re.compile(r"\$\{USER\}|\$USER(?![A-Za-z0-9_])")
JOB_NAME_RE = re.compile(r"^arboria-([a-z0-9][a-z0-9-]*)-s([1-9][0-9]*)$")


# ---------------------------------------------------------------- walltime


def parse_walltime(text: Any) -> int:
    """Seconds from "90m", "12h", "1d6h", "45s", "HH:MM:SS" or "D-HH:MM:SS"."""
    if not isinstance(text, str):
        raise ArbError(f"invalid walltime {text!r}")
    t = text.strip().lower()
    m = _CLOCK_RE.match(t)
    if m:
        d, h, mi, s = (int(x or 0) for x in m.groups())
        if mi >= 60 or s >= 60:
            raise ArbError(f"invalid walltime {text!r}: minutes and seconds must be < 60")
        total = ((d * 24 + h) * 60 + mi) * 60 + s
    else:
        m = _UNITS_RE.match(t)
        if not t or not m or not any(m.groups()):
            raise ArbError(f"invalid walltime {text!r}; use e.g. 90m, 12h, 1d6h or HH:MM:SS")
        d, h, mi, s = (int(x or 0) for x in m.groups())
        total = ((d * 24 + h) * 60 + mi) * 60 + s
    if total <= 0:
        raise ArbError(f"walltime {text!r} must be > 0")
    return total


def slurm_time(seconds: int) -> str:
    """`--time` value: HH:MM:SS (hours may exceed 24, which Slurm accepts)."""
    h, rest = divmod(int(seconds), 3600)
    m, s = divmod(rest, 60)
    return f"{h:02d}:{m:02d}:{s:02d}"


def human_duration(seconds: int) -> str:
    """Compact form for cards: 1d6h, 12h, 1h30m, 90s."""
    d, rest = divmod(int(seconds), 86400)
    h, rest = divmod(rest, 3600)
    m, s = divmod(rest, 60)
    out = "".join(f"{v}{u}" for v, u in ((d, "d"), (h, "h"), (m, "m"), (s, "s")) if v)
    return out or "0s"


def segment_plan(total_s: int, max_s: int, max_segments: int) -> list[int]:
    """Split a walltime longer than the partition limit into chained segments."""
    n = max(1, math.ceil(total_s / max_s))
    if n > max_segments:
        raise ArbError(
            f"walltime {human_duration(total_s)} needs {n} segments of at most "
            f"{human_duration(max_s)} (partition limit), more than max_segments = {max_segments}; "
            "shorten the run or raise max_segments in clusters.toml"
        )
    return [max_s] * (n - 1) + [total_s - max_s * (n - 1)]


# ---------------------------------------------------------------- billing


def billed_gpus(gpus: int, mem_gb: float, cpus: int, mem_per_gpu_gb: float,
                cpus_per_gpu: int) -> int:
    """DeltaAI charges the max of GPUs, memory and CPUs, each expressed in GPU shares."""
    eps = 1e-9
    return max(int(gpus), math.ceil(mem_gb / mem_per_gpu_gb - eps),
               math.ceil(cpus / cpus_per_gpu - eps))


# ---------------------------------------------------------------- request plan


@dataclass(frozen=True)
class Ask:
    """What the submitter asked for (stored as-is; the plan is re-derived from it)."""

    command: list[str]
    gpus: int
    nodes: int
    walltime_s: int
    partition: str
    machine: str
    mem_gb: int | None = None  # per node; None = gpus x mem_per_gpu_gb
    cpus: int | None = None  # per node; None = gpus x cpus_per_gpu

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "Ask":
        return cls(command=list(d["command"]), gpus=d["gpus"], nodes=d["nodes"],
                   walltime_s=d["walltime_s"], partition=d["partition"], machine=d["machine"],
                   mem_gb=d.get("mem_gb"), cpus=d.get("cpus"))


@dataclass(frozen=True)
class Plan:
    ask: Ask
    account: str
    partition: str
    charge: float
    max_walltime_s: int
    gpus_per_node: int  # requested per node
    cpus_per_node: int
    mem_gb_per_node: int
    billed_per_node: int
    billed_gpus: int  # total over nodes
    segments: list[int]
    env: dict[str, Any]

    @property
    def over_billed(self) -> bool:
        return self.billed_gpus > self.ask.gpus

    @property
    def est_su(self) -> float:
        return round(sum(self.billed_gpus * s / 3600 * self.charge for s in self.segments), 2)

    def request(self) -> dict[str, Any]:
        """What is stored in runs.request_json."""
        return {
            "ask": asdict(self.ask),
            "command": self.ask.command, "machine": self.ask.machine, "partition": self.partition,
            "nodes": self.ask.nodes, "gpus": self.ask.gpus, "walltime_s": self.ask.walltime_s,
            "walltime": human_duration(self.ask.walltime_s), "account": self.account,
            "gpus_per_node": self.gpus_per_node, "cpus_per_node": self.cpus_per_node,
            "mem_gb_per_node": self.mem_gb_per_node, "billed_gpus": self.billed_gpus,
            "charge": self.charge, "segments": self.segments, "env": self.env,
            "est_su": self.est_su,
        }


def build_plan(profile: "Profile", ask: Ask) -> Plan:
    """Validate `ask` against the machine profile; refuse anything that cannot be billed right."""
    if not ask.command or not all(isinstance(a, str) and a for a in ask.command):
        raise ArbError("no command: put it after `--`, e.g. arb submit ... -- python train.py")
    if not profile.account_set:
        raise ArbError(f"machine {profile.name!r} has no account; set `account` in "
                       f"{profile.source or 'clusters.toml'} (from `accounts` on the cluster)")
    if ask.nodes < 1:
        raise ArbError("--nodes must be >= 1")
    if ask.gpus < 1:
        raise ArbError("--gpus must be >= 1")
    if ask.gpus % ask.nodes:
        raise ArbError(f"--gpus {ask.gpus} is not divisible by --nodes {ask.nodes}")
    per_node = ask.gpus // ask.nodes
    if per_node > profile.gpus_per_node:
        raise ArbError(f"{per_node} GPUs per node > {profile.gpus_per_node} on {profile.name}; "
                       "use more --nodes")
    if ask.walltime_s < MIN_WALLTIME_S:
        raise ArbError(f"--time must be at least {MIN_WALLTIME_S}s")
    part = profile.partition(ask.partition)
    mem = ask.mem_gb if ask.mem_gb is not None else math.floor(per_node * profile.mem_per_gpu_gb)
    cpus = ask.cpus if ask.cpus is not None else per_node * profile.cpus_per_gpu
    if mem < 1 or cpus < 1:
        raise ArbError("--mem and --cpus must be >= 1")
    billed_node = billed_gpus(per_node, mem, cpus, profile.mem_per_gpu_gb, profile.cpus_per_gpu)
    if billed_node > profile.gpus_per_node:
        raise ArbError(f"--mem {mem}G / --cpus {cpus} per node is more than a whole "
                       f"{profile.name} node ({profile.gpus_per_node} GPU shares)")
    segments = segment_plan(ask.walltime_s, part.max_walltime_s, profile.max_segments)
    env = {"arch": profile.arch, "prelude": list(profile.prelude),
           "env_activate": list(profile.env_activate), "paths": dict(profile.paths)}
    return Plan(ask=ask, account=profile.account, partition=part.name, charge=part.charge,
                max_walltime_s=part.max_walltime_s, gpus_per_node=per_node, cpus_per_node=cpus,
                mem_gb_per_node=mem, billed_per_node=billed_node,
                billed_gpus=billed_node * ask.nodes, segments=segments, env=env)


# ---------------------------------------------------------------- approval card hash


def card_payload(snapshot_commit: str, plan: Plan) -> dict[str, Any]:
    """Everything an approval is bound to."""
    return {
        "snapshot_commit": snapshot_commit,
        "command": plan.ask.command,
        "machine": plan.ask.machine,
        "partition": plan.partition,
        "resources": {
            "account": plan.account, "nodes": plan.ask.nodes, "gpus": plan.ask.gpus,
            "gpus_per_node": plan.gpus_per_node, "cpus_per_node": plan.cpus_per_node,
            "mem_gb_per_node": plan.mem_gb_per_node, "billed_gpus": plan.billed_gpus,
            "charge": plan.charge,
        },
        "segments": plan.segments,
        "env": plan.env,
    }


def canonical_json(obj: Any) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def card_hash(snapshot_commit: str, plan: Plan) -> str:
    return hashlib.sha256(canonical_json(card_payload(snapshot_commit, plan)).encode()).hexdigest()


# ---------------------------------------------------------------- sbatch script


def job_name(run_id: str, segment: int) -> str:
    return f"arboria-{run_id}-s{segment}"


def parse_job_name(name: str) -> tuple[str, int]:
    m = JOB_NAME_RE.match(name or "")
    if not m:
        raise ArbError(f"not an arboria job name: {name!r}")
    return m.group(1), int(m.group(2))


def run_dir(paths: dict[str, str], run_id: str) -> str:
    return f"{paths['runs']}/{run_id}"


def code_dir(paths: dict[str, str], run_id: str) -> str:
    return f"{run_dir(paths, run_id)}/code"


def script_path(paths: dict[str, str], run_id: str, segment: int) -> str:
    return f"{run_dir(paths, run_id)}/sbatch/s{segment}.sh"


def log_pattern(paths: dict[str, str], run_id: str, segment: int) -> str:
    """Log path with Slurm's %j; `$USER` stays a shell variable."""
    return f"{run_dir(paths, run_id)}/logs/s{segment}.%j.out"


def log_path(paths: dict[str, str], run_id: str, segment: int, job_id: str) -> str:
    return log_pattern(paths, run_id, segment).replace("%j", job_id)


def out_dir(paths: dict[str, str], run_id: str) -> str:
    return f"{paths['out']}/{run_id}"


def sbatch_value(path: str) -> str:
    """#SBATCH lines are not shell-expanded: `$USER` must be Slurm's `%u`."""
    return _USER_VAR_RE.sub("%u", path)


def dq(path: str) -> str:
    """Double-quote a validated profile path so `$USER` still expands."""
    if any(c in path for c in '"\\`') or "$(" in path:
        raise ArbError(f"unsafe remote path {path!r}")
    return f'"{path}"'


def render_sbatch(plan: Plan, run_id: str, idea: str, segment: int) -> str:
    """The batch script for one segment (dependencies are passed to sbatch, not written here)."""
    paths = plan.env["paths"]
    n = len(plan.segments)
    if not 1 <= segment <= n:
        raise ArbError(f"segment {segment} out of range 1..{n}")
    log = sbatch_value(log_pattern(paths, run_id, segment))
    lines = [
        "#!/bin/bash",
        f"# arboria run {run_id}, segment {segment}/{n}, idea {idea}. Generated by arb; do not edit.",
        f"#SBATCH --job-name={job_name(run_id, segment)}",
        f"#SBATCH --account={plan.account}",
        f"#SBATCH --partition={plan.partition}",
        f"#SBATCH --nodes={plan.ask.nodes}",
        "#SBATCH --ntasks-per-node=1",
        f"#SBATCH --gpus-per-node={plan.gpus_per_node}",
        f"#SBATCH --cpus-per-task={plan.cpus_per_node}",
        f"#SBATCH --mem={plan.mem_gb_per_node}G",
        f"#SBATCH --time={slurm_time(plan.segments[segment - 1])}",
        f"#SBATCH --output={log}",
        f"#SBATCH --error={log}",
        "set -euo pipefail",
    ]
    setup = [*plan.env["prelude"], *plan.env["env_activate"]]
    if setup:
        # module/conda activation scripts often read unset variables
        lines += ["set +u", *setup, "set -u"]
    lines += [
        f"export ARBORIA_RUN_ID={shlex.quote(run_id)}",
        f"export ARBORIA_IDEA={shlex.quote(idea)}",
        f"export ARBORIA_SEGMENT={segment}",
        f"export ARBORIA_DATA={dq(paths['data'])}",
        f"export ARBORIA_CACHE={dq(paths['cache'])}",
        'export HF_HOME="$ARBORIA_CACHE/hf"',
        f"export ARBORIA_CKPT_DIR={dq(paths['ckpt'] + '/' + run_id)}",
        f"export ARBORIA_OUT={dq(out_dir(paths, run_id))}",
        'mkdir -p "$ARBORIA_CACHE" "$HF_HOME" "$ARBORIA_CKPT_DIR" "$ARBORIA_OUT"',
        f"cd {dq(code_dir(paths, run_id))}",
        shlex.join(plan.ask.command),
    ]
    return "\n".join(lines) + "\n"
