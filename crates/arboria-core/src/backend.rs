//! The machine interface (the compute layer of ADR 0001) and its implementations: local,
//! ssh workstation, Slurm, RunPod, Modal, plus `FakeSlurm` for tests. Ports
//! `arboria.backends` (`base`, `fake`, `ssh`).
//!
//! Invariants:
//! - [`Backend::submit`] takes a [`SubmitPermit`], which only
//!   [`crate::submission`] can create, right after re-checking the ledger for the human's
//!   approval Decision. No other code path can reach a machine's submit (Python enforced
//!   this with a source scan; here it is the type system, and `tests/guards.rs` scans too).
//! - A backend is bound to one project: it only submits and looks up job names of that
//!   project, and only reads paths under that project's `runs/` and `out/` dirs
//!   ([`restrict_path`]); what it reads reaches the agent.
//! - ssh / scp are the system commands with the user's ControlMaster; `ssh -O check` gates
//!   every remote call; `BatchMode=yes`; `Error::NeedLogin` is never retried (each try is
//!   a Duo push). No network calls outside a backend.
//! - Cloud API keys come from the keychain and never reach the agent.
//! - The five built-in machines are Rust implementations of this trait, not plugins
//!   (ADR 0003); the core keeps status mapping, billing, path limits and the ssh gate.
//!   Only FakeSlurm and SshSlurm exist so far; the others (and `prepare_env`) refuse.
//! - Processes start only through a [`Runner`] (tests replace it), as in Python.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::clusters::{MachineKind, Profile};
use crate::model::{JobState, ProjectId, RunId};
use crate::records::{py_decimal, py_repr, py_splitlines, py_strip};
use crate::slurm::{dq, log_pattern, script_path, shlex_join};
use crate::submission::SubmitPermit;
use crate::{Error, Result};

/// `请先在终端运行 `ssh {host}` 完成 Duo 认证，然后重试`.
pub fn need_login_message(host: &str) -> String {
    format!("请先在终端运行 `ssh {host}` 完成 Duo 认证，然后重试")
}

/// What a machine knows about one job.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobStatus {
    pub state: JobState,
    pub elapsed_s: u64,
    /// Billed SU as reported by the machine (Slurm: `billing=N` × hours / billing_per_su).
    pub billed_su: Option<f64>,
}

/// The environment a run needs (ADR 0001: only the project's `uv.lock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvSpec {
    /// Snapshot commit holding `uv.lock`.
    pub commit: String,
    /// Hash of `uv.lock` (environments are cached by it).
    pub lock_hash: String,
    pub arch: String,
}

/// What preparing an environment found or did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvReport {
    /// An environment for this lock already existed.
    pub reused: bool,
    /// Where it is on the machine.
    pub location: String,
    pub messages: Vec<String>,
}

/// One machine, bound to one project.
pub trait Backend {
    /// The machine kind.
    fn kind(&self) -> MachineKind;

    /// The profile it was built from.
    fn profile(&self) -> &Profile;

    /// The project it is bound to.
    fn project(&self) -> &ProjectId;

    /// 准备环境: make the run's environment exist on the machine (idempotent).
    fn prepare_env(&self, spec: &EnvSpec) -> Result<EnvReport>;

    /// 送代码: copy the snapshot commit's tree (`git archive`) into `remote_code_dir`.
    fn ship(&self, repo: &Path, commit: &str, remote_code_dir: &str) -> Result<()>;

    /// 提交: start a job (Slurm: sbatch with an `afterany` dependency if given) and return
    /// its id. Only reachable through [`crate::submission`] (see [`SubmitPermit`]).
    fn submit(
        &self,
        permit: &SubmitPermit<'_>,
        script: &str,
        job_name: &str,
        dependency: Option<&str>,
    ) -> Result<String>;

    /// The id of an existing job with this name (idempotent resubmission).
    fn find_job(&self, job_name: &str) -> Result<Option<String>>;

    /// 查状态: the jobs the machine knows about.
    fn poll(&self, job_ids: &[String]) -> Result<BTreeMap<String, JobStatus>>;

    /// 取消.
    fn cancel(&self, job_ids: &[String]) -> Result<()>;

    /// 日志: the last `n` lines of a file under the project's dirs.
    fn tail(&self, remote_path: &str, n: usize) -> Result<String>;

    /// Entries of a directory under the project's dirs.
    fn list_dir(&self, remote_path: &str) -> Result<Vec<String>>;

    /// 拉回结果: copy `remote_dir`'s contents into `local_dir` (missing remote dir: nothing).
    fn fetch(&self, remote_dir: &str, local_dir: &Path) -> Result<()>;

    /// 花费: the account's balance text, if the machine has one.
    fn balance(&self) -> Result<Option<String>>;
}

/// The backend for a profile, bound to `project`.
pub fn get_backend(profile: &Profile, project: &ProjectId) -> Result<Box<dyn Backend>> {
    match profile.kind {
        MachineKind::Fake => Ok(Box::new(FakeSlurm::new(profile.clone(), project.clone())?)),
        MachineKind::Slurm => Ok(Box::new(SshSlurm::new(
            profile.clone(),
            project.clone(),
            Arc::new(SystemRunner),
        ))),
        other => Err(not_yet(other)),
    }
}

fn not_yet(kind: MachineKind) -> Error {
    Error::Backend(format!(
        "machine kind {kind:?} is not implemented yet (ADR 0001); use a slurm or fake profile"
    ))
}

/// `(run id, segment)` of a job name of `project`; any other project's is refused.
pub fn own_job(project: &ProjectId, job_name: &str) -> Result<(RunId, u32)> {
    let (owner, run, segment) = crate::slurm::parse_job_name(job_name)?;
    if &owner != project {
        return Err(Error::Backend(format!(
            "job {job_name} belongs to project {owner}, not {project}; refused"
        )));
    }
    let run = RunId::parse(&run).map_err(|_| {
        Error::Backend(format!("job {job_name}: {} is not a run id", py_repr(&run)))
    })?;
    Ok((run, segment))
}

/// Validate Slurm job ids (`^[0-9]+(?:_[0-9]+)?$`); every bad one is named
/// (`invalid Slurm job id(s): a, b`).
pub fn check_job_ids(job_ids: &[String]) -> Result<Vec<crate::model::JobId>> {
    let bad: Vec<&str> = job_ids
        .iter()
        .filter(|j| crate::model::JobId::parse(j).is_err())
        .map(String::as_str)
        .collect();
    if !bad.is_empty() {
        return Err(Error::Backend(format!(
            "invalid Slurm job id(s): {}",
            bad.join(", ")
        )));
    }
    job_ids
        .iter()
        .map(|j| crate::model::JobId::parse(j))
        .collect()
}

fn ids_text(job_ids: &[String]) -> Result<Vec<String>> {
    Ok(check_job_ids(job_ids)?
        .into_iter()
        .map(|j| j.as_str().to_string())
        .collect())
}

/// Only paths under this project's `runs/` or `out/` dirs (no `.` / `..`, plain segments).
pub fn restrict_path(profile: &Profile, project: &ProjectId, path: &str) -> Result<String> {
    let trimmed = path.trim_end_matches('/');
    let path = if trimmed.is_empty() { "/" } else { trimmed };
    let segment_ok = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    };
    for role in ["runs", "out"] {
        let Some(base) = profile.paths.get(role) else {
            continue;
        };
        let root = format!("{base}/{project}");
        let rest = if path == root {
            Some("")
        } else {
            path.strip_prefix(&format!("{root}/"))
        };
        if let Some(rest) = rest {
            let rest = rest.trim_matches('/');
            if rest.is_empty() || rest.split('/').all(segment_ok) {
                return Ok(path.to_string());
            }
        }
    }
    Err(Error::Arb(format!(
        "remote path {} is outside project {project}'s runs/ and out/ dirs on this machine; refused",
        py_repr(path)
    )))
}

