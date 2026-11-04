//! After approval: poll jobs, derive run status and cost, cancel, watch (observation
//! window), logs, remote listings. Ports `arboria.monitor`.
//!
//! Invariants:
//! - Nothing here ever submits; only [`crate::submission::approve`] does.
//! - Job-state and run-status updates are heartbeat-like (`job_state`, `run_status`
//!   events: no backup commit); `run_final`, `run_cancel`, `run_config` do commit.
//! - Status mapping and billing stay in the core (ADR 0003): SU = sacct `billing=N` ×
//!   hours / `billing_per_su`, else the local estimate.
//! - `watch` cancels a run in its observation window on a crash or a log line matching
//!   `[watch] fail_patterns`, and the notification names the pattern and the line (≤200
//!   chars). `Error::NeedLogin` pauses that machine for the pass; never retried.
//! - Remote reads are limited to the project's `runs/` and `out/` dirs (the backend's
//!   [`crate::backend::restrict_path`]); `-n` is 1..=10000; subpaths are plain segments.
//! - `ARBORIA_OUT` is fetched once into `.arboria/runs/<run>/out` (marker `fetched`);
//!   `out/config.yaml` (≤200 KiB) is stored as `runs.config_json`.
//!
//! - `config.yaml` is stored as `{"_raw_yaml": text}` (plus `_truncated` / `_bytes` over
//!   [`CONFIG_MAX_BYTES`]): what Python stores without PyYAML, which is not a dependency.
//!   Parsing it needs a YAML crate (ADR 0002), not added yet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::backend::Backend;
use crate::cards::{request_of, run_project, LEGACY_RUN};
use crate::db::{now, Ledger};
use crate::model::{EventKind, Job, JobState, Run, RunId, RunStatus};
use crate::paths::RepoContext;
use crate::submission::{check_same_project, jobs_of, local_run_dir, run_lock};
use crate::{bail, clusters, config, records, slurm, Error, Result};

/// `.arboria/notifications.log`.
pub const NOTIFICATIONS: &str = "notifications.log";
/// Log lines read per observation pass.
pub const TAIL_LINES: usize = 50;
/// A matching log line is clipped to this in notifications.
pub const HIT_LINE_MAX: usize = 200;
/// Marker file of a fetched `ARBORIA_OUT`.
pub const FETCHED_MARKER: &str = "fetched";
/// The training code's resolved config, in `ARBORIA_OUT`.
pub const CONFIG_FILE: &str = "config.yaml";
/// Larger configs are not stored.
pub const CONFIG_MAX_BYTES: u64 = 200 * 1024;

fn terminal(s: JobState) -> bool {
    JobState::TERMINAL.contains(&s)
}

fn is_final(s: RunStatus) -> bool {
    RunStatus::FINAL.contains(&s)
}

/// Append `<now> <message>` to the notifications log; returns the line.
pub fn notify(ctx: &RepoContext, message: &str) -> Result<String> {
    use std::io::Write;
    let line = format!("{} {message}", now());
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ctx.state_dir().join(NOTIFICATIONS))?;
    f.write_all(format!("{line}\n").as_bytes())?;
    Ok(line)
}

/// A backend for the run's machine, bound to the current project.
pub fn backend_for(ctx: &RepoContext, run: &Run) -> Result<Box<dyn Backend>> {
    let profile = clusters::load_profile(run.machine.as_deref().unwrap_or(""), None)?;
    crate::backend::get_backend(&profile, &config::require_project_id(ctx)?)
}

/// Run status from its jobs' states in segment order (`n_segments` planned).
pub fn derive_status(states: &[JobState], n_segments: usize, current: RunStatus) -> RunStatus {
    if states.contains(&JobState::Failed) {
        return RunStatus::Failed;
    }
    if states.contains(&JobState::Cancelled) {
        return RunStatus::Cancelled;
    }
    if states.len() == n_segments && states.iter().all(|s| terminal(*s)) {
        // the last segment timed out
        return if states.last() == Some(&JobState::Completed) {
            RunStatus::Succeeded
        } else {
            RunStatus::Failed
        };
    }
    // not every segment is submitted yet; only approve moves it on
    if current == RunStatus::Approved {
        return current;
    }
    if states
        .iter()
        .any(|s| *s == JobState::Running || terminal(*s))
    {
        RunStatus::Running
    } else {
        RunStatus::Queued
    }
}

