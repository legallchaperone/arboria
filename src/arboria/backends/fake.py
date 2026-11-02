"""FakeSlurm: a deterministic local stand-in for a Slurm cluster (kind = "fake").

State lives in `<profile root>/state.json`; "remote" paths are local paths (with
`$USER` expanded). Time only moves when a test calls `advance()`. Outcomes are
scripted per job name with `script()`.
"""

from __future__ import annotations

import fcntl
import getpass
import io
import json
import re
import shutil
import subprocess
import tarfile
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator

from ..clusters import Profile
from ..slurm import script_path
from .base import Backend, BackendError, NeedLogin, check_job_ids, need_login_message, restrict_path

FIRST_JOB_ID = 1001
TERMINAL = ("completed", "failed", "cancelled", "timeout")
DEFAULT_SCENARIO = {"state": "completed", "run_s": 60, "queue_s": 0, "elapsed": None,
                    "billed": None, "log": "", "out_files": {}, "lose_reply": False,
                    "submit_error": False}
_USER_RE = re.compile(r"\$\{USER\}|\$USER(?![A-Za-z0-9_])")


def local_path(path: str) -> Path:
    return Path(_USER_RE.sub(getpass.getuser(), path))


class FakeSlurm(Backend):
    def __init__(self, profile: Profile, project: str):
        super().__init__(profile, project)
        self.root = Path(profile.root or "")
        self.state_file = self.root / "state.json"

    # ------------------------------------------------------------ state

    @contextmanager
    def _state(self) -> Iterator[dict[str, Any]]:
        self.root.mkdir(parents=True, exist_ok=True)
        with open(self.root / "state.lock", "a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            state = (json.loads(self.state_file.read_text()) if self.state_file.exists()
                     else {"clock": 0, "next_id": FIRST_JOB_ID, "jobs": {}, "scenarios": {},
                           "calls": [], "need_login": False})
            try:
                yield state
            finally:  # persist even when a scripted failure is raised
                tmp = self.state_file.with_suffix(".tmp")
                tmp.write_text(json.dumps(state, indent=1, sort_keys=True))
                tmp.replace(self.state_file)

    def _remote(self, state: dict[str, Any], method: str, *args: Any) -> None:
        state["calls"].append([method, *args])
        if state["need_login"]:
            raise NeedLogin(need_login_message(self.profile.host or self.profile.name))
        self._tick(state)

    # ------------------------------------------------------------ test helpers

    def script(self, job_name: str, **outcome: Any) -> None:
        """Script a job's outcome: state, run_s, queue_s, elapsed, billed, log, out_files,
        lose_reply (sbatch succeeds but the reply is lost), submit_error (sbatch fails once)."""
        unknown = set(outcome) - set(DEFAULT_SCENARIO)
        if unknown:
            raise ValueError(f"unknown outcome keys: {unknown}")
        with self._state() as st:
            st["scenarios"][job_name] = DEFAULT_SCENARIO | st["scenarios"].get(job_name, {}) | outcome

    def advance(self, seconds: int) -> None:
        with self._state() as st:
            st["clock"] += int(seconds)
            self._tick(st)

    def set_need_login(self, value: bool = True) -> None:
        with self._state() as st:
            st["need_login"] = value

    def calls(self, method: str | None = None) -> list[list[Any]]:
        with self._state() as st:
            return [c for c in st["calls"] if method is None or c[0] == method]

    def jobs(self) -> dict[str, dict[str, Any]]:
        with self._state() as st:
            self._tick(st)
            return st["jobs"]

    # ------------------------------------------------------------ simulation

    def _scenario(self, st: dict[str, Any], name: str) -> dict[str, Any]:
        return DEFAULT_SCENARIO | st["scenarios"].get(name, {})

    def _tick(self, st: dict[str, Any]) -> None:
        changed = True
        while changed:
            changed = False
            for jid in sorted(st["jobs"], key=int):
                job = st["jobs"][jid]
                sc = self._scenario(st, job["name"])
                if job["state"] == "pending":
                    dep = st["jobs"].get(job["dependency"] or "")
                    if dep is not None and dep["state"] not in TERMINAL:
                        continue
                    ready = max(job["submit_t"], dep["end_t"] if dep else 0) + sc["queue_s"]
                    if st["clock"] >= ready:
                        job.update(state="running", start_t=ready)
                        self._write_log(job, sc["log"])
                        changed = True
                if job["state"] == "running" and st["clock"] >= job["start_t"] + sc["run_s"]:
                    job.update(state=sc["state"], end_t=job["start_t"] + sc["run_s"],
                               elapsed=sc["elapsed"] if sc["elapsed"] is not None else sc["run_s"])
                    for rel, text in (sc["out_files"] if job["out_dir"] else {}).items():
                        target = local_path(job["out_dir"]) / rel
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_text(text)
                    changed = True

    def _write_log(self, job: dict[str, Any], text: str) -> None:
        if not job["log"]:
            return
        path = local_path(job["log"].replace("%j", job["id"]).replace("%u", getpass.getuser()))
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "a", encoding="utf-8") as fh:
            fh.write(text)

    # ------------------------------------------------------------ Backend

    def ship(self, repo_path: Path, commit: str, remote_code_dir: str) -> None:
        remote_code_dir = restrict_path(self.profile, self.project, remote_code_dir)
        with self._state() as st:
            self._remote(st, "ship", commit, remote_code_dir)
        proc = subprocess.run(["git", "archive", "--format=tar", commit], cwd=repo_path,
                              capture_output=True)
        if proc.returncode != 0:
            raise BackendError(f"git archive failed: {proc.stderr.decode(errors='replace')}")
        dest = local_path(remote_code_dir)
        dest.mkdir(parents=True, exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(proc.stdout)) as tar:
            tar.extractall(dest, filter="data")

    def submit(self, script_text: str, job_name: str, dependency_job_id: str | None) -> str:
        run_id, segment = self.own_job(job_name)
        with self._state() as st:
            self._remote(st, "submit", job_name, dependency_job_id)
            sc = st["scenarios"].setdefault(job_name, {})
            if sc.get("submit_error"):
                sc["submit_error"] = False
                raise BackendError(f"sbatch failed for {job_name} (scripted)")
            if dependency_job_id is not None and dependency_job_id not in st["jobs"]:
                raise BackendError(f"sbatch: invalid dependency {dependency_job_id}")
            jid = str(st["next_id"])
            st["next_id"] += 1
            target = local_path(script_path(self.profile.paths, self.project, run_id, segment))
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(script_text)
            log = re.search(r"^#SBATCH --output=(.+)$", script_text, re.M)
            out = re.search(r'^export ARBORIA_OUT="(.+)"$', script_text, re.M)
            st["jobs"][jid] = {
                "id": jid, "name": job_name, "dependency": dependency_job_id, "state": "pending",
                "submit_t": st["clock"], "start_t": None, "end_t": None, "elapsed": 0,
                "log": log.group(1) if log else "", "out_dir": out.group(1) if out else "",
                "script": script_text,
            }
            self._tick(st)
            if sc.get("lose_reply"):
                sc["lose_reply"] = False
                raise BackendError(f"connection lost after sbatch {job_name} (scripted)")
            return jid

    def find_job(self, job_name: str) -> str | None:
        self.own_job(job_name)
        with self._state() as st:
            self._remote(st, "find_job", job_name)
            ids = [j["id"] for j in st["jobs"].values() if j["name"] == job_name]
            return max(ids, key=int) if ids else None

    def poll(self, job_ids: list[str]) -> dict[str, tuple[str, int, float | None]]:
        ids = check_job_ids(job_ids)
        with self._state() as st:
            self._remote(st, "poll", ids)
            out: dict[str, tuple[str, int, float | None]] = {}
            for jid in ids:
                job = st["jobs"].get(jid)
                if job is None:
                    continue
                sc = self._scenario(st, job["name"])
                if job["state"] == "running":
                    elapsed = st["clock"] - job["start_t"]
                else:
                    elapsed = job["elapsed"] or 0
                billed = sc["billed"] if job["state"] in TERMINAL else None
                out[jid] = (job["state"], int(elapsed), billed)
            return out

    def cancel(self, job_ids: list[str]) -> None:
        ids = check_job_ids(job_ids)
        with self._state() as st:
            self._remote(st, "cancel", ids)
            for jid in ids:
                job = st["jobs"].get(jid)
                if job and job["state"] not in TERMINAL:
                    if job["state"] == "running":
                        job["elapsed"] = st["clock"] - job["start_t"]
                    job.update(state="cancelled", end_t=st["clock"])
            self._tick(st)

    def tail(self, remote_path: str, n: int) -> str:
        path = restrict_path(self.profile, self.project, remote_path)
        with self._state() as st:
            self._remote(st, "tail", path, n)
        p = local_path(path)
        if not p.is_file():
            raise BackendError(f"tail: {path}: no such file")
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
        return "\n".join(lines[-n:]) + ("\n" if lines else "")

    def list_dir(self, remote_path: str) -> list[str]:
        path = restrict_path(self.profile, self.project, remote_path)
        with self._state() as st:
            self._remote(st, "list_dir", path)
        p = local_path(path)
        if not p.is_dir():
            raise BackendError(f"ls: {path}: no such directory")
        return sorted(x.name for x in p.iterdir())

    def fetch(self, remote_dir: str, local_dir: Path) -> None:
        path = restrict_path(self.profile, self.project, remote_dir)
        with self._state() as st:
            self._remote(st, "fetch", path)
        src = local_path(path)
        local_dir.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            shutil.copytree(src, local_dir, dirs_exist_ok=True)

    def balance(self) -> str | None:
        with self._state() as st:
            self._remote(st, "balance")
        return "fake cluster: unlimited"
