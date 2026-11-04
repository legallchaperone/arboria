"""`arb machine init` and `arb doctor`: measure a Slurm machine, check a profile.

Everything remote goes through `SshSession` (gate on `ssh -O check`, BatchMode, reuse
the user's master only, never retried). Both commands only read on the cluster: one
fixed probe script per call, plus `test -d/-w` on the profile's own paths for doctor.
Nothing here edits ~/.ssh/config; a missing ControlMaster setup is reported with the
block to add.
"""

from __future__ import annotations

import math
import re
import shutil
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable

from . import clusters
from .backends.base import NeedLogin, need_login_message
from .backends.ssh import SshSession
from .clusters import PATH_ROLES
from .errors import ArbError
from .slurm import dq, parse_walltime

# One fixed script (no interpolation): each section starts with a `@@name` line.
PROBE_SCRIPT = r"""echo @@user; echo "$USER"
echo @@arch; uname -m
echo @@accounts; accounts 2>&1
echo @@partitions; scontrol show partition -o 2>&1
echo @@node; n=$(sinfo -h -N -o '%N %P' 2>/dev/null | awk '$2 ~ /\*$/ {print $1; exit}'); [ -n "$n" ] && scontrol show node -o "$n" 2>&1
echo @@flags; scontrol show config 2>/dev/null | grep -E '^PriorityFlags'
echo @@quota; quota 2>&1
echo @@tools; for t in sbatch sacct accounts jobcharge apptainer uv pixi; do command -v "$t"; done
echo @@end
"""

SSH_BLOCK = """Host {host}
    HostName <login node>
    User <user>
    ControlMaster auto
    ControlPath ~/.ssh/cm-%C
    ControlPersist 7d
    ServerAliveInterval 60"""

_SECTION_RE = re.compile(r"^@@([a-z]+)$")
_GPU_GRES_RE = re.compile(r"(?:^|,)gpu(?::[^:,()]+)?:([0-9]+)")
_QUOTA_PATH_RE = re.compile(r"\|\s*(/\S+?)\s*\|")
_MEM_UNIT_TO_PER_GB = {"K": 1024 * 1024, "M": 1024, "": 1024, "G": 1, "T": 1 / 1024}
_SHARE_TOL = 0.01  # 1%: CPU / memory shares from the node vs from the billing weights


# ------------------------------------------------------------------ parsing (pure)

def split_sections(text: str) -> dict[str, str]:
    out: dict[str, list[str]] = {}
    current = None
    for line in text.splitlines():
        m = _SECTION_RE.fullmatch(line.strip())
        if m:
            current = m.group(1)
            out.setdefault(current, [])
        elif current is not None:
            out[current].append(line)
    return {k: "\n".join(v).strip() for k, v in out.items()}


def parse_kv(line: str) -> dict[str, str]:
    """`scontrol show ... -o` line -> {key: value} (the first `=` splits)."""
    out: dict[str, str] = {}
    for token in line.split():
        key, eq, value = token.partition("=")
        if eq and key and key not in out:
            out[key] = value
    return out


def parse_partitions(text: str) -> dict[str, dict[str, str]]:
    parts = {}
    for line in text.splitlines():
        kv = parse_kv(line)
        if "PartitionName" in kv:
            parts[kv["PartitionName"]] = kv
    return parts


def parse_weights(text: str) -> dict[str, float]:
    """TRESBillingWeights -> {"cpu": per CPU, "mem": per GB, "gpu": per GPU}."""
    out: dict[str, float] = {}
    for item in (text or "").split(","):
        key, _, value = item.partition("=")
        key = key.strip().lower()
        m = re.fullmatch(r"([0-9]*\.?[0-9]+)([KMGT]?)", value.strip())
        if not m:
            continue
        number = float(m.group(1))
        if key == "cpu":
            out["cpu"] = number
        elif key == "mem":
            out["mem"] = number * _MEM_UNIT_TO_PER_GB[m.group(2)]
        elif key in ("gres/gpu", "gpu"):
            out["gpu"] = number
    return out


def parse_accounts(text: str) -> list[tuple[str, str]]:
    """`accounts` (NCSA) -> [(account, balance)] from the rows under the dashed rule."""
    rows, seen_rule = [], False
    for line in text.splitlines():
        if re.fullmatch(r"[-\s]{6,}", line) and "-" in line:
            seen_rule = True
            continue
        if seen_rule and line.strip():
            cols = line.split()
            rows.append((cols[0], cols[1] if len(cols) > 1 else ""))
    return rows


