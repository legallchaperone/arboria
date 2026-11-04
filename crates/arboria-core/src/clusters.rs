//! Machine profiles: `~/.config/arboria/clusters.toml` (or `$ARBORIA_CONFIG_DIR`), one
//! table per machine. Ports `arboria.clusters`.
//!
//! Invariants:
//! - Differences between machines live in the profile, not in code (ADR 0001). A profile
//!   is validated as a whole; an unknown key, a placeholder account (`<…>`) or a value
//!   marked TO CONFIRM fails with a message naming the key.
//! - Remote paths are absolute, plain characters, and `$USER` / `${USER}` as the only
//!   variable: they are put into double-quoted shell words and `#SBATCH` lines.
//! - `charge` and `mem_per_gpu_gb` are floats even when the TOML has an integer (Python
//!   `float(...)`): they reach the card hash as `1.0`, not `1`.
//!
//! Python knows the kinds `slurm` and `fake`; v1 of the Rust core adds `local`, `ssh`,
//! `runpod`, `modal` (ADR 0001). Their profile keys are not designed yet: the parser
//! must refuse them until they are.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

/// The profiles file name.
pub const FILENAME: &str = "clusters.toml";
/// Overrides the config dir (tests always set it).
pub const CONFIG_DIR_ENV: &str = "ARBORIA_CONFIG_DIR";
/// Remote path roles, in order (`[<machine>.paths]`).
pub const PATH_ROLES: &[&str] = &["runs", "data", "cache", "ckpt", "out"];

/// The machine kinds behind [`crate::backend::Backend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineKind {
    /// This computer.
    Local,
    /// A workstation over ssh (no scheduler).
    Ssh,
    /// A Slurm cluster over ssh (Python: `slurm`).
    Slurm,
    /// RunPod (API key in the keychain; the agent never sees it).
    Runpod,
    /// Modal (same).
    Modal,
    /// `FakeSlurm`: a local simulation for tests (Python: `fake`).
    Fake,
}

/// One partition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Partition {
    pub name: String,
    pub max_walltime_s: u64,
    /// Billing multiplier (float, see module docs).
    pub charge: f64,
}

/// One machine profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub kind: MachineKind,
    /// `~/.ssh/config` alias.
    pub host: String,
    pub arch: String,
    pub account: String,
    pub gpus_per_node: u32,
    pub cpus_per_gpu: u32,
    pub mem_per_gpu_gb: f64,
    pub default_partition: String,
    pub max_segments: u32,
    pub balance_cmd: String,
    /// sacct billing units that cost 1 SU per hour; 0 = estimate locally.
    pub billing_per_su: f64,
    pub prelude: Vec<String>,
    pub env_activate: Vec<String>,
    pub partitions: BTreeMap<String, Partition>,
    /// role → remote path (see [`PATH_ROLES`]).
    pub paths: BTreeMap<String, String>,
    /// FakeSlurm state dir (kind fake only).
    pub root: Option<String>,
    /// Which file it came from (messages only; not compared).
    #[serde(skip)]
    pub source: String,
}

impl Profile {
    /// The named partition, or the default one.
    pub fn partition(&self, name: Option<&str>) -> Result<&Partition> {
        todo!("port of arboria.clusters.Profile.partition")
    }

    /// An account is set (not empty, not a `<placeholder>`).
    pub fn account_set(&self) -> bool {
        !self.account.is_empty() && !self.account.contains('<') && !self.account.contains('>')
    }
}

/// `$ARBORIA_CONFIG_DIR` or `~/.config/arboria`.
pub fn config_dir() -> PathBuf {
    todo!("port of arboria.clusters.config_dir")
}

/// `<config dir>/clusters.toml`.
pub fn config_path() -> PathBuf {
    config_dir().join(FILENAME)
}

/// Validate a remote absolute path (`where_` names it in errors).
pub fn check_remote_path(value: &toml::Value, where_: &str) -> Result<String> {
    todo!("port of arboria.clusters.check_remote_path")
}

/// Validate one `[name]` table.
pub fn parse_profile(name: &str, raw: &toml::Value, source: &str) -> Result<Profile> {
    todo!("port of arboria.clusters.parse_profile")
}

/// The whole file, unvalidated.
pub fn load_all_raw(path: Option<&Path>) -> Result<toml::Table> {
    todo!("port of arboria.clusters.load_all_raw")
}

/// Load and validate machine `name` (other profiles are not validated).
pub fn load_profile(name: &str, path: Option<&Path>) -> Result<Profile> {
    todo!("port of arboria.clusters.load_profile")
}