/// sacct state text → [`JobState`] (`CANCELLED by 123` → cancelled, unknown → unknown).
pub fn map_slurm_state(raw: &str) -> JobState {
    let word = py_strip(raw)
        .split(crate::records::py_isspace)
        .find(|w| !w.is_empty())
        .unwrap_or("")
        .trim_end_matches('+')
        .to_uppercase();
    match word.as_str() {
        "PENDING" | "REQUEUED" | "REQUEUE_HOLD" | "REQUEUE_FED" | "PREEMPTED" | "RESV_DEL_HOLD" => {
            JobState::Pending
        }
        "RUNNING" | "COMPLETING" | "CONFIGURING" | "SUSPENDED" | "STAGE_OUT" | "SIGNALING"
        | "RESIZING" => JobState::Running,
        "COMPLETED" => JobState::Completed,
        "FAILED" | "NODE_FAIL" | "OUT_OF_MEMORY" | "BOOT_FAIL" | "DEADLINE" | "SPECIAL_EXIT" => {
            JobState::Failed
        }
        "CANCELLED" | "REVOKED" => JobState::Cancelled,
        "TIMEOUT" => JobState::Timeout,
        _ => JobState::Unknown,
    }
}

/// A job state after which nothing changes.
pub fn is_terminal(state: JobState) -> bool {
    JobState::TERMINAL.contains(&state)
}

/// SU charged, from sacct's AllocTRES `billing=N`: N × hours / billing_per_su, evaluated
/// left to right like Python (None when billing_per_su ≤ 0 or there is no billing item).
pub fn su_from_billing(alloc_tres: &str, elapsed_s: u64, billing_per_su: f64) -> Option<f64> {
    if billing_per_su <= 0.0 {
        return None;
    }
    let tres = py_strip(alloc_tres);
    // (?:^|,)billing=([0-9]+(?:\.[0-9]+)?)(?:,|$)
    let n = tres.split(',').find_map(|item| {
        let v = item.strip_prefix("billing=")?;
        let (int, frac) = match v.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (v, None),
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        (digits(int) && frac.is_none_or(digits)).then_some(v)
    })?;
    let n: f64 = n.parse().ok()?;
    Some(n * elapsed_s as f64 / 3600.0 / billing_per_su)
}

// ---------------------------------------------------------------- processes

/// A finished process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// The only place that starts processes for a backend (tests replace it; Python
/// `backends.ssh.Runner`).
pub trait Runner: std::fmt::Debug + Send + Sync {
    /// Run `argv` (stdin: `input`, or /dev/null), killing it after `timeout`.
    fn run(&self, argv: &[String], input: Option<&str>, timeout: Duration) -> Result<RemoteOutput>;

    /// `producer | consumer` (producer in `cwd`); (producer code, consumer code, stderr of both).
    fn pipe(
        &self,
        producer: &[String],
        cwd: &Path,
        consumer: &[String],
        timeout: Duration,
    ) -> Result<(i32, i32, String)>;

    /// The program is on PATH (`shutil.which`).
    fn has(&self, program: &str) -> bool;
}

/// The real processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

fn reader<R: Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    })
}

/// Wait for `child`, killing it at `deadline`; None when it was killed.
fn wait_until(child: &mut std::process::Child, deadline: Instant) -> Result<Option<i32>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status.code().unwrap_or(-1)));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_error(program: &str, e: std::io::Error) -> Error {
    Error::Backend(format!("cannot run {program}: {e}"))
}

impl Runner for SystemRunner {
    fn run(&self, argv: &[String], input: Option<&str>, timeout: Duration) -> Result<RemoteOutput> {
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| spawn_error(&argv[0], e))?;
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            let text = text.to_string();
            std::thread::spawn(move || {
                let _ = stdin.write_all(text.as_bytes());
            });
        }
        let out = reader(child.stdout.take());
        let err = reader(child.stderr.take());
        let code = wait_until(&mut child, Instant::now() + timeout)?;
        let (out, err) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        let Some(code) = code else {
            return Err(Error::Backend(format!(
                "`{}` timed out after {}s",
                shlex_join(argv),
                timeout.as_secs()
            )));
        };
        Ok(RemoteOutput {
            code,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        })
    }

    fn pipe(
        &self,
        producer: &[String],
        cwd: &Path,
        consumer: &[String],
        timeout: Duration,
    ) -> Result<(i32, i32, String)> {
        let mut p1 = Command::new(&producer[0])
            .args(&producer[1..])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| spawn_error(&producer[0], e))?;
        let p1_out = p1.stdout.take().expect("piped stdout");
        let mut p2 = match Command::new(&consumer[0])
            .args(&consumer[1..])
            .stdin(Stdio::from(p1_out))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(p) => p,
            Err(e) => {
                let _ = p1.kill();
                let _ = p1.wait();
                return Err(spawn_error(&consumer[0], e));
            }
        };
        let err1 = reader(p1.stderr.take());
        let out2 = reader(p2.stdout.take());
        let err2 = reader(p2.stderr.take());
        let deadline = Instant::now() + timeout;
        let rc2 = wait_until(&mut p2, deadline)?;
        let rc1 = wait_until(&mut p1, deadline)?;
        let _ = out2.join();
        let err = format!(
            "{}{}",
            String::from_utf8_lossy(&err1.join().unwrap_or_default()),
            String::from_utf8_lossy(&err2.join().unwrap_or_default())
        );
        match (rc1, rc2) {
            (Some(a), Some(b)) => Ok((a, b, py_strip(&err).to_string())),
            _ => Err(Error::Backend(format!(
                "`{}` timed out",
                shlex_join(consumer)
            ))),
        }
    }

    fn has(&self, program: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let p = dir.join(program);
                std::fs::metadata(&p).is_ok_and(|m| {
                    use std::os::unix::fs::PermissionsExt;
                    m.is_file() && m.permissions().mode() & 0o111 != 0
                })
            })
        })
    }
}

// ---------------------------------------------------------------- ssh

/// `ssh` options of every remote call: BatchMode, never open a master (Python
/// `backends.ssh.SSH_OPTS`).
pub const SSH_OPTS: &[&str] = &["-o", "BatchMode=yes", "-o", "ControlMaster=no"];
/// Remote command timeout (s).
pub const TIMEOUT_S: u64 = 120;
/// `ship` timeout (s).
pub const SHIP_TIMEOUT_S: u64 = 1800;
/// `fetch` timeout (s).
pub const FETCH_TIMEOUT_S: u64 = 3600;
/// How far back `find_job` asks sacct.
pub const FIND_WINDOW: &str = "now-30days";

fn strings(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|s| s.to_string()).collect()
}

/// Commands on `host` over the user's existing ssh master, nothing else (Python
/// `backends.ssh.SshSession`).
#[derive(Debug, Clone)]
pub struct SshSession {
    pub host: String,
    pub runner: Arc<dyn Runner>,
}

impl SshSession {
    /// A session on `host` that starts processes with `runner`.
    pub fn new(host: &str, runner: Arc<dyn Runner>) -> SshSession {
        SshSession {
            host: host.to_string(),
            runner,
        }
    }

    /// `ssh <SSH_OPTS> <host> <remote_cmd>`.
    pub fn ssh_argv(&self, remote_cmd: &str) -> Vec<String> {
        let mut argv = vec!["ssh".to_string()];
        argv.extend(strings(SSH_OPTS));
        argv.push(self.host.clone());
        argv.push(remote_cmd.to_string());
        argv
    }