def parse_quota_paths(text: str) -> list[str]:
    return [p for p in _QUOTA_PATH_RE.findall(text or "") if p.startswith("/")]


def walltime_text(seconds: int) -> str:
    if seconds % 3600 == 0:
        return f"{seconds // 3600}h"
    if seconds % 60 == 0:
        return f"{seconds // 60}m"
    return f"{seconds}s"


def _num(text: str | None) -> float | None:
    try:
        return float(text) if text not in (None, "") else None
    except ValueError:
        return None


def _allows(part: dict[str, str], account: str) -> bool:
    allowed = part.get("AllowAccounts", "ALL")
    denied = part.get("DenyAccounts", "")
    if account and account in denied.split(","):
        return False
    return allowed == "ALL" or (bool(account) and account in allowed.split(","))


# ------------------------------------------------------------------ proposal

@dataclass
class Proposal:
    values: dict[str, Any]
    warnings: list[str] = field(default_factory=list)
    other_partitions: list[str] = field(default_factory=list)
    accounts: list[tuple[str, str]] = field(default_factory=list)
    tools: list[str] = field(default_factory=list)


def propose(name: str, host: str, sections: dict[str, str], *, account: str | None = None,
            existing: dict[str, Any] | None = None) -> Proposal:
    """Profile values from a probe; keeps the user's own choices from `existing`."""
    existing = existing or {}
    warnings: list[str] = []
    accounts = parse_accounts(sections.get("accounts", ""))
    names = [a for a, _ in accounts]
    old_account = existing.get("account", "")
    if account:
        if names and account not in names:
            raise ArbError(f"--account {account}: not one of your accounts ({', '.join(names)})")
    elif old_account and "<" not in old_account and (not names or old_account in names):
        account = old_account
    elif len(names) == 1:
        account = names[0]
    else:
        account = "<from `accounts`>"
        warnings.append("several accounts (" + ", ".join(names) + "): pass --account"
                        if names else "`accounts` listed no account: set account by hand")

    parts = parse_partitions(sections.get("partitions", ""))
    if not parts:
        raise ArbError("`scontrol show partition` printed no partitions; is this a Slurm login "
                       "node?")
    default = next((p for p, kv in parts.items() if kv.get("Default") == "YES"), None)
    usable = {p: kv for p, kv in parts.items()
              if _allows(kv, account) and kv.get("State", "UP") == "UP"
              and kv.get("Hidden", "NO") != "YES"}
    if default not in usable:
        raise ArbError(f"the default partition {default!r} is not usable with account {account}")
    base_w = parse_weights(usable[default].get("TRESBillingWeights", ""))
    gpu_w = base_w.get("gpu", 0.0)

    node = parse_kv(sections.get("node", ""))
    m = _GPU_GRES_RE.search(node.get("Gres", ""))
    if not m:
        raise ArbError(f"cannot read the GPU count of a {default} node from `scontrol show node`")
    gpus = int(m.group(1))
    cpus_node = int(_num(node.get("CPUTot")) or 0)
    alloc_mb = (_num(node.get("RealMemory")) or 0) - (_num(node.get("MemSpecLimit")) or 0)
    job_defaults = parse_kv(usable[default].get("JobDefaults", "").replace(",", " "))
    def_mem_mb = _num(job_defaults.get("DefMemPerGPU"))

    cpus_per_gpu = cpus_node // gpus if cpus_node else 0
    mem_per_gpu_gb = (def_mem_mb or alloc_mb / gpus) / 1024 if (def_mem_mb or alloc_mb) else 0.0
    if alloc_mb > 0:  # a default whole-node request must still fit the node
        mem_per_gpu_gb = min(mem_per_gpu_gb, alloc_mb / gpus / 1024)
    if "MAX_TRES" not in sections.get("flags", ""):
        warnings.append("PriorityFlags lacks MAX_TRES: arb's billed-GPU rule "
                        "(max of GPU, CPU and memory shares) may not match this cluster")
    if gpu_w > 0 and base_w.get("cpu"):
        share = gpu_w / base_w["cpu"]
        if cpus_per_gpu and abs(share - cpus_per_gpu) / share > _SHARE_TOL:
            warnings.append(f"node gives {cpus_per_gpu} CPUs per GPU but billing makes "
                            f"{share:.2f} CPUs one GPU; using the smaller")
        cpus_per_gpu = min(cpus_per_gpu or 10**9, math.floor(share + 1e-6))
    if gpu_w > 0 and base_w.get("mem"):
        share = gpu_w / base_w["mem"]
        if mem_per_gpu_gb and abs(share - mem_per_gpu_gb) / share > _SHARE_TOL:
            warnings.append(f"node gives {mem_per_gpu_gb:.3f} GB per GPU but billing makes "
                            f"{share:.3f} GB one GPU; using the smaller")
        mem_per_gpu_gb = min(mem_per_gpu_gb or 1e18, share)
    mem_per_gpu_gb = math.floor(mem_per_gpu_gb * 1000) / 1000
    if cpus_per_gpu < 1 or mem_per_gpu_gb <= 0:
        raise ArbError("could not measure CPUs / memory per GPU; set them by hand")

    partitions: dict[str, dict[str, Any]] = {}
    keep = [default] + sorted(p for p in usable if p != default and p.startswith(default + "-"))
    for p in keep:
        kv = usable[p]
        try:
            max_s = parse_walltime(kv.get("MaxTime", ""))
        except ArbError:
            warnings.append(f"partition {p}: MaxTime {kv.get('MaxTime')!r} is not a time limit; "
                            "left out")
            continue
        w = parse_weights(kv.get("TRESBillingWeights", "")).get("gpu", 0.0)
        charge = round(w / gpu_w, 4) if gpu_w > 0 and w > 0 else 1.0
        partitions[p] = {"max_walltime": walltime_text(max_s), "charge": charge}
    other = sorted(set(usable) - set(partitions))

    tools = [line.rsplit("/", 1)[-1] for line in sections.get("tools", "").splitlines()
             if line.startswith("/")]
    if "accounts" not in tools and "balance_cmd" not in existing:
        warnings.append("no `accounts` command: balance_cmd left empty")

    old_paths = existing.get("paths") if isinstance(existing.get("paths"), dict) else {}
    if old_paths and all(isinstance(old_paths.get(r), str) and "<" not in old_paths[r]
                         for r in PATH_ROLES):
        paths = {r: old_paths[r] for r in PATH_ROLES}
    else:
        paths = default_paths(account, parse_quota_paths(sections.get("quota", "")))
        if any("<" in v for v in paths.values()):
            warnings.append("could not tell the allocation's directories from `quota`: "
                            "fill in the <proj> paths by hand")

    values: dict[str, Any] = {
        "kind": "slurm", "host": host, "arch": sections.get("arch", "").strip(),
        "account": account, "gpus_per_node": gpus, "cpus_per_gpu": cpus_per_gpu,
        "mem_per_gpu_gb": mem_per_gpu_gb, "default_partition": default,
        "max_segments": existing.get("max_segments", 4),
        "balance_cmd": existing.get("balance_cmd", "accounts" if "accounts" in tools else ""),
        # On DeltaAI one GPU-hour on the default partition is one SU.
        "billing_per_su": existing.get("billing_per_su",
                                       int(gpu_w) if gpu_w > 0 and float(gpu_w).is_integer()
                                       else 0),
        "prelude": list(existing.get("prelude", [])),
        "env_activate": list(existing.get("env_activate", [])),
        "partitions": partitions, "paths": paths,
    }
    return Proposal(values, warnings, other, accounts, tools)


