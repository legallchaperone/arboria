"""SshSlurm: a real Slurm cluster reached through an ssh ControlMaster connection.

Every remote call first runs `ssh -O check <host>` and gives up with NeedLogin if no
master connection is alive. Remote commands run with `-o BatchMode=yes` (no password
or keyboard-interactive prompt, hence never a Duo push) and `-o ControlMaster=no`
(only ever reuse the user's master). Nothing is retried automatically.
"""

from __future__ import annotations

import re
import shlex
import shutil
import subprocess
import tempfile
from pathlib import Path

from ..clusters import Profile
from ..slurm import dq, log_pattern, script_path
from .base import (
    Backend,
    BackendError,
    NeedLogin,
    check_job_ids,
    map_slurm_state,
    need_login_message,
    restrict_path,
)

SSH_OPTS = ["-o", "BatchMode=yes", "-o", "ControlMaster=no"]
TIMEOUT_S = 120
SHIP_TIMEOUT_S = 1800
FETCH_TIMEOUT_S = 3600
FIND_WINDOW = "now-30days"
_ABS_PATH_RE = re.compile(r"^/[A-Za-z0-9._/-]*$")
_CHARGE_RE = re.compile(r"(?i)(?:charge[ds]?|\bsu\b)\D{0,40}?([0-9]+(?:\.[0-9]+)?)"
                        r"|([0-9]+(?:\.[0-9]+)?)\s*SUs?\b")


class Runner:
    """The only place that starts processes (tests replace it)."""

    def run(self, argv: list[str], *, input: str | None = None,
            timeout: float = TIMEOUT_S) -> subprocess.CompletedProcess[str]:
        try:
            return subprocess.run(argv, input=input, capture_output=True, text=True,
                                  timeout=timeout,
                                  stdin=None if input is not None else subprocess.DEVNULL)
        except FileNotFoundError as exc:
            raise BackendError(f"cannot run {argv[0]}: {exc}") from exc
        except subprocess.TimeoutExpired as exc:
            raise BackendError(f"`{shlex.join(argv)}` timed out after {timeout:.0f}s") from exc

    def pipe(self, producer: list[str], cwd: Path, consumer: list[str],
             timeout: float = SHIP_TIMEOUT_S) -> tuple[int, int, str]:
        """`producer | consumer`; returns (producer rc, consumer rc, stderr of both)."""
        try:
            with subprocess.Popen(producer, cwd=cwd, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, stdin=subprocess.DEVNULL) as p1:
                p2 = subprocess.run(consumer, stdin=p1.stdout, capture_output=True, text=True,
                                    timeout=timeout)
                p1.stdout.close()
                err1 = p1.stderr.read().decode(errors="replace")
                rc1 = p1.wait(timeout=timeout)
        except FileNotFoundError as exc:
            raise BackendError(f"cannot run: {exc}") from exc
        except subprocess.TimeoutExpired as exc:
            raise BackendError(f"`{shlex.join(consumer)}` timed out") from exc
        return rc1, p2.returncode, (err1 + p2.stderr).strip()


def parse_charge(text: str) -> float | None:
    """The SU charged, if `charge_cmd` printed exactly one recognisable amount."""
    values = {float(a or b) for a, b in _CHARGE_RE.findall(text or "")}
    return values.pop() if len(values) == 1 else None


