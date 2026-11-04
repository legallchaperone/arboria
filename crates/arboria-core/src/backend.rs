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

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::clusters::{MachineKind, Profile};
use crate::model::{JobState, ProjectId, RunId};
use crate::submission::SubmitPermit;
use crate::Result;

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
    todo!("port of arboria.backends.get_backend")
}

/// `(run id, segment)` of a job name of `project`; any other project's is refused.
pub fn own_job(project: &ProjectId, job_name: &str) -> Result<(RunId, u32)> {
    todo!("port of arboria.backends.base.Backend.own_job")
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
        return Err(crate::Error::Backend(format!(
            "invalid Slurm job id(s): {}",
            bad.join(", ")
        )));
    }
    job_ids
        .iter()
        .map(|j| crate::model::JobId::parse(j))
        .collect()
}

/// Only paths under this project's `runs/` or `out/` dirs (no `.` / `..`, plain segments).
pub fn restrict_path(profile: &Profile, project: &ProjectId, path: &str) -> Result<String> {
    todo!("port of arboria.backends.base.restrict_path")
}

/// sacct state text → [`JobState`] (`CANCELLED by 123` → cancelled, unknown → unknown).
pub fn map_slurm_state(raw: &str) -> JobState {
    todo!("port of arboria.backends.base.map_slurm_state")
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

/// Commands on `host` over the user's existing ssh master, nothing else (Python
/// `backends.ssh.SshSession`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSession {
    pub host: String,
}

/// A finished remote command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl SshSession {
    /// `ssh -O check <host>`; `Error::NeedLogin` without a live master (no retry, no prompt).
    pub fn gate(&self) -> Result<()> {
        todo!("port of arboria.backends.ssh.SshSession.gate")
    }

    /// [`SshSession::gate`], then run `remote_cmd`; exit codes outside `ok_codes` are
    /// `Error::Backend` with the last line of output.
    pub fn remote(
        &self,
        remote_cmd: &str,
        input: Option<&str>,
        ok_codes: &[i32],
    ) -> Result<RemoteOutput> {
        todo!("port of arboria.backends.ssh.SshSession.remote")
    }

    /// `ssh -G host`: the effective client config (local; opens no connection).
    pub fn local_config(&self) -> Result<BTreeMap<String, String>> {
        todo!("port of arboria.backends.ssh.SshSession.local_config")
    }
}

// ---------------------------------------------------------------- built-in machines

macro_rules! stub_backend {
    ($(#[$m:meta])* $name:ident, $kind:expr, $what:literal) => {
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
            fn prepare_env(&self, spec: &EnvSpec) -> Result<EnvReport> {
                todo!(concat!($what, ".prepare_env"))
            }
            fn ship(&self, repo: &Path, commit: &str, remote_code_dir: &str) -> Result<()> {
                todo!(concat!($what, ".ship"))
            }
            fn submit(&self, permit: &SubmitPermit<'_>, script: &str, job_name: &str, dependency: Option<&str>) -> Result<String> {
                todo!(concat!($what, ".submit"))
            }
            fn find_job(&self, job_name: &str) -> Result<Option<String>> {
                todo!(concat!($what, ".find_job"))
            }
            fn poll(&self, job_ids: &[String]) -> Result<BTreeMap<String, JobStatus>> {
                todo!(concat!($what, ".poll"))
            }
            fn cancel(&self, job_ids: &[String]) -> Result<()> {
                todo!(concat!($what, ".cancel"))
            }
            fn tail(&self, remote_path: &str, n: usize) -> Result<String> {
                todo!(concat!($what, ".tail"))
            }
            fn list_dir(&self, remote_path: &str) -> Result<Vec<String>> {
                todo!(concat!($what, ".list_dir"))
            }
            fn fetch(&self, remote_dir: &str, local_dir: &Path) -> Result<()> {
                todo!(concat!($what, ".fetch"))
            }
            fn balance(&self) -> Result<Option<String>> {
                todo!(concat!($what, ".balance"))
            }
        }
    };
}

stub_backend!(
    /// A Slurm cluster over ssh (Python `backends.ssh.SshSlurm`): sbatch with `afterany`
    /// dependencies, `sacct` states and `billing=N`, reads limited by [`restrict_path`].
    SshSlurm, MachineKind::Slurm, "port of arboria.backends.ssh.SshSlurm"
);
stub_backend!(
    /// This computer (new in the Rust core): runs in a process group under
    /// `.arboria/runs/<run>`, no scheduler, cost 0.
    LocalMachine, MachineKind::Local, "local machine (ADR 0001)"
);
stub_backend!(
    /// A workstation over ssh without a scheduler (new): the same ssh gate as Slurm.
    SshWorkstation, MachineKind::Ssh, "ssh workstation (ADR 0001)"
);
stub_backend!(
    /// RunPod (new): API key from the keychain, never shown to the agent.
    RunPod, MachineKind::Runpod, "RunPod (ADR 0001)"
);
stub_backend!(
    /// Modal (new): token from the keychain, never shown to the agent.
    Modal, MachineKind::Modal, "Modal (ADR 0001)"
);

// ---------------------------------------------------------------- FakeSlurm

/// A local Slurm simulation for tests: state in a JSON file under `root`, outcomes
/// scripted per job name, a manual clock. Ports `arboria.backends.fake.FakeSlurm`.
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
        todo!("port of arboria.backends.fake.FakeSlurm.__init__")
    }

    /// Script the outcome of the job named `job_name` (state, elapsed, billed SU, log text).
    pub fn script(
        &self,
        job_name: &str,
        state: JobState,
        elapsed_s: u64,
        billed_su: Option<f64>,
        log: &str,
    ) -> Result<()> {
        todo!("port of arboria.backends.fake.FakeSlurm.script")
    }

    /// Advance the manual clock.
    pub fn advance(&self, seconds: u64) -> Result<()> {
        todo!("port of arboria.backends.fake.FakeSlurm.advance")
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
    fn prepare_env(&self, spec: &EnvSpec) -> Result<EnvReport> {
        todo!("FakeSlurm.prepare_env (new in the Rust core)")
    }
    fn ship(&self, repo: &Path, commit: &str, remote_code_dir: &str) -> Result<()> {
        todo!("port of arboria.backends.fake.FakeSlurm.ship")
    }
    fn submit(
        &self,
        permit: &SubmitPermit<'_>,
        script: &str,
        job_name: &str,
        dependency: Option<&str>,
    ) -> Result<String> {
        todo!("port of arboria.backends.fake.FakeSlurm.submit")
    }
    fn find_job(&self, job_name: &str) -> Result<Option<String>> {
        todo!("port of arboria.backends.fake.FakeSlurm.find_job")
    }
    fn poll(&self, job_ids: &[String]) -> Result<BTreeMap<String, JobStatus>> {
        todo!("port of arboria.backends.fake.FakeSlurm.poll")
    }
    fn cancel(&self, job_ids: &[String]) -> Result<()> {
        todo!("port of arboria.backends.fake.FakeSlurm.cancel")
    }
    fn tail(&self, remote_path: &str, n: usize) -> Result<String> {
        todo!("port of arboria.backends.fake.FakeSlurm.tail")
    }
    fn list_dir(&self, remote_path: &str) -> Result<Vec<String>> {
        todo!("port of arboria.backends.fake.FakeSlurm.list_dir")
    }
    fn fetch(&self, remote_dir: &str, local_dir: &Path) -> Result<()> {
        todo!("port of arboria.backends.fake.FakeSlurm.fetch")
    }
    fn balance(&self) -> Result<Option<String>> {
        todo!("port of arboria.backends.fake.FakeSlurm.balance")
    }
}