    /// `ssh -O check <host>`; `Error::NeedLogin` without a live master (no retry, no prompt).
    pub fn gate(&self) -> Result<()> {
        let out = self.runner.run(
            &strings(&["ssh", "-O", "check", &self.host]),
            None,
            Duration::from_secs(30),
        )?;
        if out.code != 0 {
            return Err(Error::NeedLogin(need_login_message(&self.host)));
        }
        Ok(())
    }

    /// [`SshSession::gate`], then run `remote_cmd`; exit codes outside `ok_codes` are
    /// `Error::Backend` with the last line of output.
    pub fn remote(
        &self,
        remote_cmd: &str,
        input: Option<&str>,
        ok_codes: &[i32],
    ) -> Result<RemoteOutput> {
        self.remote_timeout(remote_cmd, input, ok_codes, Duration::from_secs(TIMEOUT_S))
    }

    /// [`SshSession::remote`] with another timeout.
    pub fn remote_timeout(
        &self,
        remote_cmd: &str,
        input: Option<&str>,
        ok_codes: &[i32],
        timeout: Duration,
    ) -> Result<RemoteOutput> {
        self.gate()?;
        let out = self
            .runner
            .run(&self.ssh_argv(remote_cmd), input, timeout)?;
        if !ok_codes.contains(&out.code) {
            let text = if !out.stderr.is_empty() {
                &out.stderr
            } else {
                &out.stdout
            };
            let lines = py_splitlines(py_strip(text));
            let detail = lines.last().map(|l| format!(": {l}")).unwrap_or_default();
            return Err(Error::Backend(format!(
                "`{remote_cmd}` on {} failed (exit {}){detail}",
                self.host, out.code
            )));
        }
        Ok(out)
    }

    /// `ssh -G host`: the effective client config (local; opens no connection).
    pub fn local_config(&self) -> Result<BTreeMap<String, String>> {
        let out = self.runner.run(
            &strings(&["ssh", "-G", &self.host]),
            None,
            Duration::from_secs(30),
        )?;
        if out.code != 0 {
            return Err(Error::Backend(format!(
                "`ssh -G {}` failed: {}",
                self.host,
                py_strip(&out.stderr)
            )));
        }
        let mut cfg = BTreeMap::new();
        for line in py_splitlines(&out.stdout) {
            let line = py_strip(line);
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            if !key.is_empty() {
                cfg.entry(key.to_lowercase())
                    .or_insert_with(|| py_strip(value).to_string());
            }
        }
        Ok(cfg)
    }
}

// ---------------------------------------------------------------- SshSlurm

/// A Slurm cluster over ssh (Python `backends.ssh.SshSlurm`): sbatch with `afterany`
/// dependencies, `sacct` states and `billing=N`, reads limited by [`restrict_path`].
#[derive(Debug, Clone)]
pub struct SshSlurm {
    pub profile: Profile,
    pub project: ProjectId,
    pub session: SshSession,
}

/// Job ids from `<id><sep><name>` lines whose name is exactly `job_name`.
fn ids_named(text: &str, sep: char, job_name: &str) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for line in py_splitlines(text) {
        let line = py_strip(line);
        let (jid, name) = line.split_once(sep).unwrap_or((line, ""));
        if !jid.is_empty() && py_strip(name) == job_name {
            ids.push(py_strip(jid).to_string());
        }
    }
    ids_text(&ids)
}

impl SshSlurm {
    /// An SshSlurm whose processes start through `runner`.
    pub fn new(profile: Profile, project: ProjectId, runner: Arc<dyn Runner>) -> SshSlurm {
        let session = SshSession::new(&profile.host, runner);
        SshSlurm {
            profile,
            project,
            session,
        }
    }

    fn remote(&self, cmd: &str) -> Result<RemoteOutput> {
        self.session.remote(cmd, None, &[0])
    }

    /// sbatch the script; the body of [`Backend::submit`] (which needs the permit).
    fn sbatch(&self, script: &str, job_name: &str, dependency: Option<&str>) -> Result<String> {
        let (run_id, segment) = own_job(&self.project, job_name)?;
        let paths = &self.profile.paths;
        let script_at = script_path(paths, &self.project, &run_id, segment);
        let sbatch_dir = script_at.rsplit_once('/').map_or("", |(d, _)| d);
        let log = log_pattern(paths, &self.project, &run_id, segment);
        let logs_dir = log.rsplit_once('/').map_or("", |(d, _)| d);
        let dep = match dependency {
            Some(d) => format!(" --dependency=afterany:{}", ids_text(&[d.to_string()])?[0]),
            None => String::new(),
        };
        let cmd = format!(
            "mkdir -p {} {} && cat > {} && sbatch --parsable{dep} {}",
            dq(sbatch_dir)?,
            dq(logs_dir)?,
            dq(&script_at)?,
            dq(&script_at)?
        );
        let out = self.session.remote(&cmd, Some(script), &[0])?;
        let last = py_splitlines(&out.stdout)
            .into_iter()
            .map(py_strip)
            .rfind(|l| !l.is_empty())
            .unwrap_or("");
        let job_id = last.split(';').next().unwrap_or("");
        if job_id.is_empty() || !job_id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::Backend(format!(
                "sbatch printed no job id for {job_name}: {}",
                py_repr(py_strip(&out.stdout))
            )));
        }
        Ok(job_id.to_string())
    }
}

/// `parts[2].strip().isdigit()` then `int(...)`; else 0.
fn py_int_or_zero(s: &str) -> u64 {
    let t = py_strip(s);
    if t.is_empty() {
        return 0;
    }
    let mut n: u64 = 0;
    for c in t.chars() {
        match py_decimal(c) {
            Some(d) => n = n.saturating_mul(10).saturating_add(d as u64),
            None => return 0,
        }
    }
    n
}

/// Parse `sacct -j … -o JobID,State,ElapsedRaw,AllocTRES` output (Python `SshSlurm.poll`).
pub fn parse_sacct(
    stdout: &str,
    job_ids: &[String],
    billing_per_su: f64,
) -> BTreeMap<String, JobStatus> {
    let mut out = BTreeMap::new();
    for line in py_splitlines(stdout) {
        let parts: Vec<&str> = py_strip(line).split('|').collect();
        if parts.len() < 3 || !job_ids.iter().any(|j| j == parts[0]) {
            continue;
        }
        let state = map_slurm_state(parts[1]);
        let elapsed = py_int_or_zero(parts[2]);
        let billed_su = if is_terminal(state) && parts.len() > 3 {
            su_from_billing(parts[3], elapsed, billing_per_su)
        } else {
            None
        };
        out.insert(
            parts[0].to_string(),
            JobStatus {
                state,
                elapsed_s: elapsed,
                billed_su,
            },
        );
    }
    out
}

/// The `sacct` command of [`SshSlurm`]'s poll.
pub fn sacct_command(job_ids: &[String]) -> String {
    format!(
        "sacct -j {} -n -X -P -o JobID,State,ElapsedRaw,AllocTRES",
        job_ids.join(",")
    )
}

impl Backend for SshSlurm {
    fn kind(&self) -> MachineKind {
        MachineKind::Slurm
    }
    fn profile(&self) -> &Profile {
        &self.profile
    }
    fn project(&self) -> &ProjectId {
        &self.project
    }

    fn prepare_env(&self, spec: &EnvSpec) -> Result<EnvReport> {
        let _ = spec;
        Err(Error::Backend(
            "preparing environments is not implemented yet (ADR 0001); use the profile's prelude / env_activate".into(),
        ))
    }