class SshSlurm(Backend):
    def __init__(self, profile: Profile, project: str, runner: Runner | None = None):
        super().__init__(profile, project)
        self.host = profile.host
        self.runner = runner or Runner()

    # ------------------------------------------------------------ plumbing

    def ssh_argv(self, remote_cmd: str) -> list[str]:
        return ["ssh", *SSH_OPTS, self.host, remote_cmd]

    def gate(self) -> None:
        """Refuse to go on without a live master connection (no retry, no prompt)."""
        proc = self.runner.run(["ssh", "-O", "check", self.host], timeout=30)
        if proc.returncode != 0:
            raise NeedLogin(need_login_message(self.host))

    def _remote(self, remote_cmd: str, *, input: str | None = None, ok_codes: tuple = (0,),
                timeout: float = TIMEOUT_S) -> subprocess.CompletedProcess[str]:
        self.gate()
        proc = self.runner.run(self.ssh_argv(remote_cmd), input=input, timeout=timeout)
        if proc.returncode not in ok_codes:
            detail = (proc.stderr or proc.stdout or "").strip().splitlines()
            raise BackendError(f"`{remote_cmd}` on {self.host} failed (exit {proc.returncode})"
                               + (f": {detail[-1]}" if detail else ""))
        return proc

    # ------------------------------------------------------------ Backend

    def ship(self, repo_path: Path, commit: str, remote_code_dir: str) -> None:
        if not re.fullmatch(r"[0-9a-f]{7,64}", commit):
            raise BackendError(f"invalid commit {commit!r}")
        d = dq(restrict_path(self.profile, self.project, remote_code_dir))
        self.gate()
        rc1, rc2, err = self.runner.pipe(
            ["git", "archive", "--format=tar", commit], Path(repo_path),
            self.ssh_argv(f"mkdir -p {d} && tar -x -C {d}"),
        )
        if rc1 != 0 or rc2 != 0:
            raise BackendError(f"shipping {commit[:12]} to {self.host}:{remote_code_dir} failed"
                               + (f": {err.splitlines()[-1]}" if err else ""))

    def submit(self, script_text: str, job_name: str, dependency_job_id: str | None) -> str:
        run_id, segment = self.own_job(job_name)
        paths = self.profile.paths
        script = script_path(paths, self.project, run_id, segment)
        sbatch_dir = script.rsplit("/", 1)[0]
        logs_dir = log_pattern(paths, self.project, run_id, segment).rsplit("/", 1)[0]
        dep = ""
        if dependency_job_id is not None:
            dep = f" --dependency=afterany:{check_job_ids([dependency_job_id])[0]}"
        cmd = (f"mkdir -p {dq(sbatch_dir)} {dq(logs_dir)} && cat > {dq(script)} && "
               f"sbatch --parsable{dep} {dq(script)}")
        proc = self._remote(cmd, input=script_text)
        lines = [l.strip() for l in proc.stdout.splitlines() if l.strip()]
        job_id = lines[-1].split(";")[0] if lines else ""
        if not re.fullmatch(r"[0-9]+", job_id):
            raise BackendError(f"sbatch printed no job id for {job_name}: {proc.stdout.strip()!r}")
        return job_id

    def find_job(self, job_name: str) -> str | None:
        """Only the user's own jobs whose name is exactly `job_name` (re-checked locally)."""
        self.own_job(job_name)
        queued = self._ids(self._remote(
            f"squeue --name={job_name} -u \"$USER\" -h -o '%i %j'").stdout, " ", job_name)
        ids = queued or self._ids(self._remote(
            f"sacct --name={job_name} -n -X -P -o JobID,JobName "
            f"--starttime={FIND_WINDOW}").stdout, "|", job_name)
        if len(set(ids)) > 1:
            raise BackendError(f"several jobs named {job_name} ({', '.join(sorted(set(ids)))}); "
                               "cancel the extras by hand before resuming")
        return ids[0] if ids else None

    @staticmethod
    def _ids(text: str, sep: str, job_name: str) -> list[str]:
        """Job ids from `<id><sep><name>` lines whose name is exactly `job_name`."""
        ids = []
        for line in text.splitlines():
            jid, _, name = line.strip().partition(sep)
            if jid and name.strip() == job_name:
                ids.append(jid.strip())
        return check_job_ids(ids)

    def poll(self, job_ids: list[str]) -> dict[str, tuple[str, int, float | None]]:
        ids = check_job_ids(job_ids)
        if not ids:
            return {}
        proc = self._remote(f"sacct -j {','.join(ids)} -n -X -P -o JobID,State,ElapsedRaw")
        out: dict[str, tuple[str, int, float | None]] = {}
        for line in proc.stdout.splitlines():
            parts = line.strip().split("|")
            if len(parts) < 3 or parts[0] not in ids:
                continue
            state = map_slurm_state(parts[1])
            elapsed = int(parts[2]) if parts[2].strip().isdigit() else 0
            out[parts[0]] = (state, elapsed, None)
        if self.profile.charge_cmd:
            for jid, (state, elapsed, _) in list(out.items()):
                if state in ("completed", "failed", "cancelled", "timeout"):
                    out[jid] = (state, elapsed, self._charge(jid))
        return out

    def _charge(self, job_id: str) -> float | None:
        try:
            proc = self._remote(f"{self.profile.charge_cmd} {job_id}")
        except NeedLogin:
            raise
        except BackendError:
            return None  # fall back to the local estimate
        return parse_charge(proc.stdout)

    def cancel(self, job_ids: list[str]) -> None:
        ids = check_job_ids(job_ids)
        if ids:
            self._remote("scancel " + " ".join(ids))

    def tail(self, remote_path: str, n: int) -> str:
        path = restrict_path(self.profile, self.project, remote_path)
        return self._remote(f"tail -n {int(n)} {dq(path)}").stdout

    def list_dir(self, remote_path: str) -> list[str]:
        path = restrict_path(self.profile, self.project, remote_path)
        return [l for l in self._remote(f"ls -1 {dq(path)}").stdout.splitlines() if l]

    def fetch(self, remote_dir: str, local_dir: Path) -> None:
        path = restrict_path(self.profile, self.project, remote_dir)
        resolved = self._remote(
            f"if [ -d {dq(path)} ]; then cd {dq(path)} && pwd -P; fi").stdout.strip()
        local_dir = Path(local_dir)
        local_dir.mkdir(parents=True, exist_ok=True)
        if not resolved:
            return  # the run wrote nothing to ARBORIA_OUT
        if not _ABS_PATH_RE.match(resolved):
            raise BackendError(f"unexpected remote path {resolved!r}")
        ssh_cmd = shlex.join(["ssh", *SSH_OPTS])
        if shutil.which("rsync"):
            argv = ["rsync", "-a", "-e", ssh_cmd, f"{self.host}:{resolved}/", f"{local_dir}/"]
            proc = self.runner.run(argv, timeout=FETCH_TIMEOUT_S)
            if proc.returncode != 0:
                raise BackendError(f"rsync from {self.host}:{resolved} failed: "
                                   f"{proc.stderr.strip()}")
            return
        with tempfile.TemporaryDirectory(prefix="arboria-fetch-", dir=local_dir.parent) as tmp:
            argv = ["scp", "-r", "-q", *SSH_OPTS, f"{self.host}:{resolved}", tmp]
            proc = self.runner.run(argv, timeout=FETCH_TIMEOUT_S)
            if proc.returncode != 0:
                raise BackendError(f"scp from {self.host}:{resolved} failed: {proc.stderr.strip()}")
            got = Path(tmp) / resolved.rsplit("/", 1)[-1]
            if got.is_dir():
                shutil.copytree(got, local_dir, dirs_exist_ok=True)

    def balance(self) -> str | None:
        if not self.profile.balance_cmd:
            return None
        return self._remote(self.profile.balance_cmd).stdout
