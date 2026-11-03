"""After approval: poll jobs, derive run status, cancel, watch (observation window), logs.

None of this ever submits anything; only `submission.approve` does.
"""

from __future__ import annotations

import json
import re
import time as time_mod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from . import backends, clusters, records, slurm
from .backends import Backend, BackendError, NeedLogin
from .db import JOB_TERMINAL, RUN_FINAL, Ledger, now
from .errors import ArbError
from .paths import RepoContext
from .config import project_config, require_project_id
from .submission import LEGACY_RUN, check_same_project, jobs_of, local_run_dir, request_of, \
    run_lock, run_project

NOTIFICATIONS = "notifications.log"
TAIL_LINES = 50
HIT_LINE_MAX = 200
FETCHED_MARKER = "fetched"
CONFIG_FILE = "config.yaml"  # the training code's resolved config, in ARBORIA_OUT
CONFIG_MAX_BYTES = 200 * 1024


# ---------------------------------------------------------------- helpers


def notify(ctx: RepoContext, message: str) -> str:
    line = f"{now()} {message}"
    with open(ctx.state_dir / NOTIFICATIONS, "a", encoding="utf-8") as fh:
        fh.write(line + "\n")
    return line


def backend_for(ctx: RepoContext, run: dict[str, Any]) -> Backend:
    """A backend bound to the current project (arboria.toml [project] id)."""
    return backends.get_backend(clusters.load_profile(run["machine"]), require_project_id(ctx))


def derive_status(states: list[str], n_segments: int, current: str) -> str:
    """Run status from its jobs' states (segment order)."""
    if any(s == "failed" for s in states):
        return "failed"
    if any(s == "cancelled" for s in states):
        return "cancelled"
    complete = len(states) == n_segments
    if complete and all(s in JOB_TERMINAL for s in states):
        return "succeeded" if states[-1] == "completed" else "failed"  # last segment timed out
    if current == "approved":  # not every segment is submitted yet; only approve moves it on
        return current
    started = any(s == "running" or s in JOB_TERMINAL for s in states)
    return "running" if started else "queued"


def _billed(request: dict[str, Any], elapsed: int | None, reported: float | None) -> float | None:
    if reported is not None:
        return round(float(reported), 4)
    if elapsed is None:
        return None
    return round(elapsed / 3600 * request["billed_gpus"] * request["charge"], 4)


@dataclass
class PollOutcome:
    run_id: str
    status: str
    previous: str
    messages: list[str] = field(default_factory=list)

    @property
    def became_final(self) -> bool:
        return self.status in RUN_FINAL and self.previous not in RUN_FINAL


def _record(ledger: Ledger, run: dict[str, Any], updates: dict[int, dict[str, Any]],
            status: str, final_payload: dict[str, Any] | None = None,
            extra_events: list[tuple[str, dict[str, Any]]] = ()) -> None:
    """One write: job rows, run status/cost; heartbeat-only changes trigger no backup."""
    run_id = run["id"]
    with ledger.write() as conn:
        for kind, payload in extra_events:
            ledger.event(kind, run_id, payload)
        for segment, fields in updates.items():
            old = conn.execute("SELECT state FROM jobs WHERE run_id = ? AND segment = ?",
                               (run_id, segment)).fetchone()
            sets = ", ".join(f"{k} = ?" for k in fields)
            conn.execute(f"UPDATE jobs SET {sets}, updated_at = ? WHERE run_id = ? AND segment = ?",
                         (*fields.values(), now(), run_id, segment))
            if "state" in fields and old and old["state"] != fields["state"]:
                ledger.event("job_state", run_id, {"segment": segment, "from": old["state"],
                                                    "to": fields["state"]})
        cost = conn.execute("SELECT SUM(billed_su) FROM jobs WHERE run_id = ?", (run_id,)).fetchone()[0]
        conn.execute("UPDATE runs SET status = ?, actual_cost = ?, updated_at = ? WHERE id = ?",
                     (status, round(cost, 4) if cost is not None else None, now(), run_id))
        if status != run["status"]:
            if status in RUN_FINAL:
                ledger.event("run_final", run_id, {"status": status, **(final_payload or {})})
            else:
                ledger.event("run_status", run_id, {"from": run["status"], "to": status})