    fn ship(&self, repo: &Path, commit: &str, remote_code_dir: &str) -> Result<()> {
        let hex = (7..=64).contains(&commit.len())
            && commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err(Error::Backend(format!(
                "invalid commit {}",
                py_repr(commit)
            )));
        }
        let d = dq(&restrict_path(
            &self.profile,
            &self.project,
            remote_code_dir,
        )?)?;
        self.session.gate()?;
        let (rc1, rc2, err) = self.session.runner.pipe(
            &strings(&["git", "archive", "--format=tar", commit]),
            repo,
            &self
                .session
                .ssh_argv(&format!("mkdir -p {d} && tar -x -C {d}")),
            Duration::from_secs(SHIP_TIMEOUT_S),
        )?;
        if rc1 != 0 || rc2 != 0 {
            let detail = py_splitlines(&err)
                .last()
                .map(|l| format!(": {l}"))
                .unwrap_or_default();
            return Err(Error::Backend(format!(
                "shipping {} to {}:{remote_code_dir} failed{detail}",
                &commit[..12.min(commit.len())],
                self.session.host
            )));
        }
        Ok(())
    }

    fn submit(
        &self,
        permit: &SubmitPermit<'_>,
        script: &str,
        job_name: &str,
        dependency: Option<&str>,
    ) -> Result<String> {
        let _ = permit;
        self.sbatch(script, job_name, dependency)
    }

    fn find_job(&self, job_name: &str) -> Result<Option<String>> {
        own_job(&self.project, job_name)?;
        let queued = ids_named(
            &self
                .remote(&format!(
                    "squeue --name={job_name} -u \"$USER\" -h -o '%i %j'"
                ))?
                .stdout,
            ' ',
            job_name,
        )?;
        let ids = if queued.is_empty() {
            ids_named(
                &self
                    .remote(&format!(
                        "sacct --name={job_name} -n -X -P -o JobID,JobName --starttime={FIND_WINDOW}"
                    ))?
                    .stdout,
                '|',
                job_name,
            )?
        } else {
            queued
        };
        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        if distinct.len() > 1 {
            return Err(Error::Backend(format!(
                "several jobs named {job_name} ({}); cancel the extras by hand before resuming",
                distinct.join(", ")
            )));
        }
        Ok(ids.into_iter().next())
    }

    fn poll(&self, job_ids: &[String]) -> Result<BTreeMap<String, JobStatus>> {
        let ids = ids_text(job_ids)?;
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let out = self.remote(&sacct_command(&ids))?;
        Ok(parse_sacct(&out.stdout, &ids, self.profile.billing_per_su))
    }

    fn cancel(&self, job_ids: &[String]) -> Result<()> {
        let ids = ids_text(job_ids)?;
        if !ids.is_empty() {
            self.remote(&format!("scancel {}", ids.join(" ")))?;
        }
        Ok(())
    }

    fn tail(&self, remote_path: &str, n: usize) -> Result<String> {
        let path = restrict_path(&self.profile, &self.project, remote_path)?;
        Ok(self.remote(&format!("tail -n {n} {}", dq(&path)?))?.stdout)
    }

    fn list_dir(&self, remote_path: &str) -> Result<Vec<String>> {
        let path = restrict_path(&self.profile, &self.project, remote_path)?;
        let out = self.remote(&format!("ls -1 {}", dq(&path)?))?;
        Ok(py_splitlines(&out.stdout)
            .into_iter()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect())
    }

    fn fetch(&self, remote_dir: &str, local_dir: &Path) -> Result<()> {
        let path = dq(&restrict_path(&self.profile, &self.project, remote_dir)?)?;
        let out = self.remote(&format!("if [ -d {path} ]; then cd {path} && pwd -P; fi"))?;
        let resolved = py_strip(&out.stdout).to_string();
        std::fs::create_dir_all(local_dir)?;
        if resolved.is_empty() {
            return Ok(()); // the run wrote nothing to ARBORIA_OUT
        }
        let abs_ok = resolved.starts_with('/')
            && resolved
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-'));
        if !abs_ok {
            return Err(Error::Backend(format!(
                "unexpected remote path {}",
                py_repr(&resolved)
            )));
        }
        let host = &self.session.host;
        let runner = &self.session.runner;
        let timeout = Duration::from_secs(FETCH_TIMEOUT_S);
        if runner.has("rsync") {
            let mut ssh = vec!["ssh"];
            ssh.extend_from_slice(SSH_OPTS);
            let argv = vec![
                "rsync".to_string(),
                "-a".into(),
                "-e".into(),
                shlex_join(&ssh),
                format!("{host}:{resolved}/"),
                format!("{}/", local_dir.display()),
            ];
            let out = runner.run(&argv, None, timeout)?;
            if out.code != 0 {
                return Err(Error::Backend(format!(
                    "rsync from {host}:{resolved} failed: {}",
                    py_strip(&out.stderr)
                )));
            }
            return Ok(());
        }
        let parent = local_dir.parent().unwrap_or(Path::new("."));
        let tmp = TempDir::new(parent, "arboria-fetch-")?;
        let mut argv = strings(&["scp", "-r", "-q"]);
        argv.extend(strings(SSH_OPTS));
        argv.push(format!("{host}:{resolved}"));
        argv.push(tmp.0.display().to_string());
        let out = runner.run(&argv, None, timeout)?;
        if out.code != 0 {
            return Err(Error::Backend(format!(
                "scp from {host}:{resolved} failed: {}",
                py_strip(&out.stderr)
            )));
        }
        let got = tmp.0.join(resolved.rsplit('/').next().unwrap_or(""));
        if got.is_dir() {
            copy_tree(&got, local_dir)?;
        }
        Ok(())
    }

    fn balance(&self) -> Result<Option<String>> {
        if self.profile.balance_cmd.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.remote(&self.profile.balance_cmd)?.stdout))
    }
}

/// A temp dir removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(parent: &Path, prefix: &str) -> Result<TempDir> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        for _ in 0..100 {
            let dir = parent.join(format!(
                "{prefix}{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(TempDir(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::arb("cannot create a temporary directory"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `shutil.copytree(src, dst, dirs_exist_ok=True)` (symlinks copied as links).
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            let target = std::fs::read_link(&from)?;
            let _ = std::fs::remove_file(&to);
            std::os::unix::fs::symlink(target, &to)?;
        } else if ft.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- machines still to come

macro_rules! later_backend {
    ($(#[$m:meta])* $name:ident, $kind:expr) => {
        $(#[$m])*
        #[derive(Debug, Clone)]
        pub struct $name {
            pub profile: Profile,
            pub project: ProjectId,
        }

        impl Backend for $name {
            fn kind(&self) -> MachineKind {
                $kind
            }
            fn profile(&self) -> &Profile {
                &self.profile
            }
            fn project(&self) -> &ProjectId {
                &self.project
            }
            fn prepare_env(&self, _: &EnvSpec) -> Result<EnvReport> {
                Err(not_yet($kind))
            }
            fn ship(&self, _: &Path, _: &str, _: &str) -> Result<()> {
                Err(not_yet($kind))
            }
            fn submit(&self, _: &SubmitPermit<'_>, _: &str, _: &str, _: Option<&str>) -> Result<String> {
                Err(not_yet($kind))
            }
            fn find_job(&self, _: &str) -> Result<Option<String>> {
                Err(not_yet($kind))
            }
            fn poll(&self, _: &[String]) -> Result<BTreeMap<String, JobStatus>> {
                Err(not_yet($kind))
            }
            fn cancel(&self, _: &[String]) -> Result<()> {
                Err(not_yet($kind))
            }
            fn tail(&self, _: &str, _: usize) -> Result<String> {
                Err(not_yet($kind))
            }
            fn list_dir(&self, _: &str) -> Result<Vec<String>> {
                Err(not_yet($kind))
            }
            fn fetch(&self, _: &str, _: &Path) -> Result<()> {
                Err(not_yet($kind))
            }
            fn balance(&self) -> Result<Option<String>> {
                Err(not_yet($kind))
            }
        }
    };
}

later_backend!(
    /// This computer (new in the Rust core): runs in a process group under
    /// `.arboria/runs/<run>`, no scheduler, cost 0.
    LocalMachine, MachineKind::Local
);
later_backend!(
    /// A workstation over ssh without a scheduler (new): the same ssh gate as Slurm.
    SshWorkstation, MachineKind::Ssh
);
later_backend!(
    /// RunPod (new): API key from the keychain, never shown to the agent.
    RunPod, MachineKind::Runpod
);
later_backend!(
    /// Modal (new): token from the keychain, never shown to the agent.
    Modal, MachineKind::Modal
);

// ---------------------------------------------------------------- FakeSlurm

/// First job id FakeSlurm hands out.
pub const FIRST_JOB_ID: u64 = 1001;

/// A scripted job outcome (Python `fake.DEFAULT_SCENARIO`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scenario {
    /// Final state.
    pub state: JobState,
    /// Seconds from start to end.
    pub run_s: u64,
    /// Seconds pending before it starts.
    pub queue_s: u64,
    /// Elapsed reported at the end (default: `run_s`).
    pub elapsed: Option<u64>,
    /// Billed SU reported once terminal.
    pub billed: Option<f64>,
    /// Appended to the job's log when it starts.
    pub log: String,
    /// Files written under `ARBORIA_OUT` at the end.
    pub out_files: BTreeMap<String, String>,
    /// sbatch succeeds but the reply is lost (once).
    pub lose_reply: bool,
    /// sbatch fails (once).
    pub submit_error: bool,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            state: JobState::Completed,
            run_s: 60,
            queue_s: 0,
            elapsed: None,
            billed: None,
            log: String::new(),
            out_files: BTreeMap::new(),
            lose_reply: false,
            submit_error: false,
        }
    }
}