/// `jobs.billed_su`: the reported SU rounded to 4 places, else the local estimate
/// (`elapsed / 3600 × billed_gpus × charge`, rounded), None without elapsed.
pub fn billed_su(request: &Value, elapsed: Option<u64>, reported: Option<f64>) -> Option<f64> {
    if let Some(r) = reported {
        return Some(slurm::py_round(r, 4));
    }
    let elapsed = elapsed? as f64;
    let gpus = request["billed_gpus"].as_f64().unwrap_or(0.0);
    let charge = request["charge"].as_f64().unwrap_or(0.0);
    Some(slurm::py_round(elapsed / 3600.0 * gpus * charge, 4))
}

/// What one poll of one run did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PollOutcome {
    pub run_id: RunId,
    pub status: RunStatus,
    pub previous: RunStatus,
    pub messages: Vec<String>,
}

impl PollOutcome {
    /// The run just reached a final status.
    pub fn became_final(&self) -> bool {
        RunStatus::FINAL.contains(&self.status) && !RunStatus::FINAL.contains(&self.previous)
    }
}

/// Changes to one job row.
#[derive(Debug, Clone, Default)]
struct JobUpdate {
    job_id: Option<String>,
    state: Option<JobState>,
    elapsed_s: Option<u64>,
    billed_su: Option<Option<f64>>,
}

/// The state a job has once `updates` are applied.
fn state_after(job: &Job, updates: &BTreeMap<u32, JobUpdate>) -> JobState {
    updates
        .get(&job.segment)
        .and_then(|u| u.state)
        .unwrap_or(job.state)
}

/// One write: job rows, run status and cost; heartbeat-only changes trigger no backup.
fn record(
    ledger: &mut Ledger,
    run: &Run,
    updates: &BTreeMap<u32, JobUpdate>,
    status: RunStatus,
    final_payload: Map<String, Value>,
    extra_events: &[(EventKind, Value)],
) -> Result<()> {
    let run_id = &run.id;
    ledger.write(|tx| {
        for (kind, payload) in extra_events {
            tx.event(*kind, run_id.as_str(), payload)?;
        }
        for (segment, u) in updates {
            let old: Option<String> = tx
                .query_row(
                    "SELECT state FROM jobs WHERE run_id = ?1 AND segment = ?2",
                    rusqlite::params![run_id, segment],
                    |r| r.get(0),
                )
                .ok();
            let ts = now();
            if let Some(id) = &u.job_id {
                tx.execute(
                    "UPDATE jobs SET job_id = ?1, updated_at = ?2 WHERE run_id = ?3 AND segment = ?4",
                    rusqlite::params![id, ts, run_id, segment],
                )?;
            }
            if let Some(state) = u.state {
                tx.execute(
                    "UPDATE jobs SET state = ?1, updated_at = ?2 WHERE run_id = ?3 AND segment = ?4",
                    rusqlite::params![state.as_str(), ts, run_id, segment],
                )?;
            }
            if let Some(e) = u.elapsed_s {
                tx.execute(
                    "UPDATE jobs SET elapsed_s = ?1, updated_at = ?2 WHERE run_id = ?3 AND segment = ?4",
                    rusqlite::params![e as i64, ts, run_id, segment],
                )?;
            }
            if let Some(b) = u.billed_su {
                tx.execute(
                    "UPDATE jobs SET billed_su = ?1, updated_at = ?2 WHERE run_id = ?3 AND segment = ?4",
                    rusqlite::params![b, ts, run_id, segment],
                )?;
            }
            if let (Some(new), Some(old)) = (u.state, old) {
                if old != new.as_str() {
                    tx.event(
                        EventKind::JobState,
                        run_id.as_str(),
                        &json!({"segment": segment, "from": old, "to": new.as_str()}),
                    )?;
                }
            }
        }
        let cost: Option<f64> = tx.query_row(
            "SELECT SUM(billed_su) FROM jobs WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE runs SET status = ?1, actual_cost = ?2, updated_at = ?3 WHERE id = ?4",
            rusqlite::params![
                status.as_str(),
                cost.map(|c| slurm::py_round(c, 4)),
                now(),
                run_id
            ],
        )?;
        if status != run.status {
            if is_final(status) {
                let mut payload = Map::new();
                payload.insert("status".into(), status.as_str().into());
                payload.extend(final_payload);
                tx.event(EventKind::RunFinal, run_id.as_str(), &Value::Object(payload))?;
            } else {
                tx.event(
                    EventKind::RunStatus,
                    run_id.as_str(),
                    &json!({"from": run.status.as_str(), "to": status.as_str()}),
                )?;
            }
        }
        Ok(())
    })
}