def fetch_out(ctx: RepoContext, run: dict[str, Any], backend: Backend) -> Path:
    """Copy the run's ARBORIA_OUT into .arboria/runs/<run>/out/."""
    project = run_project(run)
    if project is None:
        raise ArbError(LEGACY_RUN.format(run=run["id"]))
    paths = request_of(run)["env"]["paths"]
    local = local_run_dir(ctx, run["id"])
    backend.fetch(slurm.out_dir(paths, project, run["id"]), local / "out")
    (local / FETCHED_MARKER).write_text(now() + "\n")
    return local / "out"


def _load_yaml(text: str) -> Any:
    """Parsed YAML, or raises ImportError when PyYAML is not installed."""
    import yaml  # optional

    return yaml.safe_load(text)


def config_json_from(path: Path) -> str | None:
    """runs.config_json for a fetched config.yaml: the parsed config when PyYAML is available
    and it is JSON-serialisable, else the raw text under `_raw_yaml` (capped)."""
    if not path.is_file():
        return None
    data = path.read_bytes()
    truncated = len(data) > CONFIG_MAX_BYTES
    text = data[:CONFIG_MAX_BYTES].decode("utf-8", errors="replace")
    if not truncated:
        try:
            return json.dumps(_load_yaml(text), ensure_ascii=False, sort_keys=True)
        except Exception:  # no PyYAML, invalid YAML, dates/sets that JSON cannot hold
            pass
    raw: dict[str, Any] = {"_raw_yaml": text}
    if truncated:
        raw |= {"_truncated": True, "_bytes": len(data)}
    return json.dumps(raw, ensure_ascii=False)


def store_config(ledger: Ledger, run_id: str, out_dir: Path) -> bool:
    """Save out/config.yaml into runs.config_json. Returns True if stored."""
    value = config_json_from(out_dir / CONFIG_FILE)
    if value is None:
        return False
    with ledger.write() as conn:
        conn.execute("UPDATE runs SET config_json = ?, updated_at = ? WHERE id = ?",
                     (value, now(), run_id))
        ledger.event("run_config", run_id, {"bytes": len(value)})
    return True


def _cancel_rest(backend: Backend, jobs: list[dict[str, Any]],
                 updates: dict[int, dict[str, Any]]) -> list[str]:
    """Cancel every job not yet terminal (afterany would start the next segment)."""
    ids = [j["job_id"] for j in jobs if j["job_id"]
           and updates.get(j["segment"], {}).get("state", j["state"]) not in JOB_TERMINAL]
    if ids:
        backend.cancel(ids)
    for j in jobs:
        if updates.get(j["segment"], {}).get("state", j["state"]) not in JOB_TERMINAL:
            updates.setdefault(j["segment"], {})["state"] = "cancelled"
    return ids


# ---------------------------------------------------------------- poll


def poll_run(ctx: RepoContext, ledger: Ledger, run_id: str, backend: Backend | None = None) -> PollOutcome:
    """One poll pass for one run. Raises NeedLogin/BackendError from the backend."""
    run = records.get_run(ledger, run_id)
    out = PollOutcome(run_id, run["status"], run["status"])
    if run["status"] in RUN_FINAL:
        if (jobs_of(ledger, run_id) and run_project(run) is not None
                and not (local_run_dir(ctx, run_id) / FETCHED_MARKER).exists()):
            backend = backend or backend_for(ctx, run)
            dest = fetch_out(ctx, run, backend)
            out.messages.append(f"{run_id}: fetched ARBORIA_OUT into {dest}")
            if store_config(ledger, run_id, dest):
                out.messages.append(f"{run_id}: saved {CONFIG_FILE} into the ledger")
        return out
    if run["status"] not in ("approved", "queued", "running"):
        return out
    with run_lock(ctx, run_id):
        jobs = [j for j in jobs_of(ledger, run_id) if j["job_id"]]
        if not jobs:
            return out
        backend = backend or backend_for(ctx, run)
        request = request_of(run)
        active = [j["job_id"] for j in jobs if j["state"] not in JOB_TERMINAL]
        result = backend.poll(active) if active else {}
        updates: dict[int, dict[str, Any]] = {}
        for j in jobs:
            if j["job_id"] not in result:
                continue
            state, elapsed, billed = result[j["job_id"]]
            updates[j["segment"]] = {"state": state, "elapsed_s": elapsed,
                                     "billed_su": _billed(request, elapsed, billed)}
        states = [updates.get(j["segment"], {}).get("state", j["state"])
                  for j in jobs_of(ledger, run_id)]
        status = derive_status(states, len(request["segments"]), run["status"])
        final_payload: dict[str, Any] = {}
        if status in ("failed", "cancelled"):
            cancelled = _cancel_rest(backend, jobs_of(ledger, run_id), updates)
            if cancelled:
                final_payload["cancelled_jobs"] = cancelled
        _record(ledger, run, updates, status, final_payload)
        out.status = status
    if out.became_final:
        run = records.get_run(ledger, run_id)
        cost = run["actual_cost"] or 0
        label = {"succeeded": "成功", "failed": "失败", "cancelled": "已取消"}[status]
        out.messages.append(notify(ctx, f"{run_id} {label}（{cost:.2f} SU）"))
        if run_project(run) is None:
            out.messages.append(f"{run_id}: ARBORIA_OUT not fetched (run predates project ids; "
                                "copy it by hand)")
            return out
        try:
            dest = fetch_out(ctx, run, backend)
            out.messages.append(f"{run_id}: fetched ARBORIA_OUT into {dest}")
            if store_config(ledger, run_id, dest):
                out.messages.append(f"{run_id}: saved {CONFIG_FILE} into the ledger")
        except ArbError as exc:
            out.messages.append(f"{run_id}: ARBORIA_OUT not fetched ({exc}); "
                                f"retry with `arb poll {run_id}`")
    return out