def default_paths(account: str, quota_paths: list[str]) -> dict[str, str]:
    """NCSA layout: /projects, /work/nvme, /work/hdd per allocation (<proj>-...)."""
    allocs = sorted({p.split("/")[2] for p in quota_paths
                     if re.fullmatch(r"/projects/[A-Za-z0-9._-]+", p)})
    proj = next((a for a in allocs if account == a or account.startswith(a + "-")), None)
    if proj is None and len(allocs) == 1:
        proj = allocs[0]
    proj = proj or "<proj>"
    have = set(quota_paths)
    nvme = f"/work/nvme/{proj}" if f"/work/nvme/{proj}" in have or proj == "<proj>" \
        else f"/projects/{proj}"
    hdd = f"/work/hdd/{proj}" if f"/work/hdd/{proj}" in have or proj == "<proj>" \
        else f"/projects/{proj}"
    return {
        "runs": f"/projects/{proj}/$USER/arboria/runs",
        "data": f"{nvme}/$USER/data",
        "cache": f"/projects/{proj}/$USER/arboria/cache",
        "ckpt": f"{hdd}/$USER/arboria/ckpt",
        "out": f"/projects/{proj}/$USER/arboria/out",
    }


# ------------------------------------------------------------------ TOML text

def _toml_value(v: Any) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return repr(v)
    if isinstance(v, str):
        return '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    if isinstance(v, list):
        return "[" + ", ".join(_toml_value(x) for x in v) + "]"
    raise TypeError(type(v))


