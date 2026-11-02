"""The interface every cluster backend implements, plus shared checks."""

from __future__ import annotations

import re
from abc import ABC, abstractmethod
from pathlib import Path

from ..clusters import Profile
from ..errors import ArbError

JOB_ID_RE = re.compile(r"^[0-9]+(?:_[0-9]+)?$")
_SEGMENT_RE = re.compile(r"^[A-Za-z0-9._-]+$")


class BackendError(ArbError):
    """A remote operation failed; the ledger is left in a resumable state."""


class NeedLogin(BackendError):
    """No live ssh master connection. Never retried automatically (each try = a Duo push)."""


def need_login_message(host: str) -> str:
    return f"请先在终端运行 `ssh {host}` 完成 Duo 认证，然后重试"


def check_job_ids(job_ids: list[str]) -> list[str]:
    ids = [str(j) for j in job_ids]
    bad = [j for j in ids if not JOB_ID_RE.match(j)]
    if bad:
        raise BackendError(f"invalid Slurm job id(s): {', '.join(bad)}")
    return ids


def restrict_path(profile: Profile, path: str) -> str:
    """Only paths under the profile's runs/ or out/ dirs may be read (they reach Claude)."""
    path = path.rstrip("/") or "/"
    for role in ("runs", "out"):
        root = profile.paths[role]
        if path == root or path.startswith(root + "/"):
            rest = path[len(root):].strip("/")
            parts = rest.split("/") if rest else []
            if all(p not in (".", "..") and _SEGMENT_RE.match(p) for p in parts):
                return path
    raise ArbError(f"remote path {path!r} is outside this machine's runs/ and out/ dirs; refused")


class Backend(ABC):
    """Remote operations used by arb. Only `arb approve` may call `submit`."""

    def __init__(self, profile: Profile):
        self.profile = profile

    @abstractmethod
    def ship(self, repo_path: Path, commit: str, remote_code_dir: str) -> None:
        """Copy the snapshot commit's tree (git archive) into remote_code_dir."""

    @abstractmethod
    def submit(self, script_text: str, job_name: str, dependency_job_id: str | None) -> str:
        """sbatch the script (afterany dependency if given); return the job id."""

    @abstractmethod
    def find_job(self, job_name: str) -> str | None:
        """Job id of an existing job with this name (for idempotent resubmission)."""

    @abstractmethod
    def poll(self, job_ids: list[str]) -> dict[str, tuple[str, int, float | None]]:
        """{job_id: (state, elapsed_s, billed_su or None)} for the jobs Slurm knows about."""

    @abstractmethod
    def cancel(self, job_ids: list[str]) -> None: ...

    @abstractmethod
    def tail(self, remote_path: str, n: int) -> str: ...

    @abstractmethod
    def list_dir(self, remote_path: str) -> list[str]: ...

    @abstractmethod
    def fetch(self, remote_dir: str, local_dir: Path) -> None:
        """Copy remote_dir's contents into local_dir (a missing remote dir copies nothing)."""

    @abstractmethod
    def balance(self) -> str | None: ...


# Slurm state (as printed by sacct) -> jobs.state
_SLURM_STATES = {
    "PENDING": "pending", "REQUEUED": "pending", "REQUEUE_HOLD": "pending",
    "REQUEUE_FED": "pending", "PREEMPTED": "pending", "RESV_DEL_HOLD": "pending",
    "RUNNING": "running", "COMPLETING": "running", "CONFIGURING": "running",
    "SUSPENDED": "running", "STAGE_OUT": "running", "SIGNALING": "running", "RESIZING": "running",
    "COMPLETED": "completed",
    "FAILED": "failed", "NODE_FAIL": "failed", "OUT_OF_MEMORY": "failed", "BOOT_FAIL": "failed",
    "DEADLINE": "failed", "SPECIAL_EXIT": "failed",
    "CANCELLED": "cancelled", "REVOKED": "cancelled",
    "TIMEOUT": "timeout",
}


def map_slurm_state(raw: str) -> str:
    word = (raw.strip().split() or [""])[0].rstrip("+").upper()
    return _SLURM_STATES.get(word, "unknown")
