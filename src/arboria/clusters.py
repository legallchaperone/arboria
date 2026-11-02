"""Cluster profiles: `~/.config/arboria/clusters.toml` (or `$ARBORIA_CONFIG_DIR/clusters.toml`).

Each top-level table is one machine. Values marked TO CONFIRM in the example are
measured on the real cluster at `arb machine init`; until then they fail validation.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .config import load_toml
from .errors import ArbError
from .slurm import parse_walltime

FILENAME = "clusters.toml"
KINDS = ("slurm", "fake")
PATH_ROLES = ("runs", "data", "cache", "ckpt", "out")
TOP_KEYS = {
    "kind", "host", "arch", "account", "gpus_per_node", "cpus_per_gpu", "mem_per_gpu_gb",
    "default_partition", "max_segments", "balance_cmd", "charge_cmd", "prelude", "env_activate",
    "partitions", "paths", "root",
}
PARTITION_KEYS = {"max_walltime", "charge"}
HOST_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
PROFILE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
PARTITION_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")
# Remote paths: absolute, plain characters, and `$USER` / `${USER}` as the only variable.
# They are written into shell commands inside double quotes and into #SBATCH lines
# (where `$USER` becomes `%u`), so nothing else may be special.
_PATH_SEGMENT_RE = re.compile(r"^[A-Za-z0-9._-]+$")
_USER_VAR_RE = re.compile(r"\$\{USER\}|\$USER(?![A-Za-z0-9_])")


def config_dir() -> Path:
    env = os.environ.get("ARBORIA_CONFIG_DIR")
    return Path(env) if env else Path.home() / ".config" / "arboria"


def config_path() -> Path:
    return config_dir() / FILENAME


@dataclass(frozen=True)
class Partition:
    name: str
    max_walltime_s: int
    charge: float


@dataclass(frozen=True)
class Profile:
    name: str
    kind: str
    host: str
    arch: str
    account: str
    gpus_per_node: int
    cpus_per_gpu: int
    mem_per_gpu_gb: float
    default_partition: str
    max_segments: int
    balance_cmd: str
    charge_cmd: str
    prelude: tuple[str, ...]
    env_activate: tuple[str, ...]
    partitions: dict[str, Partition]
    paths: dict[str, str]
    root: str | None = None  # FakeSlurm state dir (kind = "fake" only)
    source: str = field(default="", compare=False)

    def partition(self, name: str | None) -> Partition:
        name = name or self.default_partition
        if name not in self.partitions:
            raise ArbError(f"machine {self.name!r} has no partition {name!r}; "
                           f"known: {', '.join(sorted(self.partitions))}")
        return self.partitions[name]

    @property
    def account_set(self) -> bool:
        return bool(self.account) and "<" not in self.account and ">" not in self.account


def check_remote_path(value: Any, where: str) -> str:
    """Validate a remote absolute path from the profile (see module notes)."""
    if not isinstance(value, str) or not value:
        raise ArbError(f"{where} must be a non-empty string")
    if "<" in value or ">" in value:
        raise ArbError(f"{where} = {value!r} still has a <placeholder>; fill it in "
                       "(TO CONFIRM at machine init)")
    if not value.startswith("/"):
        raise ArbError(f"{where} = {value!r} must be an absolute path")
    plain = _USER_VAR_RE.sub("u", value)
    parts = [p for p in plain.split("/") if p]
    if not parts or any(p in (".", "..") or not _PATH_SEGMENT_RE.match(p) for p in parts):
        raise ArbError(f"{where} = {value!r}: use only letters, digits, '.', '_', '-', '/' "
                       "and $USER")
    return value.rstrip("/")


def _int(value: Any, where: str, minimum: int, hint: str = "") -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise ArbError(f"{where} must be an integer >= {minimum}{hint}")
    return value


def _lines(value: Any, where: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
        raise ArbError(f"{where} must be a list of strings (shell lines)")
    if any("\n" in v or "\r" in v for v in value):
        raise ArbError(f"{where}: put one shell line per list item")
    return tuple(value)


def parse_profile(name: str, raw: Any, source: str = FILENAME) -> Profile:
    """Validate one `[name]` table; raises ArbError naming the offending key."""
    w = f"{source}: [{name}]"
    if not isinstance(raw, dict):
        raise ArbError(f"{w} must be a table")
    if not PROFILE_NAME_RE.match(name):
        raise ArbError(f"{w}: invalid machine name")
    unknown = sorted(set(raw) - TOP_KEYS)
    if unknown:
        raise ArbError(f"{w}: unknown key(s) {', '.join(unknown)}")
    kind = raw.get("kind", "slurm")
    if kind not in KINDS:
        raise ArbError(f"{w} kind must be one of: {', '.join(KINDS)}")
    host = raw.get("host", "")
    if kind == "slurm" and (not isinstance(host, str) or not HOST_RE.match(host)):
        raise ArbError(f"{w} host must be an ~/.ssh/config alias (letters, digits, . _ -)")
    root = raw.get("root")
    if kind == "fake":
        if not isinstance(root, str) or not root:
            raise ArbError(f"{w} root (local state dir) is required for kind = \"fake\"")
    elif root is not None:
        raise ArbError(f"{w} root is only for kind = \"fake\"")
    account = raw.get("account", "")
    if not isinstance(account, str):
        raise ArbError(f"{w} account must be a string")
    if "<" not in account and any(c.isspace() for c in account.strip()):  # <placeholder> = unset
        raise ArbError(f"{w} account must be a single word")
    confirm = " (TO CONFIRM at machine init)"
    gpus_per_node = _int(raw.get("gpus_per_node"), f"{w} gpus_per_node", 1)
    cpus_per_gpu = _int(raw.get("cpus_per_gpu"), f"{w} cpus_per_gpu", 1, confirm)
    mem = raw.get("mem_per_gpu_gb")
    if isinstance(mem, bool) or not isinstance(mem, (int, float)) or mem <= 0:
        raise ArbError(f"{w} mem_per_gpu_gb must be a number > 0{confirm}")
    max_segments = _int(raw.get("max_segments", 4), f"{w} max_segments", 1)
    for key in ("arch", "balance_cmd", "charge_cmd"):
        if not isinstance(raw.get(key, ""), str):
            raise ArbError(f"{w} {key} must be a string")

    parts_raw = raw.get("partitions")
    if not isinstance(parts_raw, dict) or not parts_raw:
        raise ArbError(f"{w}: at least one [{name}.partitions.<name>] table is required")
    partitions: dict[str, Partition] = {}
    for pname, p in parts_raw.items():
        pw = f"{source}: [{name}.partitions.{pname}]"
        if not PARTITION_RE.match(pname) or not isinstance(p, dict):
            raise ArbError(f"{pw}: invalid partition")
        bad = sorted(set(p) - PARTITION_KEYS)
        if bad:
            raise ArbError(f"{pw}: unknown key(s) {', '.join(bad)}")
        try:
            max_s = parse_walltime(p.get("max_walltime", ""))
        except ArbError as exc:
            raise ArbError(f"{pw} max_walltime: {exc}") from exc
        charge = p.get("charge", 1.0)
        if isinstance(charge, bool) or not isinstance(charge, (int, float)) or charge <= 0:
            raise ArbError(f"{pw} charge must be a number > 0")
        partitions[pname] = Partition(pname, max_s, float(charge))
    default_partition = raw.get("default_partition", "")
    if default_partition not in partitions:
        raise ArbError(f"{w} default_partition {default_partition!r} is not one of the partitions")

    paths_raw = raw.get("paths")
    if not isinstance(paths_raw, dict):
        raise ArbError(f"{w}: a [{name}.paths] table is required")
    bad = sorted(set(paths_raw) - set(PATH_ROLES))
    if bad:
        raise ArbError(f"{source}: [{name}.paths] unknown key(s) {', '.join(bad)}")
    paths = {}
    for role in PATH_ROLES:
        if role not in paths_raw:
            raise ArbError(f"{source}: [{name}.paths] {role} is required")
        paths[role] = check_remote_path(paths_raw[role], f"{source}: [{name}.paths] {role}")

    return Profile(
        name=name, kind=kind, host=host if isinstance(host, str) else "",
        arch=raw.get("arch", ""), account=account.strip(), gpus_per_node=gpus_per_node,
        cpus_per_gpu=cpus_per_gpu, mem_per_gpu_gb=float(mem), default_partition=default_partition,
        max_segments=max_segments, balance_cmd=raw.get("balance_cmd", ""),
        charge_cmd=raw.get("charge_cmd", ""),
        prelude=_lines(raw.get("prelude", []), f"{w} prelude"),
        env_activate=_lines(raw.get("env_activate", []), f"{w} env_activate"),
        partitions=partitions, paths=paths, root=root, source=source,
    )


def load_all_raw(path: Path | None = None) -> dict[str, Any]:
    path = path or config_path()
    if not path.is_file():
        raise ArbError(f"no cluster profiles: {path} does not exist (see examples/clusters.toml; "
                       "`arb machine init` will write it)")
    return load_toml(path, "cluster profiles")


def load_profile(name: str, path: Path | None = None) -> Profile:
    """Load and validate machine `name` (other profiles in the file are not validated)."""
    path = path or config_path()
    raw = load_all_raw(path)
    if name not in raw:
        known = ", ".join(sorted(k for k, v in raw.items() if isinstance(v, dict))) or "none"
        raise ArbError(f"no machine {name!r} in {path} (known: {known})")
    return parse_profile(name, raw[name], str(path))