def _key(k: str) -> str:
    return k if re.fullmatch(r"[A-Za-z0-9_-]+", k) else _toml_value(k)


def render_profile(name: str, values: dict[str, Any], note: str = "") -> str:
    lines = [f"# {note}"] if note else []
    lines.append(f"[{_key(name)}]")
    for k, v in values.items():
        if k not in ("partitions", "paths"):
            lines.append(f"{k} = {_toml_value(v)}")
    for pname, p in values["partitions"].items():
        lines.append(f"[{_key(name)}.partitions.{_key(pname)}]")
        lines.extend(f"{k} = {_toml_value(v)}" for k, v in p.items())
    lines.append(f"[{_key(name)}.paths]")
    lines.extend(f"{r} = {_toml_value(values['paths'][r])}" for r in PATH_ROLES)
    return "\n".join(lines) + "\n"


def _header_machine(line: str) -> str | None:
    """Machine name of a `[name]` / `[name.x]` header line, else None."""
    m = re.match(r"^\s*\[\s*(\"[^\"]*\"|[A-Za-z0-9_-]+)", line)
    if not m or line.lstrip().startswith("[["):
        return None
    return m.group(1).strip('"')


def replace_table(text: str, name: str, new_block: str) -> str:
    """`text` with machine `name`'s tables (and comments just above them) replaced."""
    lines = text.splitlines()
    out: list[str] = []
    inside = False
    placed = False
    for line in lines:
        machine = _header_machine(line)
        if machine is not None:
            inside = machine == name
            if inside:
                if not placed:
                    while out and out[-1].lstrip().startswith("#"):
                        out.pop()
                    out.append(new_block.rstrip("\n"))
                    placed = True
                continue
        if not inside:
            out.append(line)
    if not placed:
        if out and out[-1].strip():
            out.append("")
        out.append(new_block.rstrip("\n"))
    return "\n".join(out).rstrip("\n") + "\n"


# ------------------------------------------------------------------ commands

def _session(host: str, runner=None) -> SshSession:
    if not clusters.HOST_RE.fullmatch(host or ""):
        raise ArbError(f"invalid ssh host alias {host!r}")
    return SshSession(host, runner)


def ssh_setup_problems(cfg: dict[str, str]) -> list[str]:
    problems = []
    if cfg.get("controlmaster", "no") not in ("auto", "yes", "autoask", "ask"):
        problems.append("ControlMaster is off")
    if not cfg.get("controlpath") or cfg.get("controlpath") == "none":
        problems.append("no ControlPath")
    if cfg.get("controlpersist", "no") in ("no", "false", "0"):
        problems.append("ControlPersist is off (each command would need a new Duo login)")
    return problems


def probe(session: SshSession) -> dict[str, str]:
    """One fixed read-only script on the login node. NeedLogin if no master."""
    proc = session.remote("bash -s", input=PROBE_SCRIPT)
    sections = split_sections(proc.stdout)
    if "end" not in sections:
        raise ArbError("the probe script did not finish; output:\n" + proc.stdout[-2000:])
    return sections


@dataclass
class InitResult:
    name: str
    text: str
    proposal: Proposal
    path: Path
    written: bool
    backup: Path | None
    ssh_problems: list[str]
    still_invalid: str | None


