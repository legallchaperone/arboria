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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{load_toml, py_repr_toml, toml_number};
use crate::records::{py_repr, py_strip};
use crate::slurm::{parse_walltime, replace_user_var};
use crate::{bail, Error, Result};

/// The profiles file name.
pub const FILENAME: &str = "clusters.toml";
/// Overrides the config dir (tests always set it).
pub const CONFIG_DIR_ENV: &str = "ARBORIA_CONFIG_DIR";
/// Remote path roles, in order (`[<machine>.paths]`).
pub const PATH_ROLES: &[&str] = &["runs", "data", "cache", "ckpt", "out"];
/// The `kind`s a profile may have today (Python's; the Rust-only kinds are refused until
/// their keys are designed, see module docs).
pub const KINDS: &[&str] = &["slurm", "fake"];
/// The keys of a `[<machine>]` table.
pub const TOP_KEYS: &[&str] = &[
    "kind",
    "host",
    "arch",
    "account",
    "gpus_per_node",
    "cpus_per_gpu",
    "mem_per_gpu_gb",
    "default_partition",
    "max_segments",
    "balance_cmd",
    "billing_per_su",
    "prelude",
    "env_activate",
    "partitions",
    "paths",
    "root",
];
/// The keys of a `[<machine>.partitions.<name>]` table.
pub const PARTITION_KEYS: &[&str] = &["max_walltime", "charge"];

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
    /// The named partition, or the default one (None or empty: Python's `name or default`).
    pub fn partition(&self, name: Option<&str>) -> Result<&Partition> {
        let name = name
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.default_partition);
        self.partitions.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.partitions.keys().map(String::as_str).collect();
            Error::Arb(format!(
                "machine {} has no partition {}; known: {}",
                py_repr(&self.name),
                py_repr(name),
                known.join(", ")
            ))
        })
    }

    /// An account is set (not empty, not a `<placeholder>`).
    pub fn account_set(&self) -> bool {
        !self.account.is_empty() && !self.account.contains('<') && !self.account.contains('>')
    }
}

/// `$ARBORIA_CONFIG_DIR` or `~/.config/arboria`.
pub fn config_dir() -> PathBuf {
    match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::home_dir()
            .unwrap_or_default()
            .join(".config")
            .join("arboria"),
    }
}

/// `<config dir>/clusters.toml`.
pub fn config_path() -> PathBuf {
    config_dir().join(FILENAME)
}