def active_runs(ledger: Ledger) -> list[dict[str, Any]]:
    return ledger.all("SELECT * FROM runs WHERE status IN ('approved', 'queued', 'running')"
                      " AND request_json IS NOT NULL ORDER BY id")


# ---------------------------------------------------------------- cancel / fail


def _stop(ctx: RepoContext, ledger: Ledger, run: dict[str, Any], backend: Backend, status: str,
          payload: dict[str, Any], extra_event: str | None = None) -> list[str]:
    run_id = run["id"]
    jobs = jobs_of(ledger, run_id)
    updates: dict[int, dict[str, Any]] = {}
    for j in jobs:  # adopt jobs whose sbatch reply was lost (only this project's names)
        if (not j["job_id"] and j["state"] == "submitting"
                and slurm.job_project(j["job_name"]) == backend.project):
            found = backend.find_job(j["job_name"])
            if found:
                updates[j["segment"]] = {"job_id": found}
                j["job_id"] = found
    cancelled = _cancel_rest(backend, jobs, updates)
    extra = [(extra_event, {"jobs": cancelled})] if extra_event else []
    _record(ledger, run, updates, status, payload | {"cancelled_jobs": cancelled}, extra)
    return cancelled


def cancel(ctx: RepoContext, ledger: Ledger, run_id: str) -> tuple[dict[str, Any], list[str]]:
    run = records.get_run(ledger, run_id)
    if run["status"] == "awaiting_approval":
        raise ArbError(f"run {run_id} was never approved; use `arb reject {run_id}`")
    if run["status"] not in ("approved", "queued", "running"):
        raise ArbError(f"run {run_id} is {run['status']}; nothing to cancel")
    with run_lock(ctx, run_id):
        backend = backend_for(ctx, run)
        cancelled = _stop(ctx, ledger, run, backend, "cancelled", {"by": "arb cancel"},
                          extra_event="run_cancel")
    notify(ctx, f"{run_id} 已取消（arb cancel）")
    return records.get_run(ledger, run_id), cancelled


def log_path_for(run: dict[str, Any], job: dict[str, Any]) -> str:
    project = run_project(run)
    if project is None:
        raise ArbError(LEGACY_RUN.format(run=run["id"]))
    paths = request_of(run)["env"]["paths"]
    return slurm.log_path(paths, project, run["id"], job["segment"], job["job_id"])


def first_hit(text: str, patterns: tuple[re.Pattern[str], ...]) -> tuple[str, str] | None:
    """(pattern, log line) of the first line matching any of `patterns`."""
    for line in text.splitlines():
        for pat in patterns:
            if pat.search(line):
                return pat.pattern, line.strip()[:HIT_LINE_MAX]
    return None


