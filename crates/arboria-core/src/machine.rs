//! `arb machine init` and `arb doctor`: measure a Slurm machine over the user's ssh
//! master and check a profile. Ports `arboria.machine` (the parsers are pure; the probe
//! script runs through [`crate::backend::SshSession`]).
//!
//! Invariants:
//! - Read-only on the cluster: one fixed script per call ([`PROBE_SCRIPT`]), plus
//!   `test -d/-w` on the profile's own paths for doctor. Nothing edits `~/.ssh/config`;
//!   a missing ControlMaster setup is reported with the block to add ([`SSH_BLOCK`]).
//! - `ssh -O check` gates every call; NeedLogin is never retried.
//! - The proposal keeps the user's own choices from an existing profile; `--write
//!   --replace` keeps a backup of `clusters.toml`.
//! - Parsers are tested against the real DeltaAI output
//!   (`tests/fixtures/deltaai_probe.txt` on the Python side).
//! - "probe" here is the machine-measuring script, unrelated to [`crate::probe`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::backend::{need_login_message, Runner, SshSession, SystemRunner};
use crate::canonical::python_float_repr;
use crate::clusters::{self, Profile, PATH_ROLES};
use crate::records::{py_isspace, py_repr, py_splitlines, py_strip};
use crate::slurm::{dq, parse_walltime, py_round};
use crate::{bail, Error, Result};

/// The fixed read-only measuring script (Python `machine.PROBE_SCRIPT`, verbatim).
pub const PROBE_SCRIPT: &str = r#"echo @@user; echo "$USER"
echo @@arch; uname -m
echo @@accounts; accounts 2>&1
echo @@partitions; scontrol show partition -o 2>&1
echo @@node; n=$(sinfo -h -N -o '%N %P' 2>/dev/null | awk '$2 ~ /\*$/ {print $1; exit}'); [ -n "$n" ] && scontrol show node -o "$n" 2>&1
echo @@flags; scontrol show config 2>/dev/null | grep -E '^PriorityFlags'
echo @@quota; quota 2>&1
echo @@tools; for t in sbatch sacct accounts jobcharge apptainer uv pixi; do command -v "$t"; done
echo @@end
"#;

/// The `~/.ssh/config` block suggested when the master setup is missing.
pub const SSH_BLOCK: &str = "Host {host}
    HostName <login node>
    User <user>
    ControlMaster auto
    ControlPath ~/.ssh/cm-%C
    ControlPersist 7d
    ServerAliveInterval 60";

/// CPU / memory shares from the node vs from the billing weights may differ by this.
const SHARE_TOL: f64 = 0.01;

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(py_isspace).filter(|w| !w.is_empty())
}

/// `@@name` sections of the probe output (each stripped).
pub fn split_sections(text: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in py_splitlines(text) {
        let t = py_strip(line);
        let section = t
            .strip_prefix("@@")
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase()));
        if let Some(name) = section {
            out.entry(name.to_string()).or_default();
            current = Some(name.to_string());
        } else if let Some(c) = &current {
            out.get_mut(c).expect("entry").push(line);
        }
    }
    out.into_iter()
        .map(|(k, v)| (k, py_strip(&v.join("\n")).to_string()))
        .collect()
}

/// `scontrol show … -o` line → key / value (the first `=` splits; the first key wins).
pub fn parse_kv(line: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for token in words(line) {
        if let Some((k, v)) = token.split_once('=') {
            if !k.is_empty() && !out.contains_key(k) {
                out.insert(k.to_string(), v.to_string());
            }
        }
    }
    out
}

/// `scontrol show partition -o` → partition name → key/value.
pub fn parse_partitions(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut parts = BTreeMap::new();
    for line in py_splitlines(text) {
        let kv = parse_kv(line);
        if let Some(name) = kv.get("PartitionName").cloned() {
            parts.insert(name, kv);
        }
    }
    parts
}

/// `([0-9]*\.?[0-9]+)([KMGT]?)` on the whole string → (number, unit).
fn weight_value(v: &str) -> Option<(f64, char)> {
    let (num, unit) = match v.chars().last() {
        Some(c @ ('K' | 'M' | 'G' | 'T')) => (&v[..v.len() - 1], c),
        _ => (v, ' '),
    };
    let (int, frac) = match num.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (num, None),
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let ok = match frac {
        None => !int.is_empty() && digits(int),
        Some(f) => !f.is_empty() && digits(int) && digits(f),
    };
    ok.then(|| num.parse().ok().map(|n| (n, unit)))?
}

/// `TRESBillingWeights` → weights per TRES (`cpu`, `mem` per GB, `gpu`).
pub fn parse_weights(text: &str) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for item in text.split(',') {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        let key = py_strip(key).to_lowercase();
        let Some((number, unit)) = weight_value(py_strip(value)) else {
            continue;
        };
        match key.as_str() {
            "cpu" => {
                out.insert("cpu".into(), number);
            }
            "mem" => {
                let per_gb = match unit {
                    'K' => 1024.0 * 1024.0,
                    'M' | ' ' => 1024.0,
                    'G' => 1.0,
                    _ => 1.0 / 1024.0,
                };
                out.insert("mem".into(), number * per_gb);
            }
            "gres/gpu" | "gpu" => {
                out.insert("gpu".into(), number);
            }
            _ => {}
        }
    }
    out
}