fn legacy(run: &Run) -> Error {
    Error::Arb(LEGACY_RUN.replace("{run}", run.id.as_str()))
}

fn run_paths(run: &Run) -> Result<BTreeMap<String, String>> {
    Ok(serde_json::from_value(
        request_of(run)?["env"]["paths"].clone(),
    )?)
}

/// Copy the run's `ARBORIA_OUT` into `.arboria/runs/<run>/out` (once).
pub fn fetch_out(ctx: &RepoContext, run: &Run, backend: &dyn Backend) -> Result<PathBuf> {
    let project = run_project(run)?.ok_or_else(|| legacy(run))?;
    let paths = run_paths(run)?;
    let local = local_run_dir(ctx, &run.id);
    backend.fetch(
        &slurm::out_dir(&paths, &project, &run.id),
        &local.join("out"),
    )?;
    std::fs::write(local.join(FETCHED_MARKER), format!("{}\n", now()))?;
    Ok(local.join("out"))
}

/// `out/config.yaml` as the `runs.config_json` text, if present: `{"_raw_yaml": text}`
/// (with `_truncated` / `_bytes` over the cap), keys in Python's order.
pub fn config_json_from(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let data = std::fs::read(path)?;
    let cap = CONFIG_MAX_BYTES as usize;
    let truncated = data.len() > cap;
    let text = String::from_utf8_lossy(&data[..data.len().min(cap)]).into_owned();
    let mut out = String::from("{\"_raw_yaml\": ");
    crate::canonical::write_string(&mut out, &text);
    if truncated {
        out.push_str(&format!(
            ", \"_truncated\": true, \"_bytes\": {}",
            data.len()
        ));
    }
    out.push('}');
    Ok(Some(out))
}

/// Store the fetched config in `runs.config_json` (`run_config` event).
pub fn store_config(ledger: &mut Ledger, run_id: &RunId, out_dir: &Path) -> Result<bool> {
    let Some(value) = config_json_from(&out_dir.join(CONFIG_FILE))? else {
        return Ok(false);
    };
    ledger.write(|tx| {
        tx.execute(
            "UPDATE runs SET config_json = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![value, now(), run_id],
        )?;
        tx.event(
            EventKind::RunConfig,
            run_id.as_str(),
            &json!({"bytes": value.chars().count()}),
        )
    })?;
    Ok(true)
}

/// Cancel every job not yet terminal (afterany would start the next segment); marks
/// them cancelled in `updates`. Returns the cancelled job ids.
fn cancel_rest(
    backend: &dyn Backend,
    jobs: &[Job],
    updates: &mut BTreeMap<u32, JobUpdate>,
) -> Result<Vec<String>> {
    let ids: Vec<String> = jobs
        .iter()
        .filter(|j| !terminal(state_after(j, updates)))
        .filter_map(|j| {
            updates
                .get(&j.segment)
                .and_then(|u| u.job_id.clone())
                .or_else(|| j.job_id.clone())
        })
        .collect();
    if !ids.is_empty() {
        backend.cancel(&ids)?;
    }
    for j in jobs {
        if !terminal(state_after(j, updates)) {
            updates.entry(j.segment).or_default().state = Some(JobState::Cancelled);
        }
    }
    Ok(ids)
}

fn fetch_and_store(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run: &Run,
    backend: &dyn Backend,
    out: &mut PollOutcome,
) -> Result<()> {
    let dest = fetch_out(ctx, run, backend)?;
    out.messages.push(format!(
        "{}: fetched ARBORIA_OUT into {}",
        run.id,
        dest.display()
    ));
    if store_config(ledger, &run.id, &dest)? {
        out.messages
            .push(format!("{}: saved {CONFIG_FILE} into the ledger", run.id));
    }
    Ok(())
}