def machine_init(name: str, *, host: str | None = None, account: str | None = None,
                 write: bool = False, replace: bool = False, runner=None,
                 now: Callable[[], datetime] | None = None) -> InitResult:
    if not clusters.PROFILE_NAME_RE.fullmatch(name) or "." in name:
        raise ArbError(f"invalid machine name {name!r} (letters, digits, _ and -)")
    path = clusters.config_path()
    raw_all = clusters.load_all_raw(path) if path.is_file() else {}
    existing = raw_all.get(name) if isinstance(raw_all.get(name), dict) else None
    if existing and existing.get("kind", "slurm") != "slurm":
        raise ArbError(f"machine {name!r} in {path} is kind = {existing.get('kind')!r}, not slurm")
    host = host or (existing or {}).get("host") or name
    session = _session(host, runner)
    ssh_problems = ssh_setup_problems(session.local_config())
    old = dict(existing or {})
    old.pop("charge_cmd", None)
    proposal = propose(name, host, probe(session), account=account, existing=old)
    when = (now or datetime.now)()
    stamp = when.strftime("%Y-%m-%d")
    text = render_profile(name, proposal.values, f"measured by `arb machine init {name}` {stamp}")

    written, backup = False, None
    if write:
        if existing is not None and not replace:
            raise ArbError(f"{path} already has [{name}]; add --replace to rewrite it "
                           "(a backup is kept), or copy the values above by hand")
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.is_file():
            backup = path.with_name(f"{path.name}.bak-{when.strftime('%Y%m%dT%H%M%S')}")
            shutil.copy2(path, backup)
            new_text = replace_table(path.read_text(), name, text)
        else:
            new_text = ("# Arboria cluster profiles (see examples/clusters.toml in arboria).\n\n"
                        + text)
        tmp = path.with_name(path.name + ".tmp")
        tmp.write_text(new_text)
        clusters.load_toml(tmp, "cluster profiles")  # still parses as TOML, or nothing changes
        tmp.replace(path)
        written = True
    still_invalid = None
    try:
        clusters.parse_profile(name, _reparse(text)[name], str(path))
    except ArbError as exc:
        still_invalid = str(exc)
    return InitResult(name, text, proposal, path, written, backup, ssh_problems, still_invalid)


def _reparse(text: str) -> dict[str, Any]:
    import tomllib

    return tomllib.loads(text)


# ------------------------------------------------------------------ doctor

@dataclass
class Check:
    level: str  # "ok" | "warn" | "fail" | "info"
    what: str