/// A FakeSlurm job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FakeJob {
    pub id: String,
    pub name: String,
    pub dependency: Option<String>,
    pub state: JobState,
    pub submit_t: u64,
    pub start_t: Option<u64>,
    pub end_t: Option<u64>,
    pub elapsed: u64,
    pub log: String,
    pub out_dir: String,
    pub script: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FakeState {
    clock: u64,
    next_id: u64,
    jobs: BTreeMap<String, FakeJob>,
    scenarios: BTreeMap<String, Scenario>,
    calls: Vec<Value>,
    need_login: bool,
}

impl Default for FakeState {
    fn default() -> Self {
        FakeState {
            clock: 0,
            next_id: FIRST_JOB_ID,
            jobs: BTreeMap::new(),
            scenarios: BTreeMap::new(),
            calls: Vec::new(),
            need_login: false,
        }
    }
}

/// Python `getpass.getuser()`: LOGNAME, USER, LNAME, USERNAME, else `id -un`.
fn user_name() -> String {
    for var in ["LOGNAME", "USER", "LNAME", "USERNAME"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    Command::new("id")
        .arg("-un")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "user".into())
}

/// A "remote" path of FakeSlurm on this computer (`$USER` expanded).
pub fn fake_local_path(path: &str) -> PathBuf {
    PathBuf::from(crate::slurm::replace_user_var(path, &user_name()))
}

/// A local Slurm simulation for tests: state in `<root>/state.json` (same keys as
/// Python's), outcomes scripted per job name, a manual clock. Ports
/// `arboria.backends.fake.FakeSlurm`.
#[derive(Debug, Clone)]
pub struct FakeSlurm {
    pub profile: Profile,
    pub project: ProjectId,
    /// The profile's `root`: state file and the fake remote file system.
    pub root: PathBuf,
}

impl FakeSlurm {
    /// A FakeSlurm for a `kind = "fake"` profile.
    pub fn new(profile: Profile, project: ProjectId) -> Result<Self> {
        let root = PathBuf::from(profile.root.clone().unwrap_or_default());
        Ok(FakeSlurm {
            profile,
            project,
            root,
        })
    }

    /// Run `f` on the state under `<root>/state.lock`; the state is saved even when `f`
    /// fails (a scripted failure still consumed its scenario).
    fn with_state<T>(&self, f: impl FnOnce(&mut FakeState) -> Result<T>) -> Result<T> {
        std::fs::create_dir_all(&self.root)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(self.root.join("state.lock"))?;
        lock.lock()?;
        let file = self.root.join("state.json");
        let mut state: FakeState = if file.exists() {
            serde_json::from_str(&std::fs::read_to_string(&file)?)?
        } else {
            FakeState::default()
        };
        let result = f(&mut state);
        let tmp = self.root.join("state.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&state)?)?;
        std::fs::rename(&tmp, &file)?;
        lock.unlock()?;
        result
    }

    fn remote_call(&self, st: &mut FakeState, call: Value) -> Result<()> {
        st.calls.push(call);
        if st.need_login {
            let host = if self.profile.host.is_empty() {
                &self.profile.name
            } else {
                &self.profile.host
            };
            return Err(Error::NeedLogin(need_login_message(host)));
        }
        tick(st);
        Ok(())
    }

    /// Script the outcome of the job named `job_name`.
    pub fn script(&self, job_name: &str, scenario: Scenario) -> Result<()> {
        self.with_state(|st| {
            st.scenarios.insert(job_name.to_string(), scenario);
            Ok(())
        })
    }

    /// Advance the manual clock.
    pub fn advance(&self, seconds: u64) -> Result<()> {
        self.with_state(|st| {
            st.clock += seconds;
            tick(st);
            Ok(())
        })
    }

    /// Make every remote call fail with `NeedLogin` (or stop doing so).
    pub fn set_need_login(&self, value: bool) -> Result<()> {
        self.with_state(|st| {
            st.need_login = value;
            Ok(())
        })
    }

    /// The remote calls so far (`[method, args…]`), optionally of one method.
    pub fn calls(&self, method: Option<&str>) -> Result<Vec<Value>> {
        self.with_state(|st| {
            Ok(st
                .calls
                .iter()
                .filter(|c| method.is_none_or(|m| c[0] == m))
                .cloned()
                .collect())
        })
    }

    /// The jobs, after a tick.
    pub fn jobs(&self) -> Result<BTreeMap<String, FakeJob>> {
        self.with_state(|st| {
            tick(st);
            Ok(st.jobs.clone())
        })
    }

    /// sbatch; the body of [`Backend::submit`] (which needs the permit).
    fn sbatch(&self, script: &str, job_name: &str, dependency: Option<&str>) -> Result<String> {
        let (run_id, segment) = own_job(&self.project, job_name)?;
        let target = fake_local_path(&script_path(
            &self.profile.paths,
            &self.project,
            &run_id,
            segment,
        ));
        self.with_state(|st| {
            self.remote_call(st, json!(["submit", job_name, dependency]))?;
            let sc = st.scenarios.entry(job_name.to_string()).or_default();
            if sc.submit_error {
                sc.submit_error = false;
                return Err(Error::Backend(format!(
                    "sbatch failed for {job_name} (scripted)"
                )));
            }
            if let Some(dep) = dependency {
                if !st.jobs.contains_key(dep) {
                    return Err(Error::Backend(format!("sbatch: invalid dependency {dep}")));
                }
            }
            let jid = st.next_id.to_string();
            st.next_id += 1;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, script)?;
            let grab = |re: &str| {
                regex::Regex::new(re)
                    .expect("static regex")
                    .captures(script)
                    .map(|c| c[1].to_string())
                    .unwrap_or_default()
            };
            st.jobs.insert(
                jid.clone(),
                FakeJob {
                    id: jid.clone(),
                    name: job_name.to_string(),
                    dependency: dependency.map(str::to_string),
                    state: JobState::Pending,
                    submit_t: st.clock,
                    start_t: None,
                    end_t: None,
                    elapsed: 0,
                    log: grab(r"(?m)^#SBATCH --output=(.+)$"),
                    out_dir: grab(r#"(?m)^export ARBORIA_OUT="(.+)"$"#),
                    script: script.to_string(),
                },
            );
            tick(st);
            let lose_reply = st
                .scenarios
                .get_mut(job_name)
                .is_some_and(|sc| std::mem::take(&mut sc.lose_reply));
            if lose_reply {
                return Err(Error::Backend(format!(
                    "connection lost after sbatch {job_name} (scripted)"
                )));
            }
            Ok(jid)
        })
    }
}

fn scenario(st: &FakeState, name: &str) -> Scenario {
    st.scenarios.get(name).cloned().unwrap_or_default()
}

fn tick(st: &mut FakeState) {
    let mut changed = true;
    while changed {
        changed = false;
        let mut ids: Vec<String> = st.jobs.keys().cloned().collect();
        ids.sort_by_key(|j| j.parse::<u64>().unwrap_or(u64::MAX));
        for jid in ids {
            let sc = scenario(st, &st.jobs[&jid].name);
            let job = st.jobs[&jid].clone();
            let mut job = job;
            if job.state == JobState::Pending {
                let dep = job
                    .dependency
                    .as_ref()
                    .and_then(|d| st.jobs.get(d))
                    .cloned();
                if let Some(d) = &dep {
                    if !is_terminal(d.state) {
                        continue;
                    }
                }
                let ready = job
                    .submit_t
                    .max(dep.as_ref().and_then(|d| d.end_t).unwrap_or(0))
                    + sc.queue_s;
                if st.clock >= ready {
                    job.state = JobState::Running;
                    job.start_t = Some(ready);
                    write_log(&job, &sc.log);
                    changed = true;
                }
            }
            if job.state == JobState::Running {
                let start = job.start_t.unwrap_or(0);
                if st.clock >= start + sc.run_s {
                    job.state = sc.state;
                    job.end_t = Some(start + sc.run_s);
                    job.elapsed = sc.elapsed.unwrap_or(sc.run_s);
                    if !job.out_dir.is_empty() {
                        for (rel, text) in &sc.out_files {
                            let target = fake_local_path(&job.out_dir).join(rel);
                            if let Some(parent) = target.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(target, text);
                        }
                    }
                    changed = true;
                }
            }
            st.jobs.insert(jid, job);
        }
    }
}

fn write_log(job: &FakeJob, text: &str) {
    if job.log.is_empty() {
        return;
    }
    let user = user_name();
    let path = fake_local_path(&job.log.replace("%j", &job.id).replace("%u", &user));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(text.as_bytes());
    }
}