/// `^[A-Za-z0-9][A-Za-z0-9<extra>]*$` on the whole string.
pub(crate) fn name_ok(s: &str, extra: &[u8]) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// Validate a remote absolute path (`where_` names it in errors).
pub fn check_remote_path(value: &toml::Value, where_: &str) -> Result<String> {
    let value = match value {
        toml::Value::String(s) if !s.is_empty() => s.as_str(),
        _ => bail!("{where_} must be a non-empty string"),
    };
    if value.contains(['<', '>']) {
        bail!(
            "{where_} = {} still has a <placeholder>; fill it in (TO CONFIRM at machine init)",
            py_repr(value)
        );
    }
    if !value.starts_with('/') {
        bail!("{where_} = {} must be an absolute path", py_repr(value));
    }
    // `$USER` stands for one plain character
    let plain = replace_user_var(value, "u");
    let parts: Vec<&str> = plain.split('/').filter(|p| !p.is_empty()).collect();
    // ^[A-Za-z0-9._-]+$ on the whole segment: no newline gets into #SBATCH lines
    let segment_ok = |p: &str| {
        let b = p.as_bytes();
        !b.is_empty()
            && b.iter()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    };
    if parts.is_empty()
        || parts
            .iter()
            .any(|p| *p == "." || *p == ".." || !segment_ok(p))
    {
        bail!(
            "{where_} = {}: use only letters, digits, '.', '_', '-', '/' and $USER",
            py_repr(value)
        );
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn int_at_least(v: Option<&toml::Value>, where_: &str, minimum: i64, hint: &str) -> Result<u32> {
    match v {
        Some(toml::Value::Integer(i)) if *i >= minimum => u32::try_from(*i)
            .map_err(|_| Error::Arb(format!("{where_} must be an integer >= {minimum}{hint}"))),
        _ => bail!("{where_} must be an integer >= {minimum}{hint}"),
    }
}

fn lines(v: Option<&toml::Value>, where_: &str) -> Result<Vec<String>> {
    let items = match v {
        None => return Ok(Vec::new()),
        Some(toml::Value::Array(items)) => items,
        Some(_) => bail!("{where_} must be a list of strings (shell lines)"),
    };
    let strings: Option<Vec<String>> = items.iter().map(|i| i.as_str().map(String::from)).collect();
    let Some(strings) = strings else {
        bail!("{where_} must be a list of strings (shell lines)");
    };
    if strings.iter().any(|s| s.contains(['\n', '\r'])) {
        bail!("{where_}: put one shell line per list item");
    }
    Ok(strings)
}

fn string_or<'a>(v: Option<&'a toml::Value>, default: &'a str) -> Option<&'a str> {
    match v {
        None => Some(default),
        Some(v) => v.as_str(),
    }
}

/// Validate one `[name]` table.
pub fn parse_profile(name: &str, raw: &toml::Value, source: &str) -> Result<Profile> {
    let w = format!("{source}: [{name}]");
    let Some(raw) = raw.as_table() else {
        bail!("{w} must be a table");
    };
    if !name_ok(name, b"._-") {
        bail!("{w}: invalid machine name");
    }
    if raw.contains_key("charge_cmd") {
        bail!(
            "{w} charge_cmd is gone: set billing_per_su (sacct billing units that cost one SU \
             per hour; 1000 on DeltaAI) instead"
        );
    }
    let mut unknown: Vec<&str> = raw
        .keys()
        .map(String::as_str)
        .filter(|k| !TOP_KEYS.contains(k))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        bail!("{w}: unknown key(s) {}", unknown.join(", "));
    }
    let kind = match string_or(raw.get("kind"), "slurm") {
        Some("slurm") => MachineKind::Slurm,
        Some("fake") => MachineKind::Fake,
        _ => bail!("{w} kind must be one of: {}", KINDS.join(", ")),
    };
    let host = raw.get("host");
    let host_str = string_or(host, "");
    if kind == MachineKind::Slurm && !host_str.is_some_and(|h| name_ok(h, b"._-")) {
        bail!("{w} host must be an ~/.ssh/config alias (letters, digits, . _ -)");
    }
    let root = raw.get("root");
    let root = if kind == MachineKind::Fake {
        match root.and_then(toml::Value::as_str) {
            Some(r) if !r.is_empty() => Some(r.to_string()),
            _ => bail!("{w} root (local state dir) is required for kind = \"fake\""),
        }
    } else if root.is_some() {
        bail!("{w} root is only for kind = \"fake\"");
    } else {
        None
    };
    let Some(account) = string_or(raw.get("account"), "") else {
        bail!("{w} account must be a string");
    };
    // a <placeholder> counts as unset, so it may contain spaces
    if !account.contains('<') && py_strip(account).chars().any(crate::records::py_isspace) {
        bail!("{w} account must be a single word");
    }
    let confirm = " (TO CONFIRM at machine init)";
    let gpus_per_node = int_at_least(
        raw.get("gpus_per_node"),
        &format!("{w} gpus_per_node"),
        1,
        "",
    )?;
    let cpus_per_gpu = int_at_least(
        raw.get("cpus_per_gpu"),
        &format!("{w} cpus_per_gpu"),
        1,
        confirm,
    )?;
    let mem_per_gpu_gb = match raw.get("mem_per_gpu_gb").and_then(toml_number) {
        Some(m) if m > 0.0 => m,
        _ => bail!("{w} mem_per_gpu_gb must be a number > 0{confirm}"),
    };
    let max_segments = match raw.get("max_segments") {
        None => 4,
        v => int_at_least(v, &format!("{w} max_segments"), 1, "")?,
    };
    let billing_per_su = match raw.get("billing_per_su") {
        None => 0.0,
        Some(v) => match toml_number(v) {
            Some(b) if b >= 0.0 => b,
            _ => bail!("{w} billing_per_su must be a number >= 0 (0 = estimate locally)"),
        },
    };
    let mut strings = [String::new(), String::new()];
    for (slot, key) in strings.iter_mut().zip(["arch", "balance_cmd"]) {
        match string_or(raw.get(key), "") {
            Some(s) => *slot = s.to_string(),
            None => bail!("{w} {key} must be a string"),
        }
    }
    let [arch, balance_cmd] = strings;

    let parts_raw = match raw.get("partitions").and_then(toml::Value::as_table) {
        Some(t) if !t.is_empty() => t,
        _ => bail!("{w}: at least one [{name}.partitions.<name>] table is required"),
    };
    let mut partitions = BTreeMap::new();
    // file order (toml's preserve_order), like the Python dict: the first bad one is reported
    for (pname, p) in parts_raw {
        let pw = format!("{source}: [{name}.partitions.{pname}]");
        let Some(p) = p.as_table().filter(|_| name_ok(pname, b"_-")) else {
            bail!("{pw}: invalid partition");
        };
        let mut bad: Vec<&str> = p
            .keys()
            .map(String::as_str)
            .filter(|k| !PARTITION_KEYS.contains(k))
            .collect();
        bad.sort_unstable();
        if !bad.is_empty() {
            bail!("{pw}: unknown key(s) {}", bad.join(", "));
        }
        let max_walltime_s = match p.get("max_walltime") {
            None => parse_walltime(""),
            Some(toml::Value::String(s)) => parse_walltime(s),
            Some(other) => Err(Error::Arb(format!(
                "invalid walltime {}",
                py_repr_toml(other)
            ))),
        }
        .map_err(|e| Error::Arb(format!("{pw} max_walltime: {e}")))?;
        let charge = match p.get("charge") {
            None => 1.0,
            Some(v) => match toml_number(v) {
                Some(c) if c > 0.0 => c,
                _ => bail!("{pw} charge must be a number > 0"),
            },
        };
        partitions.insert(
            pname.clone(),
            Partition {
                name: pname.clone(),
                max_walltime_s,
                charge,
            },
        );
    }
    let empty = toml::Value::String(String::new());
    let default_partition = raw.get("default_partition").unwrap_or(&empty);
    let Some(default_partition) = default_partition
        .as_str()
        .filter(|d| partitions.contains_key(*d))
        .map(String::from)
    else {
        bail!(
            "{w} default_partition {} is not one of the partitions",
            py_repr_toml(default_partition)
        );
    };

    let Some(paths_raw) = raw.get("paths").and_then(toml::Value::as_table) else {
        bail!("{w}: a [{name}.paths] table is required");
    };
    let mut bad: Vec<&str> = paths_raw
        .keys()
        .map(String::as_str)
        .filter(|k| !PATH_ROLES.contains(k))
        .collect();
    bad.sort_unstable();
    if !bad.is_empty() {
        bail!("{source}: [{name}.paths] unknown key(s) {}", bad.join(", "));
    }
    let mut paths = BTreeMap::new();
    for role in PATH_ROLES {
        let Some(v) = paths_raw.get(*role) else {
            bail!("{source}: [{name}.paths] {role} is required");
        };
        let path = check_remote_path(v, &format!("{source}: [{name}.paths] {role}"))?;
        paths.insert(role.to_string(), path);
    }

    Ok(Profile {
        name: name.to_string(),
        kind,
        host: host_str.unwrap_or_default().to_string(),
        arch,
        account: py_strip(account).to_string(),
        gpus_per_node,
        cpus_per_gpu,
        mem_per_gpu_gb,
        default_partition,
        max_segments,
        balance_cmd,
        billing_per_su,
        prelude: lines(raw.get("prelude"), &format!("{w} prelude"))?,
        env_activate: lines(raw.get("env_activate"), &format!("{w} env_activate"))?,
        partitions,
        paths,
        root,
        source: source.to_string(),
    })
}

/// The whole file, unvalidated.
pub fn load_all_raw(path: Option<&Path>) -> Result<toml::Table> {
    let path = path.map_or_else(config_path, Path::to_path_buf);
    if !path.is_file() {
        bail!(
            "no cluster profiles: {} does not exist (see examples/clusters.toml; `arb machine \
             init` will write it)",
            path.display()
        );
    }
    load_toml(&path, "cluster profiles")
}

/// Load and validate machine `name` (other profiles are not validated).
pub fn load_profile(name: &str, path: Option<&Path>) -> Result<Profile> {
    let path = path.map_or_else(config_path, Path::to_path_buf);
    let raw = load_all_raw(Some(&path))?;
    let Some(table) = raw.get(name) else {
        let mut known: Vec<&str> = raw
            .iter()
            .filter(|(_, v)| v.is_table())
            .map(|(k, _)| k.as_str())
            .collect();
        known.sort_unstable();
        let known = if known.is_empty() {
            "none".to_string()
        } else {
            known.join(", ")
        };
        bail!(
            "no machine {} in {} (known: {known})",
            py_repr(name),
            path.display()
        );
    };
    parse_profile(name, table, &path.display().to_string())
}

// ---------------------------------------------------------------- editing (the app, ADR 0004)

/// The text the app's editor starts from when there is no clusters.toml yet.
pub const TEMPLATE: &str = include_str!("../../../examples/clusters.toml");

/// One `[machine]` table as the editor shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileCheck {
    pub name: String,
    pub kind: Option<String>,
    pub host: Option<String>,
    /// Why `arb` would refuse this profile (None: it is valid).
    pub error: Option<String>,
}