def doctor(name: str, *, project_id: str | None = None, runner=None) -> list[Check]:
    """Read-only checks of machine `name`; stops early (fail) if there is no ssh master."""
    checks: list[Check] = []
    path = clusters.config_path()
    raw_all = clusters.load_all_raw(path)
    raw = raw_all.get(name)
    if not isinstance(raw, dict):
        known = ", ".join(sorted(k for k, v in raw_all.items() if isinstance(v, dict))) or "none"
        raise ArbError(f"no machine {name!r} in {path} (known: {known}); "
                       f"run `arb machine init {name}`")
    profile = None
    try:
        profile = clusters.parse_profile(name, raw, str(path))
        checks.append(Check("ok", f"profile [{name}] in {path} is valid"))
    except ArbError as exc:
        checks.append(Check("fail", str(exc)))
    if project_id:
        checks.append(Check("info", f"project id {project_id}"))
    if (profile.kind if profile else raw.get("kind", "slurm")) == "fake":
        checks.append(Check("info", "kind = \"fake\": nothing remote to check"))
        return checks
    host = profile.host if profile else str(raw.get("host", ""))
    session = _session(host, runner)
    problems = ssh_setup_problems(session.local_config())
    if problems:
        checks.append(Check("fail", f"ssh {host}: " + "; ".join(problems)
                            + ". Add to ~/.ssh/config:\n" + SSH_BLOCK.format(host=host)))
    else:
        checks.append(Check("ok", f"ssh {host}: ControlMaster + ControlPersist configured"))
    try:
        session.gate()
    except NeedLogin:
        checks.append(Check("fail", need_login_message(host)))
        return checks
    checks.append(Check("ok", f"ssh master to {host} is up"))

    sections = probe(session)
    arch = sections.get("arch", "").strip()
    if profile and profile.arch and arch and arch != profile.arch:
        checks.append(Check("fail", f"login node is {arch}, profile says arch = {profile.arch}"))
    elif arch:
        checks.append(Check("ok", f"login node arch {arch}"))
    tools = [l.rsplit("/", 1)[-1] for l in sections.get("tools", "").splitlines()
             if l.startswith("/")]
    for t in ("sbatch", "sacct"):
        if t not in tools:
            checks.append(Check("fail", f"`{t}` not found on the login node"))
    checks.append(Check("info", "login node has: " + (", ".join(tools) or "none of the tools")))
    if profile is None:
        return checks

    accounts = dict(parse_accounts(sections.get("accounts", "")))
    if accounts and profile.account not in accounts:
        checks.append(Check("fail", f"account {profile.account} is not yours "
                                    f"(yours: {', '.join(accounts)})"))
    elif accounts:
        checks.append(Check("ok", f"account {profile.account}: balance {accounts[profile.account]}"
                                  " (hours)"))
    try:  # re-measure (the account was checked above)
        fresh = propose(name, host, sections, existing=raw).values
    except ArbError as exc:
        checks.append(Check("warn", f"could not re-measure the machine: {exc}"))
        fresh = None
    if fresh:
        for key in ("gpus_per_node", "cpus_per_gpu", "mem_per_gpu_gb", "default_partition"):
            have = getattr(profile, key)
            if have != fresh[key]:
                checks.append(Check("warn", f"{key} = {have}, measured {fresh[key]}"))
    parts = parse_partitions(sections.get("partitions", ""))
    base = parse_weights(parts.get(profile.default_partition, {}).get("TRESBillingWeights", ""))
    part_problems = 0
    for pname, part in profile.partitions.items():
        live = parts.get(pname)
        if live is None or not _allows(live, profile.account):
            checks.append(Check("fail", f"partition {pname} does not exist or does not "
                                        f"take account {profile.account}"))
            part_problems += 1
            continue
        try:
            live_max = parse_walltime(live.get("MaxTime", ""))
        except ArbError:
            live_max = None
        if live_max is not None and part.max_walltime_s > live_max:
            checks.append(Check("fail", f"partition {pname}: max_walltime "
                                        f"{walltime_text(part.max_walltime_s)} > Slurm's "
                                        f"{walltime_text(live_max)}"))
            part_problems += 1
        w = parse_weights(live.get("TRESBillingWeights", "")).get("gpu", 0.0)
        if base.get("gpu") and w and abs(w / base["gpu"] - part.charge) > 1e-6:
            checks.append(Check("warn", f"partition {pname}: charge = {part.charge}, "
                                        f"billing weights say {round(w / base['gpu'], 4)}"))
            part_problems += 1
    if not part_problems:
        checks.append(Check("ok", "partitions " + ", ".join(profile.partitions)
                            + " exist with these limits"))

    checks.extend(_path_checks(session, profile))
    for line in sections.get("quota", "").splitlines():
        if any(p in line for p in ("/projects/", "/work/")):
            checks.append(Check("info", "quota " + " ".join(line.replace("|", " ").split())))
    if not profile.env_activate:
        checks.append(Check("warn", "env_activate is empty: jobs run with the login shell's "
                                    "python (decide the environment, then fill it in)"))
    return checks


def _path_checks(session: SshSession, profile: clusters.Profile) -> list[Check]:
    """Each path, or its deepest existing parent, must be a writable directory."""
    lines = []
    for role in PATH_ROLES:
        lines.append(f't={dq(profile.paths[role])}; d=$t; '
                     'while [ ! -e "$d" ] && [ "$d" != / ]; do d=$(dirname "$d"); done; '
                     'if [ -d "$d" ] && [ -w "$d" ]; then w=1; else w=0; fi; '
                     'if [ "$d" = "$t" ]; then e=1; else e=0; fi; '
                     f'echo "{role} $w $e $d"')
    proc = session.remote("bash -s", input="\n".join(lines) + "\n")
    seen: dict[str, tuple[str, str, str]] = {}
    for line in proc.stdout.splitlines():
        parts = line.split(" ", 3)
        if len(parts) == 4 and parts[0] in PATH_ROLES:
            seen[parts[0]] = (parts[1], parts[2], parts[3])
    out = []
    for role in PATH_ROLES:
        writable, exists, at = seen.get(role, ("0", "0", "?"))
        target = profile.paths[role]
        if writable != "1":
            out.append(Check("fail", f"paths.{role} {target}: {at} is not a writable directory"))
        else:
            how = "exists" if exists == "1" else f"will be created under {at}"
            out.append(Check("ok", f"paths.{role} {target} ({how})"))
    return out