/// `accounts` (NCSA) → `[(account, balance)]` from the rows under the dashed rule. Also
/// the built-in `balance` probe's parser (ADR 0003).
pub fn parse_accounts(text: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut seen_rule = false;
    for line in py_splitlines(text) {
        let rule = line.chars().count() >= 6
            && line.chars().all(|c| c == '-' || py_isspace(c))
            && line.contains('-');
        if rule {
            seen_rule = true;
            continue;
        }
        if seen_rule && !py_strip(line).is_empty() {
            let cols: Vec<&str> = words(line).collect();
            rows.push((
                cols[0].to_string(),
                cols.get(1).map_or(String::new(), |c| c.to_string()),
            ));
        }
    }
    rows
}

/// Absolute paths in `quota` output (`| /path |`).
pub fn parse_quota_paths(text: &str) -> Vec<String> {
    let re = Regex::new(r"\|\s*(/\S+?)\s*\|").expect("valid regex");
    re.captures_iter(text)
        .map(|c| c[1].to_string())
        .filter(|p| p.starts_with('/'))
        .collect()
}

/// `48h`, `90m` or `45s`.
pub fn walltime_text(seconds: u64) -> String {
    if seconds.is_multiple_of(3600) {
        format!("{}h", seconds / 3600)
    } else if seconds.is_multiple_of(60) {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

fn num(text: Option<&String>) -> Option<f64> {
    text.filter(|t| !t.is_empty())
        .and_then(|t| py_strip(t).parse().ok())
}

fn allows(part: &BTreeMap<String, String>, account: &str) -> bool {
    let allowed = part.get("AllowAccounts").map_or("ALL", String::as_str);
    let denied = part.get("DenyAccounts").map_or("", String::as_str);
    if !account.is_empty() && denied.split(',').any(|a| a == account) {
        return false;
    }
    allowed == "ALL" || (!account.is_empty() && allowed.split(',').any(|a| a == account))
}

/// A proposed profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    /// The profile's keys and values in Python's order (rendered as TOML).
    pub values: Map<String, Value>,
    pub warnings: Vec<String>,
    pub other_partitions: Vec<String>,
    pub accounts: Vec<(String, String)>,
    pub tools: Vec<String>,
}

fn get_str<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

fn tools_of(sections: &BTreeMap<String, String>) -> Vec<String> {
    py_splitlines(sections.get("tools").map_or("", String::as_str))
        .into_iter()
        .filter(|l| l.starts_with('/'))
        .map(|l| l.rsplit('/').next().unwrap_or_default().to_string())
        .collect()
}

/// Profile values from a probe's sections; keeps the user's choices from `existing`
/// (the profile's raw table, as JSON).
pub fn propose(
    name: &str,
    host: &str,
    sections: &BTreeMap<String, String>,
    account: Option<&str>,
    existing: Option<&Map<String, Value>>,
) -> Result<Proposal> {
    let _ = name;
    let empty = Map::new();
    let existing = existing.unwrap_or(&empty);
    let section = |k: &str| sections.get(k).map_or("", String::as_str);
    let mut warnings = Vec::new();
    let accounts = parse_accounts(section("accounts"));
    let names: Vec<&str> = accounts.iter().map(|(a, _)| a.as_str()).collect();
    let old_account = get_str(existing, "account").unwrap_or("");
    let account: String = match account.filter(|a| !a.is_empty()) {
        Some(a) => {
            if !names.is_empty() && !names.contains(&a) {
                bail!(
                    "--account {a}: not one of your accounts ({})",
                    names.join(", ")
                );
            }
            a.to_string()
        }
        None if !old_account.is_empty()
            && !old_account.contains('<')
            && (names.is_empty() || names.contains(&old_account)) =>
        {
            old_account.to_string()
        }
        None if names.len() == 1 => names[0].to_string(),
        None => {
            warnings.push(if names.is_empty() {
                "`accounts` listed no account: set account by hand".to_string()
            } else {
                format!("several accounts ({}): pass --account", names.join(", "))
            });
            "<from `accounts`>".to_string()
        }
    };

    let parts = parse_partitions(section("partitions"));
    if parts.is_empty() {
        bail!("`scontrol show partition` printed no partitions; is this a Slurm login node?");
    }
    // Python: the first partition (file order) with Default=YES
    let default = py_splitlines(section("partitions"))
        .into_iter()
        .map(parse_kv)
        .find(|kv| {
            kv.contains_key("PartitionName") && kv.get("Default").is_some_and(|d| d == "YES")
        })
        .and_then(|kv| kv.get("PartitionName").cloned());
    let usable: BTreeMap<&String, &BTreeMap<String, String>> = parts
        .iter()
        .filter(|(_, kv)| {
            allows(kv, &account)
                && kv.get("State").map_or("UP", String::as_str) == "UP"
                && kv.get("Hidden").map_or("NO", String::as_str) != "YES"
        })
        .collect();
    let Some(default) = default.clone().filter(|d| usable.contains_key(d)) else {
        let shown = match &default {
            Some(d) => py_repr(d),
            None => "None".into(),
        };
        bail!("the default partition {shown} is not usable with account {account}");
    };
    let dkv = usable[&default];
    let base_w = parse_weights(dkv.get("TRESBillingWeights").map_or("", String::as_str));
    let gpu_w = base_w.get("gpu").copied().unwrap_or(0.0);

    let node = parse_kv(section("node"));
    let gres = Regex::new(r"(?:^|,)gpu(?::[^:,()]+)?:([0-9]+)").expect("valid regex");
    let Some(m) = gres.captures(node.get("Gres").map_or("", String::as_str)) else {
        bail!("cannot read the GPU count of a {default} node from `scontrol show node`");
    };
    let gpus: u64 = m[1].parse().map_err(|_| Error::arb("bad GPU count"))?;
    let cpus_node = num(node.get("CPUTot")).unwrap_or(0.0) as u64;
    let alloc_mb =
        num(node.get("RealMemory")).unwrap_or(0.0) - num(node.get("MemSpecLimit")).unwrap_or(0.0);
    let job_defaults = parse_kv(
        &dkv.get("JobDefaults")
            .map_or(String::new(), |j| j.replace(',', " ")),
    );
    let def_mem_mb = num(job_defaults.get("DefMemPerGPU")).filter(|v| *v != 0.0);

    let mut cpus_per_gpu = if cpus_node != 0 { cpus_node / gpus } else { 0 };
    let mut mem_per_gpu_gb = if def_mem_mb.is_some() || alloc_mb != 0.0 {
        def_mem_mb.unwrap_or(alloc_mb / gpus as f64) / 1024.0
    } else {
        0.0
    };
    if alloc_mb > 0.0 {
        // a default whole-node request must still fit the node
        mem_per_gpu_gb = mem_per_gpu_gb.min(alloc_mb / gpus as f64 / 1024.0);
    }
    if !section("flags").contains("MAX_TRES") {
        warnings.push(
            "PriorityFlags lacks MAX_TRES: arb's billed-GPU rule (max of GPU, CPU and memory shares) may not match this cluster"
                .into(),
        );
    }
    if let Some(cpu_w) = base_w
        .get("cpu")
        .copied()
        .filter(|w| gpu_w > 0.0 && *w != 0.0)
    {
        let share = gpu_w / cpu_w;
        if cpus_per_gpu != 0 && (share - cpus_per_gpu as f64).abs() / share > SHARE_TOL {
            warnings.push(format!(
                "node gives {cpus_per_gpu} CPUs per GPU but billing makes {share:.2} CPUs one GPU; using the smaller"
            ));
        }
        let have = if cpus_per_gpu != 0 {
            cpus_per_gpu
        } else {
            1_000_000_000
        };
        cpus_per_gpu = have.min((share + 1e-6).floor() as u64);
    }
    if let Some(mem_w) = base_w
        .get("mem")
        .copied()
        .filter(|w| gpu_w > 0.0 && *w != 0.0)
    {
        let share = gpu_w / mem_w;
        if mem_per_gpu_gb != 0.0 && (share - mem_per_gpu_gb).abs() / share > SHARE_TOL {
            warnings.push(format!(
                "node gives {mem_per_gpu_gb:.3} GB per GPU but billing makes {share:.3} GB one GPU; using the smaller"
            ));
        }
        let have = if mem_per_gpu_gb != 0.0 {
            mem_per_gpu_gb
        } else {
            1e18
        };
        mem_per_gpu_gb = have.min(share);
    }
    let mem_per_gpu_gb = (mem_per_gpu_gb * 1000.0).floor() / 1000.0;
    if cpus_per_gpu < 1 || mem_per_gpu_gb <= 0.0 {
        bail!("could not measure CPUs / memory per GPU; set them by hand");
    }

    let mut partitions = Map::new();
    let mut keep = vec![default.clone()];
    let mut siblings: Vec<String> = usable
        .keys()
        .filter(|p| **p != &default && p.starts_with(&format!("{default}-")))
        .map(|p| p.to_string())
        .collect();
    siblings.sort();
    keep.extend(siblings);
    for p in &keep {
        let kv = usable[p];
        let max_time = kv.get("MaxTime").map_or("", String::as_str);
        let max_s = match parse_walltime(max_time) {
            Ok(s) => s,
            Err(e) if e.is_arb() => {
                let shown = kv.get("MaxTime").map_or("None".to_string(), |t| py_repr(t));
                warnings.push(format!(
                    "partition {p}: MaxTime {shown} is not a time limit; left out"
                ));
                continue;
            }
            Err(e) => return Err(e),
        };
        let w = parse_weights(kv.get("TRESBillingWeights").map_or("", String::as_str))
            .get("gpu")
            .copied()
            .unwrap_or(0.0);
        let charge = if gpu_w > 0.0 && w > 0.0 {
            py_round(w / gpu_w, 4)
        } else {
            1.0
        };
        let mut part = Map::new();
        part.insert("max_walltime".into(), walltime_text(max_s).into());
        part.insert("charge".into(), charge.into());
        partitions.insert(p.clone(), Value::Object(part));
    }
    let other: Vec<String> = usable
        .keys()
        .filter(|p| !partitions.contains_key(p.as_str()))
        .map(|p| p.to_string())
        .collect();

    let tools = tools_of(sections);
    let has_accounts = tools.iter().any(|t| t == "accounts");
    if !has_accounts && !existing.contains_key("balance_cmd") {
        warnings.push("no `accounts` command: balance_cmd left empty".into());
    }

    let old_paths = existing.get("paths").and_then(Value::as_object);
    let kept = old_paths.filter(|op| {
        !op.is_empty()
            && PATH_ROLES.iter().all(|r| {
                op.get(*r)
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.contains('<'))
            })
    });
    let mut paths = Map::new();
    match kept {
        Some(op) => {
            for r in PATH_ROLES {
                paths.insert(r.to_string(), op[*r].clone());
            }
        }
        None => {
            let d = default_paths(&account, &parse_quota_paths(section("quota")));
            if d.iter().any(|(_, v)| v.contains('<')) {
                warnings.push(
                    "could not tell the allocation's directories from `quota`: fill in the <proj> paths by hand"
                        .into(),
                );
            }
            for (r, v) in d {
                paths.insert(r, v.into());
            }
        }
    }

    let billing_default: Value = if gpu_w > 0.0 && gpu_w.fract() == 0.0 {
        (gpu_w as i64).into()
    } else {
        0.into()
    };
    let list_of = |k: &str| match existing.get(k) {
        Some(Value::Array(a)) => Value::Array(a.clone()),
        _ => Value::Array(Vec::new()),
    };
    let mut values = Map::new();
    values.insert("kind".into(), "slurm".into());
    values.insert("host".into(), host.into());
    values.insert("arch".into(), py_strip(section("arch")).into());
    values.insert("account".into(), account.clone().into());
    values.insert("gpus_per_node".into(), gpus.into());
    values.insert("cpus_per_gpu".into(), cpus_per_gpu.into());
    values.insert("mem_per_gpu_gb".into(), mem_per_gpu_gb.into());
    values.insert("default_partition".into(), default.clone().into());
    values.insert(
        "max_segments".into(),
        existing
            .get("max_segments")
            .cloned()
            .unwrap_or_else(|| 4.into()),
    );
    values.insert(
        "balance_cmd".into(),
        existing
            .get("balance_cmd")
            .cloned()
            .unwrap_or_else(|| if has_accounts { "accounts" } else { "" }.into()),
    );
    values.insert(
        "billing_per_su".into(),
        existing
            .get("billing_per_su")
            .cloned()
            .unwrap_or(billing_default),
    );
    values.insert("prelude".into(), list_of("prelude"));
    values.insert("env_activate".into(), list_of("env_activate"));
    values.insert("partitions".into(), Value::Object(partitions));
    values.insert("paths".into(), Value::Object(paths));
    Ok(Proposal {
        values,
        warnings,
        other_partitions: other,
        accounts,
        tools,
    })
}