/// `arb poll RUN`: update job states, run status and cost; fetch `ARBORIA_OUT` when done.
pub fn poll_run(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    backend: Option<&dyn Backend>,
) -> Result<PollOutcome> {
    let run = records::get_run(ledger, run_id)?;
    let mut out = PollOutcome {
        run_id: run_id.clone(),
        status: run.status,
        previous: run.status,
        messages: Vec::new(),
    };
    let owned: Box<dyn Backend>;
    let mut backend = backend;
    if is_final(run.status) {
        if !jobs_of(ledger, run_id)?.is_empty()
            && run_project(&run)?.is_some()
            && !local_run_dir(ctx, run_id).join(FETCHED_MARKER).exists()
        {
            let b: &dyn Backend = match backend {
                Some(b) => b,
                None => {
                    owned = backend_for(ctx, &run)?;
                    owned.as_ref()
                }
            };
            fetch_and_store(ctx, ledger, &run, b, &mut out)?;
        }
        return Ok(out);
    }
    if !RunStatus::ACTIVE.contains(&run.status) {
        return Ok(out);
    }
    let owned2: Box<dyn Backend>;
    {
        let _lock = run_lock(ctx, run_id)?;
        let jobs: Vec<Job> = jobs_of(ledger, run_id)?
            .into_iter()
            .filter(|j| j.job_id.is_some())
            .collect();
        if jobs.is_empty() {
            return Ok(out);
        }
        if backend.is_none() {
            owned2 = backend_for(ctx, &run)?;
            backend = Some(owned2.as_ref());
        }
        let b = backend.expect("set above");
        let request = request_of(&run)?;
        let active: Vec<String> = jobs
            .iter()
            .filter(|j| !terminal(j.state))
            .filter_map(|j| j.job_id.clone())
            .collect();
        let result = if active.is_empty() {
            BTreeMap::new()
        } else {
            b.poll(&active)?
        };
        let mut updates: BTreeMap<u32, JobUpdate> = BTreeMap::new();
        for j in &jobs {
            let Some(st) = j.job_id.as_ref().and_then(|id| result.get(id)) else {
                continue;
            };
            updates.insert(
                j.segment,
                JobUpdate {
                    state: Some(st.state),
                    elapsed_s: Some(st.elapsed_s),
                    billed_su: Some(billed_su(&request, Some(st.elapsed_s), st.billed_su)),
                    ..Default::default()
                },
            );
        }
        let all = jobs_of(ledger, run_id)?;
        let states: Vec<JobState> = all.iter().map(|j| state_after(j, &updates)).collect();
        let n_segments = request["segments"].as_array().map_or(0, Vec::len);
        let status = derive_status(&states, n_segments, run.status);
        let mut final_payload = Map::new();
        if matches!(status, RunStatus::Failed | RunStatus::Cancelled) {
            let cancelled = cancel_rest(b, &all, &mut updates)?;
            if !cancelled.is_empty() {
                final_payload.insert("cancelled_jobs".into(), json!(cancelled));
            }
        }
        record(ledger, &run, &updates, status, final_payload, &[])?;
        out.status = status;
    }
    if out.became_final() {
        let run = records::get_run(ledger, run_id)?;
        let cost = run.actual_cost.unwrap_or(0.0);
        let label = match out.status {
            RunStatus::Succeeded => "成功",
            RunStatus::Failed => "失败",
            _ => "已取消",
        };
        out.messages
            .push(notify(ctx, &format!("{run_id} {label}（{cost:.2} SU）"))?);
        if run_project(&run)?.is_none() {
            out.messages.push(format!(
                "{run_id}: ARBORIA_OUT not fetched (run predates project ids; copy it by hand)"
            ));
            return Ok(out);
        }
        let b = backend.expect("polled with a backend");
        if let Err(e) = fetch_and_store(ctx, ledger, &run, b, &mut out) {
            if matches!(e, Error::Sqlite(_) | Error::Io(_) | Error::Json(_)) {
                return Err(e);
            }
            out.messages.push(format!(
                "{run_id}: ARBORIA_OUT not fetched ({e}); retry with `arb poll {run_id}`"
            ));
        }
    }
    Ok(out)
}