impl Backend for FakeSlurm {
    fn kind(&self) -> MachineKind {
        MachineKind::Fake
    }
    fn profile(&self) -> &Profile {
        &self.profile
    }
    fn project(&self) -> &ProjectId {
        &self.project
    }

    /// Records the call; an environment "exists" once prepared (`<root>/envs/<lock hash>`).
    fn prepare_env(&self, spec: &EnvSpec) -> Result<EnvReport> {
        self.with_state(|st| {
            self.remote_call(st, json!(["prepare_env", spec.lock_hash]))?;
            let location = self.root.join("envs").join(&spec.lock_hash);
            let reused = location.is_dir();
            std::fs::create_dir_all(&location)?;
            Ok(EnvReport {
                reused,
                location: location.display().to_string(),
                messages: Vec::new(),
            })
        })
    }

    fn ship(&self, repo: &Path, commit: &str, remote_code_dir: &str) -> Result<()> {
        let dir = restrict_path(&self.profile, &self.project, remote_code_dir)?;
        self.with_state(|st| self.remote_call(st, json!(["ship", commit, dir])))?;
        let dest = fake_local_path(&dir);
        std::fs::create_dir_all(&dest)?;
        let (rc1, rc2, err) = SystemRunner.pipe(
            &strings(&["git", "archive", "--format=tar", commit]),
            repo,
            &strings(&["tar", "-x", "-C", &dest.display().to_string()]),
            Duration::from_secs(SHIP_TIMEOUT_S),
        )?;
        if rc1 != 0 || rc2 != 0 {
            return Err(Error::Backend(format!("git archive failed: {err}")));
        }
        Ok(())
    }

    fn submit(
        &self,
        permit: &SubmitPermit<'_>,
        script: &str,
        job_name: &str,
        dependency: Option<&str>,
    ) -> Result<String> {
        let _ = permit;
        self.sbatch(script, job_name, dependency)
    }

    fn find_job(&self, job_name: &str) -> Result<Option<String>> {
        own_job(&self.project, job_name)?;
        self.with_state(|st| {
            self.remote_call(st, json!(["find_job", job_name]))?;
            Ok(st
                .jobs
                .values()
                .filter(|j| j.name == job_name)
                .map(|j| j.id.clone())
                .max_by_key(|id| id.parse::<u64>().unwrap_or(0)))
        })
    }

    fn poll(&self, job_ids: &[String]) -> Result<BTreeMap<String, JobStatus>> {
        let ids = ids_text(job_ids)?;
        self.with_state(|st| {
            self.remote_call(st, json!(["poll", ids]))?;
            let mut out = BTreeMap::new();
            for jid in &ids {
                let Some(job) = st.jobs.get(jid) else {
                    continue;
                };
                let sc = scenario(st, &job.name);
                let elapsed = if job.state == JobState::Running {
                    st.clock - job.start_t.unwrap_or(0)
                } else {
                    job.elapsed
                };
                let billed_su = if is_terminal(job.state) {
                    sc.billed
                } else {
                    None
                };
                out.insert(
                    jid.clone(),
                    JobStatus {
                        state: job.state,
                        elapsed_s: elapsed,
                        billed_su,
                    },
                );
            }
            Ok(out)
        })
    }

    fn cancel(&self, job_ids: &[String]) -> Result<()> {
        let ids = ids_text(job_ids)?;
        self.with_state(|st| {
            self.remote_call(st, json!(["cancel", ids]))?;
            let clock = st.clock;
            for jid in &ids {
                if let Some(job) = st.jobs.get_mut(jid) {
                    if !is_terminal(job.state) {
                        if job.state == JobState::Running {
                            job.elapsed = clock - job.start_t.unwrap_or(0);
                        }
                        job.state = JobState::Cancelled;
                        job.end_t = Some(clock);
                    }
                }
            }
            tick(st);
            Ok(())
        })
    }

    fn tail(&self, remote_path: &str, n: usize) -> Result<String> {
        let path = restrict_path(&self.profile, &self.project, remote_path)?;
        self.with_state(|st| self.remote_call(st, json!(["tail", path, n])))?;
        let p = fake_local_path(&path);
        if !p.is_file() {
            return Err(Error::Backend(format!("tail: {path}: no such file")));
        }
        let text = String::from_utf8_lossy(&std::fs::read(&p)?).into_owned();
        let lines = py_splitlines(&text);
        let start = lines.len().saturating_sub(n);
        let mut out = lines[start..].join("\n");
        if !lines.is_empty() {
            out.push('\n');
        }
        Ok(out)
    }

    fn list_dir(&self, remote_path: &str) -> Result<Vec<String>> {
        let path = restrict_path(&self.profile, &self.project, remote_path)?;
        self.with_state(|st| self.remote_call(st, json!(["list_dir", path])))?;
        let p = fake_local_path(&path);
        if !p.is_dir() {
            return Err(Error::Backend(format!("ls: {path}: no such directory")));
        }
        let mut names: Vec<String> = std::fs::read_dir(&p)?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        names.sort();
        Ok(names)
    }