/// NCSA layout: `/projects`, `/work/nvme`, `/work/hdd` per allocation (`<proj>-…`).
pub fn default_paths(account: &str, quota_paths: &[String]) -> Vec<(String, String)> {
    let mut allocs: Vec<&str> = quota_paths
        .iter()
        .filter_map(|p| p.strip_prefix("/projects/"))
        .filter(|a| {
            !a.is_empty()
                && a.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
        .collect();
    allocs.sort();
    allocs.dedup();
    let proj = allocs
        .iter()
        .find(|a| account == **a || account.starts_with(&format!("{a}-")))
        .copied()
        .or(if allocs.len() == 1 {
            Some(allocs[0])
        } else {
            None
        })
        .unwrap_or("<proj>");
    let have = |p: String| quota_paths.contains(&p);
    let nvme = if have(format!("/work/nvme/{proj}")) || proj == "<proj>" {
        format!("/work/nvme/{proj}")
    } else {
        format!("/projects/{proj}")
    };
    let hdd = if have(format!("/work/hdd/{proj}")) || proj == "<proj>" {
        format!("/work/hdd/{proj}")
    } else {
        format!("/projects/{proj}")
    };
    vec![
        (
            "runs".into(),
            format!("/projects/{proj}/$USER/arboria/runs"),
        ),
        ("data".into(), format!("{nvme}/$USER/data")),
        (
            "cache".into(),
            format!("/projects/{proj}/$USER/arboria/cache"),
        ),
        ("ckpt".into(), format!("{hdd}/$USER/arboria/ckpt")),
        ("out".into(), format!("/projects/{proj}/$USER/arboria/out")),
    ]
}

// ------------------------------------------------------------------ TOML text

fn toml_value(v: &Value) -> Result<String> {
    Ok(match v {
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) if !n.is_f64() => i.to_string(),
            _ => python_float_repr(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(toml_value)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        other => bail!("cannot write {other} into clusters.toml"),
    })
}

fn key(k: &str) -> Result<String> {
    if !k.is_empty()
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(k.to_string())
    } else {
        toml_value(&Value::String(k.into()))
    }
}

/// The `[name]` TOML table text (values in the proposal's order; `note` as a comment).
pub fn render_profile(name: &str, proposal: &Proposal, note: &str) -> String {
    render_values(name, &proposal.values, note).expect("propose makes TOML-able values")
}

fn render_values(name: &str, values: &Map<String, Value>, note: &str) -> Result<String> {
    let mut lines = Vec::new();
    if !note.is_empty() {
        lines.push(format!("# {note}"));
    }
    let n = key(name)?;
    lines.push(format!("[{n}]"));
    for (k, v) in values {
        if k != "partitions" && k != "paths" {
            lines.push(format!("{k} = {}", toml_value(v)?));
        }
    }
    if let Some(parts) = values.get("partitions").and_then(Value::as_object) {
        for (pname, p) in parts {
            lines.push(format!("[{n}.partitions.{}]", key(pname)?));
            for (k, v) in p.as_object().into_iter().flatten() {
                lines.push(format!("{k} = {}", toml_value(v)?));
            }
        }
    }
    lines.push(format!("[{n}.paths]"));
    for r in PATH_ROLES {
        lines.push(format!("{r} = {}", toml_value(&values["paths"][*r])?));
    }
    Ok(lines.join("\n") + "\n")
}

/// Machine name of a `[name]` / `[name.x]` header line, else None.
fn header_machine(line: &str) -> Option<String> {
    let t = line.trim_start_matches(py_isspace);
    if t.starts_with("[[") {
        return None;
    }
    let t = t.strip_prefix('[')?.trim_start_matches(py_isspace);
    if let Some(rest) = t.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    let n: String = t
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (!n.is_empty()).then_some(n)
}

/// `text` with machine `name`'s tables (and the comments just above them) replaced by
/// `new_block`, or `new_block` appended.
pub fn replace_table(text: &str, name: &str, new_block: &str) -> Result<String> {
    let block = new_block.trim_end_matches('\n');
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    let mut placed = false;
    for line in py_splitlines(text) {
        if let Some(machine) = header_machine(line) {
            inside = machine == name;
            if inside {
                if !placed {
                    while out
                        .last()
                        .is_some_and(|l| l.trim_start_matches(py_isspace).starts_with('#'))
                    {
                        out.pop();
                    }
                    out.push(block);
                    placed = true;
                }
                continue;
            }
        }
        if !inside {
            out.push(line);
        }
    }
    if !placed {
        if out.last().is_some_and(|l| !py_strip(l).is_empty()) {
            out.push("");
        }
        out.push(block);
    }
    Ok(out.join("\n").trim_end_matches('\n').to_string() + "\n")
}

// ------------------------------------------------------------------ commands

fn session(host: &str, runner: Arc<dyn Runner>) -> Result<SshSession> {
    if !clusters::name_ok(host, b"._-") {
        bail!("invalid ssh host alias {}", py_repr(host));
    }
    Ok(SshSession::new(host, runner))
}

/// What is missing from the effective ssh config for a reusable master connection.
pub fn ssh_setup_problems(cfg: &BTreeMap<String, String>) -> Vec<String> {
    let get = |k: &str, d: &'static str| cfg.get(k).map_or(d, String::as_str);
    let mut problems = Vec::new();
    if !matches!(
        get("controlmaster", "no"),
        "auto" | "yes" | "autoask" | "ask"
    ) {
        problems.push("ControlMaster is off".into());
    }
    if matches!(get("controlpath", ""), "" | "none") {
        problems.push("no ControlPath".into());
    }
    if matches!(get("controlpersist", "no"), "no" | "false" | "0") {
        problems.push("ControlPersist is off (each command would need a new Duo login)".into());
    }
    problems
}

/// One fixed read-only script on the login node; NeedLogin without a master.
pub fn probe(session: &SshSession) -> Result<BTreeMap<String, String>> {
    let out = session.remote("bash -s", Some(PROBE_SCRIPT), &[0])?;
    let sections = split_sections(&out.stdout);
    if !sections.contains_key("end") {
        let tail: String = {
            let chars: Vec<char> = out.stdout.chars().collect();
            chars[chars.len().saturating_sub(2000)..].iter().collect()
        };
        bail!("the probe script did not finish; output:\n{tail}");
    }
    Ok(sections)
}

/// Options of `arb machine init`.
#[derive(Debug, Clone, Default)]
pub struct InitOptions<'a> {
    pub host: Option<&'a str>,
    pub account: Option<&'a str>,
    pub write: bool,
    pub replace: bool,
}