/// Runs whose jobs may still be on a machine.
pub fn active_runs(ledger: &Ledger) -> Result<Vec<Run>> {
    crate::db::all(
        ledger.conn(),
        "SELECT * FROM runs WHERE status IN ('approved', 'queued', 'running') \
         AND request_json IS NOT NULL ORDER BY id",
        [],
    )
}

/// Stop a run: adopt jobs whose sbatch reply was lost, cancel the rest, record `status`.
fn stop(
    ledger: &mut Ledger,
    run: &Run,
    backend: &dyn Backend,
    status: RunStatus,
    payload: Map<String, Value>,
    extra_event: Option<EventKind>,
) -> Result<Vec<String>> {
    let mut jobs = jobs_of(ledger, &run.id)?;
    let mut updates: BTreeMap<u32, JobUpdate> = BTreeMap::new();
    for j in &mut jobs {
        // only this project's names
        if j.job_id.is_none()
            && j.state == JobState::Submitting
            && slurm::job_project(&j.job_name).as_ref() == Some(backend.project())
        {
            if let Some(found) = backend.find_job(&j.job_name)? {
                updates.entry(j.segment).or_default().job_id = Some(found.clone());
                j.job_id = Some(found);
            }
        }
    }
    let cancelled = cancel_rest(backend, &jobs, &mut updates)?;
    let extra: Vec<(EventKind, Value)> = extra_event
        .map(|k| (k, json!({"jobs": cancelled})))
        .into_iter()
        .collect();
    let mut payload = payload;
    payload.insert("cancelled_jobs".into(), json!(cancelled));
    record(ledger, run, &updates, status, payload, &extra)?;
    Ok(cancelled)
}

/// `arb cancel`: cancel the run's jobs; returns the run and the cancelled job ids.
pub fn cancel(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
) -> Result<(Run, Vec<String>)> {
    cancel_with(ctx, ledger, run_id, None)
}

/// [`cancel`] with a given backend (tests; None = the run's machine).
pub fn cancel_with(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    backend: Option<&dyn Backend>,
) -> Result<(Run, Vec<String>)> {
    let run = records::get_run(ledger, run_id)?;
    if run.status == RunStatus::AwaitingApproval {
        bail!("run {run_id} was never approved; use `arb reject {run_id}`");
    }
    if !RunStatus::ACTIVE.contains(&run.status) {
        bail!("run {run_id} is {}; nothing to cancel", run.status);
    }
    let cancelled = {
        let _lock = run_lock(ctx, run_id)?;
        let owned;
        let b = match backend {
            Some(b) => b,
            None => {
                owned = backend_for(ctx, &run)?;
                owned.as_ref()
            }
        };
        let mut payload = Map::new();
        payload.insert("by".into(), "arb cancel".into());
        stop(
            ledger,
            &run,
            b,
            RunStatus::Cancelled,
            payload,
            Some(EventKind::RunCancel),
        )?
    };
    notify(ctx, &format!("{run_id} 已取消（arb cancel）"))?;
    Ok((records::get_run(ledger, run_id)?, cancelled))
}

fn log_path_for(run: &Run, job: &Job) -> Result<String> {
    let project = run_project(run)?.ok_or_else(|| legacy(run))?;
    Ok(slurm::log_path(
        &run_paths(run)?,
        &project,
        &run.id,
        job.segment,
        job.job_id.as_deref().unwrap_or(""),
    ))
}

/// The first `(pattern, line)` of `text` matching one of `patterns`.
pub fn first_hit(text: &str, patterns: &[String]) -> Result<Option<(String, String)>> {
    let compiled = patterns
        .iter()
        .map(|p| config::compile_fail_pattern(p).map_err(|e| Error::Arb(format!("{p}: {e}"))))
        .collect::<Result<Vec<_>>>()?;
    for line in records::py_splitlines(text) {
        for (src, re) in patterns.iter().zip(&compiled) {
            if re.is_match(line) {
                let clipped: String = records::py_strip(line).chars().take(HIT_LINE_MAX).collect();
                return Ok(Some((src.clone(), clipped)));
            }
        }
    }
    Ok(None)
}