/// A clusters.toml text checked the way `arb` reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileCheck {
    pub path: PathBuf,
    /// The file exists (else the text is [`TEMPLATE`]).
    pub exists: bool,
    pub text: String,
    /// The text is not TOML: why.
    pub syntax_error: Option<String>,
    pub profiles: Vec<ProfileCheck>,
}

impl FileCheck {
    /// Valid TOML and every profile valid.
    pub fn ok(&self) -> bool {
        self.syntax_error.is_none() && self.profiles.iter().all(|p| p.error.is_none())
    }
}

/// Check `text` as `<config dir>/clusters.toml`: TOML syntax, then each table with
/// [`parse_profile`] (what `arb` will say about it). Reads nothing.
pub fn check_text(text: &str) -> FileCheck {
    let path = config_path();
    let mut out = FileCheck {
        exists: path.is_file(),
        path,
        text: text.to_string(),
        syntax_error: None,
        profiles: Vec::new(),
    };
    let table = match text.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => {
            out.syntax_error = Some(e.message().to_string());
            return out;
        }
    };
    for (name, raw) in &table {
        let get = |k: &str| raw.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let error = if raw.is_table() {
            parse_profile(name, raw, FILENAME)
                .err()
                .map(|e| e.to_string())
        } else {
            Some(format!(
                "{FILENAME}: {name} is not a [{name}] table (each machine is one table)"
            ))
        };
        out.profiles.push(ProfileCheck {
            name: name.clone(),
            kind: get("kind"),
            host: get("host"),
            error,
        });
    }
    out
}

/// The current file checked, or the template when there is none.
pub fn read_checked() -> Result<FileCheck> {
    let path = config_path();
    let text = if path.is_file() {
        std::fs::read_to_string(&path)?
    } else {
        TEMPLATE.to_string()
    };
    Ok(check_text(&text))
}

/// Save `text` as clusters.toml when [`check_text`] finds nothing wrong (else nothing is
/// written); the previous file is kept as `clusters.toml.bak`; the write is atomic.
pub fn save_text(text: &str) -> Result<FileCheck> {
    let checked = check_text(text);
    if let Some(e) = &checked.syntax_error {
        bail!("{FILENAME} is not valid TOML ({e}); nothing was saved");
    }
    if let Some(p) = checked.profiles.iter().find(|p| p.error.is_some()) {
        bail!(
            "{}; nothing was saved",
            p.error.as_deref().unwrap_or_default()
        );
    }
    let path = checked.path.clone();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir)?;
    if path.is_file() {
        std::fs::copy(&path, dir.join(format!("{FILENAME}.bak")))?;
    }
    let tmp = dir.join(format!(".{FILENAME}.{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(check_text(text))
}