/// What `arb machine init` did.
#[derive(Debug, Clone, Serialize)]
pub struct InitResult {
    pub name: String,
    pub text: String,
    pub proposal: Proposal,
    pub path: PathBuf,
    pub written: bool,
    pub backup: Option<PathBuf>,
    pub ssh_problems: Vec<String>,
    pub still_invalid: Option<String>,
}

/// A TOML value as JSON (datetimes as their text).
pub fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => s.clone().into(),
        toml::Value::Integer(i) => (*i).into(),
        toml::Value::Float(f) => (*f).into(),
        toml::Value::Boolean(b) => (*b).into(),
        toml::Value::Datetime(d) => d.to_string().into(),
        toml::Value::Array(a) => a.iter().map(toml_to_json).collect::<Vec<_>>().into(),
        toml::Value::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

/// `arb machine init NAME`.
pub fn machine_init(name: &str, opts: InitOptions<'_>) -> Result<InitResult> {
    machine_init_with(name, opts, Arc::new(SystemRunner), SystemTime::now())
}

/// [`machine_init`] with a given runner and clock (tests). The stamps are UTC.
pub fn machine_init_with(
    name: &str,
    opts: InitOptions<'_>,
    runner: Arc<dyn Runner>,
    now: SystemTime,
) -> Result<InitResult> {
    if !clusters::name_ok(name, b"._-") || name.contains('.') {
        bail!(
            "invalid machine name {} (letters, digits, _ and -)",
            py_repr(name)
        );
    }
    let path = clusters::config_path();
    let raw_all = if path.is_file() {
        clusters::load_all_raw(Some(&path))?
    } else {
        toml::Table::new()
    };
    let existing = raw_all.get(name).and_then(toml::Value::as_table).map(|t| {
        match toml_to_json(&toml::Value::Table(t.clone())) {
            Value::Object(m) => m,
            _ => unreachable!("a table"),
        }
    });
    if let Some(e) = &existing {
        let kind = e.get("kind").cloned().unwrap_or_else(|| "slurm".into());
        if kind != "slurm" {
            bail!(
                "machine {} in {} is kind = {}, not slurm",
                py_repr(name),
                path.display(),
                crate::canonical::python_repr(&kind)
            );
        }
    }
    let host = opts
        .host
        .filter(|h| !h.is_empty())
        .map(String::from)
        .or_else(|| {
            existing
                .as_ref()
                .and_then(|e| get_str(e, "host"))
                .filter(|h| !h.is_empty())
                .map(String::from)
        })
        .unwrap_or_else(|| name.to_string());
    let session = session(&host, runner)?;
    let ssh_problems = ssh_setup_problems(&session.local_config()?);
    let mut old = existing.clone().unwrap_or_default();
    old.remove("charge_cmd");
    let proposal = propose(name, &host, &probe(&session)?, opts.account, Some(&old))?;
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let ts = crate::db::format_utc(secs); // YYYY-MM-DDTHH:MM:SSZ
    let text = render_profile(
        name,
        &proposal,
        &format!("measured by `arb machine init {name}` {}", &ts[..10]),
    );
    let (mut written, mut backup) = (false, None);
    if opts.write {
        if existing.is_some() && !opts.replace {
            bail!(
                "{} already has [{name}]; add --replace to rewrite it (a backup is kept), or copy the values above by hand",
                path.display()
            );
        }
        std::fs::create_dir_all(path.parent().expect("has a parent"))?;
        let file_name = path
            .file_name()
            .expect("a file")
            .to_string_lossy()
            .into_owned();
        let new_text = if path.is_file() {
            let stamp: String = ts
                .chars()
                .filter(|c| *c != '-' && *c != ':' && *c != 'Z')
                .collect();
            let b = path.with_file_name(format!("{file_name}.bak-{stamp}"));
            std::fs::copy(&path, &b)?;
            backup = Some(b);
            replace_table(&std::fs::read_to_string(&path)?, name, &text)?
        } else {
            format!("# Arboria cluster profiles (see examples/clusters.toml in arboria).\n\n{text}")
        };
        let tmp = path.with_file_name(format!("{file_name}.tmp"));
        std::fs::write(&tmp, new_text)?;
        // still parses as TOML, or nothing changes
        if let Err(e) = crate::config::load_toml(&tmp, "cluster profiles") {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        std::fs::rename(&tmp, &path)?;
        written = true;
    }
    let reparsed: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| Error::Arb(e.message().to_string()))?;
    let still_invalid = clusters::parse_profile(name, &reparsed[name], &path.display().to_string())
        .err()
        .map(|e| e.to_string());
    Ok(InitResult {
        name: name.into(),
        text,
        proposal,
        path,
        written,
        backup,
        ssh_problems,
        still_invalid,
    })
}