    fn fetch(&self, remote_dir: &str, local_dir: &Path) -> Result<()> {
        let path = restrict_path(&self.profile, &self.project, remote_dir)?;
        self.with_state(|st| self.remote_call(st, json!(["fetch", path])))?;
        let src = fake_local_path(&path);
        std::fs::create_dir_all(local_dir)?;
        if src.is_dir() {
            copy_tree(&src, local_dir)?;
        }
        Ok(())
    }

    fn balance(&self) -> Result<Option<String>> {
        self.with_state(|st| self.remote_call(st, json!(["balance"])))?;
        Ok(Some("fake cluster: unlimited".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "arboria-backend-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p.canonicalize().unwrap())
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project() -> ProjectId {
        ProjectId::parse("wavefront-3f2a").unwrap()
    }

    fn fake(tmp: &Tmp) -> FakeSlurm {
        let r = tmp.0.display();
        let text = format!(
            "[fake]\nkind = \"fake\"\nroot = \"{r}/state\"\ngpus_per_node = 4\ncpus_per_gpu = 16\n\
             mem_per_gpu_gb = 100\ndefault_partition = \"ghx4\"\n[fake.partitions.ghx4]\n\
             max_walltime = \"48h\"\n[fake.paths]\nruns = \"{r}/remote/$USER/runs\"\n\
             data = \"{r}/remote/data\"\ncache = \"{r}/remote/cache\"\n\
             ckpt = \"{r}/remote/ckpt\"\nout = \"{r}/remote/out\"\n"
        );
        let raw: toml::Value = toml::from_str(&text).unwrap();
        let profile = crate::clusters::parse_profile("fake", &raw["fake"], "t").unwrap();
        FakeSlurm::new(profile, project()).unwrap()
    }

    fn name(run: &str, k: u32) -> String {
        format!("arb-wavefront-3f2a-{run}-s{k}")
    }

    fn script(f: &FakeSlurm, run: &str, k: u32) -> String {
        let run_id = RunId::parse(run).unwrap();
        let log = log_pattern(&f.profile.paths, &f.project, &run_id, k);
        let out = crate::slurm::out_dir(&f.profile.paths, &f.project, &run_id);
        format!(
            "#!/bin/bash\n#SBATCH --output={}\nexport ARBORIA_OUT=\"{out}\"\necho hi\n",
            crate::slurm::sbatch_value(&log)
        )
    }

    #[test]
    fn fake_lifecycle_dependencies_and_clock() {
        let tmp = Tmp::new("fake");
        let f = fake(&tmp);
        f.script(
            &name("r001", 1),
            Scenario {
                run_s: 100,
                queue_s: 10,
                log: "step 1 loss 2.0\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        f.script(
            &name("r001", 2),
            Scenario {
                state: JobState::Failed,
                run_s: 50,
                billed: Some(0.5),
                out_files: [("config.yaml".to_string(), "lr: 1\n".to_string())].into(),
                ..Default::default()
            },
        )
        .unwrap();
        let j1 = f
            .sbatch(&script(&f, "r001", 1), &name("r001", 1), None)
            .unwrap();
        let j2 = f
            .sbatch(&script(&f, "r001", 2), &name("r001", 2), Some(&j1))
            .unwrap();
        assert_eq!((j1.as_str(), j2.as_str()), ("1001", "1002"));
        let st = |f: &FakeSlurm| -> Vec<(JobState, u64)> {
            f.poll(&["1001".into(), "1002".into(), "9".into()])
                .unwrap()
                .values()
                .map(|s| (s.state, s.elapsed_s))
                .collect()
        };
        assert_eq!(st(&f), [(JobState::Pending, 0), (JobState::Pending, 0)]);
        f.advance(30).unwrap();
        assert_eq!(st(&f), [(JobState::Running, 20), (JobState::Pending, 0)]);
        // the log is written when the job starts, with %u and %j filled in
        let run_dir =
            crate::slurm::run_dir(&f.profile.paths, &f.project, &RunId::parse("r001").unwrap());
        let log = format!("{run_dir}/logs/s1.1001.out");
        assert_eq!(f.tail(&log, 5).unwrap(), "step 1 loss 2.0\n");
        assert!(fake_local_path(&log).is_file());
        f.advance(100).unwrap(); // s1 ends at 110, s2 starts then and ends at 160
        assert_eq!(
            st(&f),
            [(JobState::Completed, 100), (JobState::Running, 20)]
        );
        f.advance(100).unwrap();
        let polled = f.poll(&["1002".into()]).unwrap();
        assert_eq!(
            polled["1002"],
            JobStatus {
                state: JobState::Failed,
                elapsed_s: 50,
                billed_su: Some(0.5)
            }
        );
        let local = tmp.0.join("pulled");
        let out =
            crate::slurm::out_dir(&f.profile.paths, &f.project, &RunId::parse("r001").unwrap());
        f.fetch(&out, &local).unwrap();
        assert_eq!(
            std::fs::read_to_string(local.join("config.yaml")).unwrap(),
            "lr: 1\n"
        );
        assert_eq!(f.list_dir(&out).unwrap(), ["config.yaml"]);
        assert_eq!(
            f.find_job(&name("r001", 2)).unwrap().as_deref(),
            Some("1002")
        );
        assert_eq!(f.find_job(&name("r002", 1)).unwrap(), None);
        assert!(f.calls(Some("submit")).unwrap().len() == 2);
        assert_eq!(
            f.calls(Some("submit")).unwrap()[1],
            json!(["submit", name("r001", 2), "1001"])
        );
        // the script landed at the run's sbatch path
        assert!(fake_local_path(&format!("{run_dir}/sbatch/s2.sh")).is_file());
        assert_eq!(
            f.balance().unwrap().as_deref(),
            Some("fake cluster: unlimited")
        );
    }

    #[test]
    fn fake_scripted_failures_cancel_and_need_login() {
        let tmp = Tmp::new("fake-fail");
        let f = fake(&tmp);
        let n1 = name("r002", 1);
        f.script(
            &n1,
            Scenario {
                submit_error: true,
                lose_reply: true,
                ..Default::default()
            },
        )
        .unwrap();
        let text = script(&f, "r002", 1);
        assert_eq!(
            f.sbatch(&text, &n1, None).unwrap_err().to_string(),
            format!("sbatch failed for {n1} (scripted)")
        );
        assert!(f.jobs().unwrap().is_empty());
        assert_eq!(
            f.sbatch(&text, &n1, Some("77")).unwrap_err().to_string(),
            "sbatch: invalid dependency 77"
        );
        // the reply is lost, but the job exists and find_job adopts it
        assert_eq!(
            f.sbatch(&text, &n1, None).unwrap_err().to_string(),
            format!("connection lost after sbatch {n1} (scripted)")
        );
        assert_eq!(f.find_job(&n1).unwrap().as_deref(), Some("1001"));
        f.advance(10).unwrap();
        f.cancel(&["1001".into()]).unwrap();
        let j = &f.jobs().unwrap()["1001"];
        assert_eq!(
            (j.state, j.elapsed, j.end_t),
            (JobState::Cancelled, 10, Some(10))
        );
        assert_eq!(
            f.sbatch(&text, "arb-other-1a2b-r002-s1", None)
                .unwrap_err()
                .to_string(),
            "job arb-other-1a2b-r002-s1 belongs to project other-1a2b, not wavefront-3f2a; refused"
        );
        assert!(f
            .poll(&["1;x".into()])
            .unwrap_err()
            .to_string()
            .starts_with("invalid Slurm job id(s)"));
        assert!(f
            .tail("/etc/passwd", 3)
            .unwrap_err()
            .to_string()
            .contains("outside project"));
        f.set_need_login(true).unwrap();
        let err = f.poll(&["1001".into()]).unwrap_err();
        assert!(matches!(err, Error::NeedLogin(_)));
        assert_eq!(err.to_string(), need_login_message("fake"));
    }

    #[derive(Debug, Default)]
    struct Canned {
        gate_code: i32,
        reply: RemoteOutput,
        rsync: bool,
        argv: Mutex<Vec<(Vec<String>, Option<String>)>>,
    }

    impl Runner for Canned {
        fn run(&self, argv: &[String], input: Option<&str>, _: Duration) -> Result<RemoteOutput> {
            self.argv
                .lock()
                .unwrap()
                .push((argv.to_vec(), input.map(str::to_string)));
            if argv.get(1).map(String::as_str) == Some("-O") {
                return Ok(RemoteOutput {
                    code: self.gate_code,
                    ..Default::default()
                });
            }
            Ok(self.reply.clone())
        }
        fn pipe(
            &self,
            p: &[String],
            _: &Path,
            c: &[String],
            _: Duration,
        ) -> Result<(i32, i32, String)> {
            self.argv.lock().unwrap().push(([p, c].concat(), None));
            Ok((0, 1, "tar: boom\n".into()))
        }
        fn has(&self, _: &str) -> bool {
            self.rsync
        }
    }

    fn slurm(tmp: &Tmp, canned: Canned) -> (Arc<Canned>, SshSlurm) {
        let mut profile = fake(tmp).profile;
        profile.kind = MachineKind::Slurm;
        profile.host = "deltaai".into();
        let runner = Arc::new(canned);
        let s = SshSlurm::new(profile, project(), runner.clone());
        (runner, s)
    }

    #[test]
    fn ssh_slurm_sbatch_gate_and_errors() {
        let tmp = Tmp::new("ssh");
        let (runner, s) = slurm(
            &tmp,
            Canned {
                reply: RemoteOutput {
                    stdout: "Submitted\n4523101;cluster\n".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let id = s
            .sbatch("#!/bin/bash\n", &name("r003", 2), Some("4523100"))
            .unwrap();
        assert_eq!(id, "4523101");
        let calls = runner.argv.lock().unwrap().clone();
        assert_eq!(calls[0].0, ["ssh", "-O", "check", "deltaai"]);
        let run_dir = format!("{}/remote/$USER/runs/wavefront-3f2a/r003", tmp.0.display());
        assert_eq!(
            calls[1].0,
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ControlMaster=no",
                "deltaai",
                &format!(
                    "mkdir -p \"{run_dir}/sbatch\" \"{run_dir}/logs\" && cat > \"{run_dir}/sbatch/s2.sh\" && sbatch --parsable --dependency=afterany:4523100 \"{run_dir}/sbatch/s2.sh\""
                )
            ]
        );
        assert_eq!(calls[1].1.as_deref(), Some("#!/bin/bash\n"));
        assert!(s
            .sbatch("x", &name("r003", 1), Some("1;rm -rf"))
            .unwrap_err()
            .to_string()
            .starts_with("invalid Slurm job id(s)"));

        let (_, s) = slurm(
            &tmp,
            Canned {
                reply: RemoteOutput {
                    stdout: "sbatch: error\n".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(
            s.sbatch("x", &name("r003", 1), None)
                .unwrap_err()
                .to_string(),
            format!(
                "sbatch printed no job id for {}: 'sbatch: error'",
                name("r003", 1)
            )
        );

        let (_, s) = slurm(
            &tmp,
            Canned {
                gate_code: 255,
                ..Default::default()
            },
        );
        assert!(matches!(s.poll(&["1".into()]), Err(Error::NeedLogin(_))));
        assert!(s.poll(&[]).unwrap().is_empty(), "no ids: no remote call");

        let (_, s) = slurm(
            &tmp,
            Canned {
                reply: RemoteOutput {
                    code: 1,
                    stdout: String::new(),
                    stderr: "warn\nscancel: error: Kill job error\n".into(),
                },
                ..Default::default()
            },
        );
        assert_eq!(
            s.cancel(&["7".into(), "8".into()]).unwrap_err().to_string(),
            "`scancel 7 8` on deltaai failed (exit 1): scancel: error: Kill job error"
        );
        let err = s
            .ship(&tmp.0, "abc1234", &format!("{run_dir}/code"))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("shipping abc1234 to deltaai:{run_dir}/code failed: tar: boom")
        );
        assert_eq!(
            s.ship(&tmp.0, "HEAD", &run_dir).unwrap_err().to_string(),
            "invalid commit 'HEAD'"
        );
    }

    #[test]
    fn ssh_fetch_uses_rsync_when_present() {
        let tmp = Tmp::new("fetch");
        let (runner, s) = slurm(
            &tmp,
            Canned {
                rsync: true,
                reply: RemoteOutput {
                    stdout: "/real/out/wavefront-3f2a/r001\n".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let out = format!("{}/remote/out/wavefront-3f2a/r001", tmp.0.display());
        let local = tmp.0.join("local");
        s.fetch(&out, &local).unwrap();
        let calls = runner.argv.lock().unwrap().clone();
        assert_eq!(
            calls.last().unwrap().0,
            [
                "rsync",
                "-a",
                "-e",
                "ssh -o BatchMode=yes -o ControlMaster=no",
                "deltaai:/real/out/wavefront-3f2a/r001/",
                &format!("{}/", local.display())
            ]
        );
        assert!(local.is_dir());

        let (_, s) = slurm(
            &tmp,
            Canned {
                reply: RemoteOutput {
                    stdout: "/real/o ut;rm\n".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(
            s.fetch(&out, &local).unwrap_err().to_string(),
            "unexpected remote path '/real/o ut;rm'"
        );
    }

    #[test]
    fn ssh_local_config_parses_first_value() {
        let session = SshSession::new(
            "deltaai",
            Arc::new(Canned {
                reply: RemoteOutput {
                    stdout: "user wchen38\nHostName dtai-login.example.edu\nhostname other\ncontrolpath /tmp/cm-%C\n".into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        let cfg = session.local_config().unwrap();
        assert_eq!(cfg["hostname"], "dtai-login.example.edu");
        assert_eq!(cfg["controlpath"], "/tmp/cm-%C");
    }

    #[test]
    fn system_runner_times_out_and_pipes() {
        let r = SystemRunner;
        let out = r
            .run(
                &strings(&["sh", "-c", "cat; echo err >&2"]),
                Some("in\n"),
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (0, "in\n", "err\n")
        );
        let err = r
            .run(&strings(&["sleep", "5"]), None, Duration::from_millis(100))
            .unwrap_err();
        assert_eq!(err.to_string(), "`sleep 5` timed out after 0s");
        let (a, b, _) = r
            .pipe(
                &strings(&["printf", "abc"]),
                Path::new("/"),
                &strings(&["grep", "-q", "abc"]),
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!((a, b), (0, 0));
        assert!(r.has("sh"));
        assert!(!r.has("no-such-program-arboria"));
        assert!(r
            .run(
                &strings(&["no-such-program-arboria"]),
                None,
                Duration::from_secs(1)
            )
            .unwrap_err()
            .to_string()
            .starts_with("cannot run no-such-program-arboria: "));
    }

    #[test]
    fn fake_ships_a_commit() {
        let tmp = Tmp::new("ship");
        let f = fake(&tmp);
        let repo = tmp.0.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let o = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(o.status.success(), "{o:?}");
            String::from_utf8(o.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::create_dir_all(repo.join("pkg")).unwrap();
        std::fs::write(repo.join("pkg/a.py"), "a\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "x"]);
        let commit = git(&["rev-parse", "HEAD"]);
        let run_id = RunId::parse("r001").unwrap();
        let code = crate::slurm::code_dir(&f.profile.paths, &f.project, &run_id);
        f.ship(&repo, &commit, &code).unwrap();
        assert_eq!(
            std::fs::read_to_string(fake_local_path(&code).join("pkg/a.py")).unwrap(),
            "a\n"
        );
    }
}