/// One observation pass over a run: cancel on a crash or a failure pattern in its window.
pub fn observe(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    backend: &dyn Backend,
    window_minutes: u32,
) -> Result<Vec<String>> {
    let patterns = config::project_config(ctx)?.watch.fail_patterns;
    let window_s = u64::from(window_minutes) * 60;
    let run = records::get_run(ledger, run_id)?;
    if !RunStatus::ACTIVE.contains(&run.status) || run_project(&run)?.is_none() {
        return Ok(Vec::new());
    }
    for job in jobs_of(ledger, run_id)? {
        if job.state != JobState::Running
            || job.job_id.is_none()
            || job.elapsed_s.unwrap_or(0) as u64 > window_s
        {
            continue;
        }
        let text = match backend.tail(&log_path_for(&run, &job)?, TAIL_LINES) {
            Ok(t) => t,
            Err(e @ Error::NeedLogin(_)) => return Err(e),
            Err(Error::Backend(_)) => continue, // log not created yet
            Err(e) => return Err(e),
        };
        let Some((pattern, line)) = first_hit(&text, &patterns)? else {
            continue;
        };
        let reason = format!("s{} 日志出现异常（匹配 {pattern}）：{line}", job.segment);
        {
            let _lock = run_lock(ctx, run_id)?;
            let run = records::get_run(ledger, run_id)?;
            if is_final(run.status) {
                return Ok(Vec::new());
            }
            let mut payload = Map::new();
            payload.insert("reason".into(), reason.clone().into());
            payload.insert("by".into(), "arb watch".into());
            payload.insert("segment".into(), job.segment.into());
            payload.insert("job_id".into(), json!(job.job_id));
            payload.insert("pattern".into(), pattern.into());
            payload.insert("line".into(), line.into());
            stop(ledger, &run, backend, RunStatus::Failed, payload, None)?;
        }
        return Ok(vec![notify(
            ctx,
            &format!("{run_id} 失败：{reason}，已取消剩余作业"),
        )?]);
    }
    Ok(Vec::new())
}

/// Makes the backend for a run ([`watch_with`]).
pub type BackendFactory<'a> = dyn FnMut(&RepoContext, &Run) -> Result<Box<dyn Backend>> + 'a;

/// Options of `arb watch`.
#[derive(Debug, Clone, Copy)]
pub struct WatchOptions {
    pub once: bool,
    pub interval_s: u64,
    /// None: `[watch] observation_minutes`.
    pub window_minutes: Option<u32>,
    /// Tests: stop after this many passes.
    pub max_iterations: Option<u32>,
}

/// `arb watch`: poll + observe active runs until none is left (or `once`). Exit status 1
/// when a machine needed a login.
pub fn watch(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    opts: WatchOptions,
    echo: &mut dyn FnMut(&str),
) -> Result<i32> {
    watch_with(ctx, ledger, opts, echo, &mut |ctx, run| {
        backend_for(ctx, run)
    })
}

/// [`watch`] with a backend factory (tests).
pub fn watch_with(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    opts: WatchOptions,
    echo: &mut dyn FnMut(&str),
    make_backend: &mut BackendFactory<'_>,
) -> Result<i32> {
    config::require_project_id(ctx)?;
    let window = match opts.window_minutes {
        Some(w) => w,
        None => config::project_config(ctx)?.watch.observation_minutes,
    };
    let mut paused: BTreeMap<String, String> = BTreeMap::new();
    let mut iterations = 0u32;
    loop {
        iterations += 1;
        for run in active_runs(ledger)? {
            let machine = run.machine.clone().unwrap_or_default();
            if paused.contains_key(&machine) {
                continue;
            }
            let pass = (|| -> Result<Vec<String>> {
                let backend = make_backend(ctx, &run)?;
                let outcome = poll_run(ctx, ledger, &run.id, Some(backend.as_ref()))?;
                let mut msgs = outcome.messages.clone();
                if !is_final(outcome.status) {
                    msgs.extend(observe(ctx, ledger, &run.id, backend.as_ref(), window)?);
                }
                Ok(msgs)
            })();
            match pass {
                Ok(msgs) => msgs.iter().for_each(|m| echo(m)),
                Err(Error::NeedLogin(msg)) => {
                    paused.insert(machine.clone(), msg.clone());
                    echo(&notify(
                        ctx,
                        &format!(
                            "{machine}: {msg}；watch 已暂停对 {machine} 的远程调用，登录后重新运行 arb watch"
                        ),
                    )?);
                }
                Err(e @ (Error::Sqlite(_) | Error::Io(_) | Error::Json(_))) => return Err(e),
                Err(e) => echo(&format!("warning: {}: {e}", run.id)),
            }
        }
        if opts.once || opts.max_iterations.is_some_and(|m| iterations >= m) {
            return Ok(if paused.is_empty() { 0 } else { 1 });
        }
        std::thread::sleep(std::time::Duration::from_secs(opts.interval_s));
    }
}