// ------------------------------------------------------------------ doctor

/// One doctor line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// `ok` | `warn` | `fail` | `info`.
    pub level: String,
    pub what: String,
}

fn check(level: &str, what: impl Into<String>) -> Check {
    Check {
        level: level.into(),
        what: what.into(),
    }
}

/// Python `str()` of a number for messages (`4`, `110.898`).
fn py_num(v: f64) -> String {
    python_float_repr(v)
}

/// `arb doctor [NAME]`: read-only checks; stops early (fail) without an ssh master.
pub fn doctor(name: &str, project_id: Option<&crate::model::ProjectId>) -> Result<Vec<Check>> {
    doctor_with(name, project_id, Arc::new(SystemRunner))
}

/// [`doctor`] with a given runner (tests).
pub fn doctor_with(
    name: &str,
    project_id: Option<&crate::model::ProjectId>,
    runner: Arc<dyn Runner>,
) -> Result<Vec<Check>> {
    let mut checks = Vec::new();
    let path = clusters::config_path();
    let raw_all = clusters::load_all_raw(Some(&path))?;
    let Some(raw) = raw_all.get(name).filter(|v| v.is_table()) else {
        let mut known: Vec<&str> = raw_all
            .iter()
            .filter(|(_, v)| v.is_table())
            .map(|(k, _)| k.as_str())
            .collect();
        known.sort();
        let known = if known.is_empty() {
            "none".to_string()
        } else {
            known.join(", ")
        };
        bail!(
            "no machine {} in {} (known: {known}); run `arb machine init {name}`",
            py_repr(name),
            path.display()
        );
    };
    let profile: Option<Profile> =
        match clusters::parse_profile(name, raw, &path.display().to_string()) {
            Ok(p) => {
                checks.push(check(
                    "ok",
                    format!("profile [{name}] in {} is valid", path.display()),
                ));
                Some(p)
            }
            Err(e) if e.is_arb() => {
                checks.push(check("fail", e.to_string()));
                None
            }
            Err(e) => return Err(e),
        };
    if let Some(pid) = project_id {
        checks.push(check("info", format!("project id {pid}")));
    }
    let raw_kind = raw
        .get("kind")
        .and_then(toml::Value::as_str)
        .unwrap_or("slurm");
    let is_fake = match &profile {
        Some(p) => p.kind == clusters::MachineKind::Fake,
        None => raw_kind == "fake",
    };
    if is_fake {
        checks.push(check("info", "kind = \"fake\": nothing remote to check"));
        return Ok(checks);
    }
    let host = match &profile {
        Some(p) => p.host.clone(),
        None => raw.get("host").map_or(String::new(), |h| match h {
            toml::Value::String(s) => s.clone(),
            other => other.to_string(),
        }),
    };
    let session = session(&host, runner)?;
    let problems = ssh_setup_problems(&session.local_config()?);
    if problems.is_empty() {
        checks.push(check(
            "ok",
            format!("ssh {host}: ControlMaster + ControlPersist configured"),
        ));
    } else {
        checks.push(check(
            "fail",
            format!(
                "ssh {host}: {}. Add to ~/.ssh/config:\n{}",
                problems.join("; "),
                SSH_BLOCK.replace("{host}", &host)
            ),
        ));
    }
    match session.gate() {
        Ok(()) => {}
        Err(Error::NeedLogin(_)) => {
            checks.push(check("fail", need_login_message(&host)));
            return Ok(checks);
        }
        Err(e) => return Err(e),
    }
    checks.push(check("ok", format!("ssh master to {host} is up")));

    let sections = probe(&session)?;
    let section = |k: &str| sections.get(k).map_or("", String::as_str);
    let arch = py_strip(section("arch"));
    match &profile {
        Some(p) if !p.arch.is_empty() && !arch.is_empty() && arch != p.arch => checks.push(check(
            "fail",
            format!("login node is {arch}, profile says arch = {}", p.arch),
        )),
        _ if !arch.is_empty() => checks.push(check("ok", format!("login node arch {arch}"))),
        _ => {}
    }
    let tools = tools_of(&sections);
    for t in ["sbatch", "sacct"] {
        if !tools.iter().any(|x| x == t) {
            checks.push(check("fail", format!("`{t}` not found on the login node")));
        }
    }
    let shown = if tools.is_empty() {
        "none of the tools".to_string()
    } else {
        tools.join(", ")
    };
    checks.push(check("info", format!("login node has: {shown}")));
    let Some(profile) = profile else {
        return Ok(checks);
    };

    let accounts: BTreeMap<String, String> =
        parse_accounts(section("accounts")).into_iter().collect();
    let names: Vec<String> = parse_accounts(section("accounts"))
        .into_iter()
        .map(|(a, _)| a)
        .collect();
    if !accounts.is_empty() && !accounts.contains_key(&profile.account) {
        checks.push(check(
            "fail",
            format!(
                "account {} is not yours (yours: {})",
                profile.account,
                names.join(", ")
            ),
        ));
    } else if !accounts.is_empty() {
        checks.push(check(
            "ok",
            format!(
                "account {}: balance {} (hours)",
                profile.account, accounts[&profile.account]
            ),
        ));
    }
    let raw_json = match toml_to_json(raw) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    match propose(name, &host, &sections, None, Some(&raw_json)) {
        Ok(fresh) => {
            let v = &fresh.values;
            let have = [
                (
                    "gpus_per_node",
                    profile.gpus_per_node.to_string(),
                    v["gpus_per_node"].to_string(),
                ),
                (
                    "cpus_per_gpu",
                    profile.cpus_per_gpu.to_string(),
                    v["cpus_per_gpu"].to_string(),
                ),
                (
                    "mem_per_gpu_gb",
                    py_num(profile.mem_per_gpu_gb),
                    v["mem_per_gpu_gb"].as_f64().map_or(String::new(), py_num),
                ),
                (
                    "default_partition",
                    profile.default_partition.clone(),
                    v["default_partition"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ),
            ];
            for (k, a, b) in have {
                let same = if k == "mem_per_gpu_gb" {
                    v["mem_per_gpu_gb"].as_f64() == Some(profile.mem_per_gpu_gb)
                } else {
                    a == b
                };
                if !same {
                    checks.push(check("warn", format!("{k} = {a}, measured {b}")));
                }
            }
        }
        Err(e) if e.is_arb() => {
            checks.push(check(
                "warn",
                format!("could not re-measure the machine: {e}"),
            ));
        }
        Err(e) => return Err(e),
    }
    let parts = parse_partitions(section("partitions"));
    let base = parse_weights(
        parts
            .get(&profile.default_partition)
            .and_then(|p| p.get("TRESBillingWeights"))
            .map_or("", String::as_str),
    );
    let mut part_problems = 0;
    for (pname, part) in &profile.partitions {
        let Some(live) = parts.get(pname).filter(|l| allows(l, &profile.account)) else {
            checks.push(check(
                "fail",
                format!(
                    "partition {pname} does not exist or does not take account {}",
                    profile.account
                ),
            ));
            part_problems += 1;
            continue;
        };
        let live_max = parse_walltime(live.get("MaxTime").map_or("", String::as_str)).ok();
        if let Some(m) = live_max.filter(|m| part.max_walltime_s > *m) {
            checks.push(check(
                "fail",
                format!(
                    "partition {pname}: max_walltime {} > Slurm's {}",
                    walltime_text(part.max_walltime_s),
                    walltime_text(m)
                ),
            ));
            part_problems += 1;
        }
        let w = parse_weights(live.get("TRESBillingWeights").map_or("", String::as_str))
            .get("gpu")
            .copied()
            .unwrap_or(0.0);
        if let Some(g) = base.get("gpu").copied().filter(|g| *g != 0.0 && w != 0.0) {
            if (w / g - part.charge).abs() > 1e-6 {
                checks.push(check(
                    "warn",
                    format!(
                        "partition {pname}: charge = {}, billing weights say {}",
                        py_num(part.charge),
                        py_num(py_round(w / g, 4))
                    ),
                ));
                part_problems += 1;
            }
        }
    }
    if part_problems == 0 {
        let names: Vec<&str> = profile.partitions.keys().map(String::as_str).collect();
        checks.push(check(
            "ok",
            format!("partitions {} exist with these limits", names.join(", ")),
        ));
    }
    checks.extend(path_checks(&session, &profile)?);
    for line in py_splitlines(section("quota")) {
        if line.contains("/projects/") || line.contains("/work/") {
            let text = line.replace('|', " ");
            checks.push(check(
                "info",
                format!("quota {}", words(&text).collect::<Vec<_>>().join(" ")),
            ));
        }
    }
    if profile.env_activate.is_empty() {
        checks.push(check(
            "warn",
            "env_activate is empty: jobs run with the login shell's python (decide the environment, then fill it in)",
        ));
    }
    Ok(checks)
}

/// Each path, or its deepest existing parent, must be a writable directory.
fn path_checks(session: &SshSession, profile: &Profile) -> Result<Vec<Check>> {
    let mut lines = Vec::new();
    for role in PATH_ROLES {
        let target = profile.paths.get(*role).map_or("", String::as_str);
        lines.push(format!(
            "t={}; d=$t; while [ ! -e \"$d\" ] && [ \"$d\" != / ]; do d=$(dirname \"$d\"); done; \
             if [ -d \"$d\" ] && [ -w \"$d\" ]; then w=1; else w=0; fi; \
             if [ \"$d\" = \"$t\" ]; then e=1; else e=0; fi; echo \"{role} $w $e $d\"",
            dq(target)?
        ));
    }
    let out = session.remote("bash -s", Some(&(lines.join("\n") + "\n")), &[0])?;
    let mut seen: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    for line in py_splitlines(&out.stdout) {
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        if parts.len() == 4 && PATH_ROLES.contains(&parts[0]) {
            seen.insert(
                parts[0].into(),
                (parts[1].into(), parts[2].into(), parts[3].into()),
            );
        }
    }
    let mut checks = Vec::new();
    for role in PATH_ROLES {
        let (writable, exists, at) =
            seen.get(*role)
                .cloned()
                .unwrap_or(("0".into(), "0".into(), "?".into()));
        let target = profile.paths.get(*role).map_or("", String::as_str);
        if writable != "1" {
            checks.push(check(
                "fail",
                format!("paths.{role} {target}: {at} is not a writable directory"),
            ));
        } else {
            let how = if exists == "1" {
                "exists".to_string()
            } else {
                format!("will be created under {at}")
            };
            checks.push(check("ok", format!("paths.{role} {target} ({how})")));
        }
    }
    Ok(checks)
}