def observe(ctx: RepoContext, ledger: Ledger, run_id: str, backend: Backend,
            window_s: int | None = None) -> str | None:
    """Observation window: tail fresh jobs' logs; a line matching one of arboria.toml's
    [watch] fail_patterns -> cancel + fail the run, naming the pattern and the line."""
    watch_cfg = project_config(ctx).watch
    if window_s is None:
        window_s = watch_cfg.observation_minutes * 60
    run = records.get_run(ledger, run_id)
    if run["status"] not in ("queued", "running", "approved") or run_project(run) is None:
        return None
    for job in jobs_of(ledger, run_id):
        if job["state"] != "running" or not job["job_id"] or (job["elapsed_s"] or 0) > window_s:
            continue
        try:
            text = backend.tail(log_path_for(run, job), TAIL_LINES)
        except NeedLogin:
            raise
        except BackendError:
            continue  # log not created yet
        hit = first_hit(text, watch_cfg.fail_patterns)
        if hit is None:
            continue
        pattern, line = hit
        with run_lock(ctx, run_id):
            run = records.get_run(ledger, run_id)
            if run["status"] in RUN_FINAL:
                return None
            reason = f"s{job['segment']} 日志出现异常（匹配 {pattern}）：{line}"
            _stop(ctx, ledger, run, backend, "failed",
                  {"reason": reason, "by": "arb watch", "segment": job["segment"],
                   "job_id": job["job_id"], "pattern": pattern, "line": line})
        return notify(ctx, f"{run_id} 失败：{reason}，已取消剩余作业")
    return None


# ---------------------------------------------------------------- watch


def watch(ctx: RepoContext, ledger: Ledger, *, once: bool = False, interval: float = 60,
          window_s: int | None = None, echo: Callable[[str], None] = print,
          sleep: Callable[[float], None] = time_mod.sleep, max_iterations: int | None = None) -> int:
    """Poll all active runs every `interval` s. NeedLogin pauses that machine for good
    (until `arb watch` is started again): no automatic retries, one notification."""
    require_project_id(ctx)
    if window_s is None:
        window_s = project_config(ctx).watch.observation_minutes * 60
    paused: dict[str, str] = {}
    iterations = 0
    while True:
        iterations += 1
        for run in active_runs(ledger):
            machine = run["machine"]
            if machine in paused:
                continue
            try:
                backend = backend_for(ctx, run)
                outcome = poll_run(ctx, ledger, run["id"], backend)
                for m in outcome.messages:
                    echo(m)
                if outcome.status not in RUN_FINAL:
                    msg = observe(ctx, ledger, run["id"], backend, window_s)
                    if msg:
                        echo(msg)
            except NeedLogin as exc:
                paused[machine] = str(exc)
                echo(notify(ctx, f"{machine}: {exc}；watch 已暂停对 {machine} 的远程调用，"
                                 "登录后重新运行 arb watch"))
            except ArbError as exc:
                echo(f"warning: {run['id']}: {exc}")
        if once or (max_iterations is not None and iterations >= max_iterations):
            return 1 if paused else 0
        sleep(interval)


# ---------------------------------------------------------------- read-only remote views


def logs(ctx: RepoContext, ledger: Ledger, run_id: str, segment: int | None = None,
         n: int = 100) -> str:
    run = records.get_run(ledger, run_id)
    jobs = [j for j in jobs_of(ledger, run_id) if j["job_id"]]
    if not jobs:
        raise ArbError(f"run {run_id} has no submitted jobs yet")
    check_same_project(ctx, run)
    if segment is None:
        started = [j for j in jobs if j["state"] not in ("pending", "submitting")]
        job = (started or jobs)[-1]
    else:
        job = next((j for j in jobs if j["segment"] == segment), None)
        if job is None:
            raise ArbError(f"run {run_id} has no submitted segment {segment}")
    if not 1 <= n <= 10_000:
        raise ArbError("-n must be between 1 and 10000")
    return backend_for(ctx, run).tail(log_path_for(run, job), n)


_SUBPATH_RE = re.compile(r"^[A-Za-z0-9._-]+(?:/[A-Za-z0-9._-]+)*$")


def remote_ls(ctx: RepoContext, ledger: Ledger, run_id: str, subpath: str | None = None,
              out: bool = False) -> tuple[str, list[str]]:
    run = records.get_run(ledger, run_id)
    project = check_same_project(ctx, run)
    paths = request_of(run)["env"]["paths"]
    base = (slurm.out_dir(paths, project, run_id) if out
            else slurm.run_dir(paths, project, run_id))
    if subpath:
        sub = subpath.strip("/")
        if not _SUBPATH_RE.match(sub) or any(p in (".", "..") for p in sub.split("/")):
            raise ArbError(f"invalid subpath {subpath!r}")
        base = f"{base}/{sub}"
    return base, backend_for(ctx, run).list_dir(base)