/// `arb logs`: the last `n` lines of a segment's log (default: the latest started one).
pub fn logs(
    ctx: &RepoContext,
    ledger: &Ledger,
    run_id: &RunId,
    segment: Option<u32>,
    n: usize,
) -> Result<String> {
    let (run, job) = log_job(ctx, ledger, run_id, segment, n)?;
    backend_for(ctx, &run)?.tail(&log_path_for(&run, &job)?, n)
}

/// The run and job [`logs`] reads (all checks done).
pub fn log_job(
    ctx: &RepoContext,
    ledger: &Ledger,
    run_id: &RunId,
    segment: Option<u32>,
    n: usize,
) -> Result<(Run, Job)> {
    let run = records::get_run(ledger, run_id)?;
    let jobs: Vec<Job> = jobs_of(ledger, run_id)?
        .into_iter()
        .filter(|j| j.job_id.is_some())
        .collect();
    if jobs.is_empty() {
        bail!("run {run_id} has no submitted jobs yet");
    }
    check_same_project(ctx, &run)?;
    let job = match segment {
        None => {
            let started: Vec<&Job> = jobs
                .iter()
                .filter(|j| !matches!(j.state, JobState::Pending | JobState::Submitting))
                .collect();
            started
                .last()
                .copied()
                .unwrap_or_else(|| jobs.last().expect("non-empty"))
                .clone()
        }
        Some(k) => jobs
            .iter()
            .find(|j| j.segment == k)
            .cloned()
            .ok_or_else(|| Error::Arb(format!("run {run_id} has no submitted segment {k}")))?,
    };
    if !(1..=10_000).contains(&n) {
        bail!("-n must be between 1 and 10000");
    }
    Ok((run, job))
}

/// `arb remote ls`: `(remote dir, entries)` of the run dir or its `ARBORIA_OUT`.
pub fn remote_ls(
    ctx: &RepoContext,
    ledger: &Ledger,
    run_id: &RunId,
    subpath: Option<&str>,
    out: bool,
) -> Result<(String, Vec<String>)> {
    let (run, base) = remote_dir(ctx, ledger, run_id, subpath, out)?;
    let entries = backend_for(ctx, &run)?.list_dir(&base)?;
    Ok((base, entries))
}

/// The directory [`remote_ls`] lists (all checks done).
pub fn remote_dir(
    ctx: &RepoContext,
    ledger: &Ledger,
    run_id: &RunId,
    subpath: Option<&str>,
    out: bool,
) -> Result<(Run, String)> {
    let run = records::get_run(ledger, run_id)?;
    let project = check_same_project(ctx, &run)?;
    let paths = run_paths(&run)?;
    let mut base = if out {
        slurm::out_dir(&paths, &project, run_id)
    } else {
        slurm::run_dir(&paths, &project, run_id)
    };
    if let Some(sub) = subpath.filter(|s| !s.is_empty()) {
        let s = sub.trim_matches('/');
        let ok = !s.is_empty()
            && s.split('/').all(|p| {
                !p.is_empty()
                    && p != "."
                    && p != ".."
                    && p.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            });
        if !ok {
            bail!("invalid subpath {}", records::py_repr(sub));
        }
        base = format!("{base}/{s}");
    }
    Ok((run, base))
}
