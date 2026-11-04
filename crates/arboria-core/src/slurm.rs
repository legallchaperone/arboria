//! Pure Slurm logic: walltime, DeltaAI billing, segment plans, command overrides, job
//! names, remote paths, sbatch scripts. Ports `arboria.slurm` (the card hash moved to
//! [`crate::cards`]).
//!
//! Invariants:
//! - Nothing here touches the ledger, the file system or the network.
//! - Byte-compatible with Python (ADR 0001; test vectors `slurm_*`): walltime parsing,
//!   `slurm_time`, `human_duration`, segments, billed GPUs, the request dict, job names
//!   `arb-<project>-<run>-s<k>` (≤64 chars), remote paths, override rendering, and the
//!   sbatch text (including `shlex.quote` / `shlex.join` quoting and `$USER` → `%u` in
//!   `#SBATCH` lines).
//! - `est_su` is Python's `round(x, 2)`: round-half-even on the exact binary value — not
//!   `(x * 100).round() / 100`.

//!
//! Paths: the `paths` maps come from a validated profile (every role in
//! [`crate::clusters::PATH_ROLES`] present); a missing role is a bug and panics rather
//! than producing a wrong remote path.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::canonical::python_float_repr;
use crate::clusters::Profile;
use crate::model::{py_dollar, IdeaName, ProjectId, RunId};
use crate::records::{py_decimal, py_repr, py_strip};
use crate::{bail, Error, Result};

/// `--time` must be at least this long.
pub const MIN_WALLTIME_S: u64 = 60;
/// Slurm job names longer than this are refused.
pub const MAX_JOB_NAME: usize = 64;

// ---------------------------------------------------------------- walltime

/// A run of Unicode decimal digits at the start of `s` (Python `\d+`, greedy): its value
/// and the rest. None when `s` does not start with a digit; `Err` on overflow.
fn digits(s: &str) -> Option<(Option<u64>, &str)> {
    let end = s
        .char_indices()
        .find(|(_, c)| py_decimal(*c).is_none())
        .map_or(s.len(), |(i, _)| i);
    if end == 0 {
        return None;
    }
    let value = s[..end].chars().try_fold(0u64, |acc, c| {
        acc.checked_mul(10)?
            .checked_add(u64::from(py_decimal(c).expect("digit")))
    });
    Some((value, &s[end..]))
}

/// `^(?:(\d+)-)?(\d+):(\d{2}):(\d{2})$` → (days, hours, minutes, seconds).
fn parse_clock(t: &str) -> Option<[Option<u64>; 4]> {
    let two = |s: &str| -> Option<(Option<u64>, ())> {
        let mut cs = s.chars();
        let (a, b) = (py_decimal(cs.next()?)?, py_decimal(cs.next()?)?);
        cs.next()
            .is_none()
            .then_some((Some(u64::from(a * 10 + b)), ()))
    };
    let (first, rest) = digits(t)?;
    let (days, hours, rest) = match rest.strip_prefix('-') {
        Some(r) => {
            let (h, r) = digits(r)?;
            (first, h, r)
        }
        None => (Some(0), first, rest),
    };
    let rest = rest.strip_prefix(':')?;
    let (mm, ss) = rest.split_once(':')?;
    let (m, ()) = two(mm)?;
    let (s, ()) = two(ss)?;
    Some([days, hours, m, s])
}

/// `^(?:(\d+)d)?(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$` → the four parts (None = absent).
/// The digit runs are maximal and each must be followed by its unit, so this is the
/// regex's only match.
#[allow(clippy::type_complexity)]
fn parse_units(t: &str) -> Option<[Option<Option<u64>>; 4]> {
    let mut parts = [None; 4];
    let mut rest = t;
    for (i, unit) in ['d', 'h', 'm', 's'].into_iter().enumerate() {
        if let Some((v, r)) = digits(rest) {
            if let Some(r) = r.strip_prefix(unit) {
                parts[i] = Some(v);
                rest = r;
            }
        }
    }
    rest.is_empty().then_some(parts)
}

fn total_seconds(d: u64, h: u64, m: u64, s: u64) -> Option<u64> {
    d.checked_mul(24)?
        .checked_add(h)?
        .checked_mul(60)?
        .checked_add(m)?
        .checked_mul(60)?
        .checked_add(s)
}

/// Seconds from `90m`, `12h`, `1d6h`, `45s`, `HH:MM:SS` or `D-HH:MM:SS`.
pub fn parse_walltime(text: &str) -> Result<u64> {
    let t = py_strip(text).to_lowercase();
    let too_long = || Error::Arb(format!("walltime {} is too long", py_repr(text)));
    let total = if let Some(parts) = parse_clock(&t) {
        let [d, h, m, s] = parts.map(|p| p.ok_or_else(too_long));
        let (d, h, m, s) = (d?, h?, m?, s?);
        if m >= 60 || s >= 60 {
            bail!(
                "invalid walltime {}: minutes and seconds must be < 60",
                py_repr(text)
            );
        }
        total_seconds(d, h, m, s).ok_or_else(too_long)?
    } else {
        let parts = parse_units(&t).filter(|p| p.iter().any(Option::is_some));
        let Some(parts) = parts else {
            bail!(
                "invalid walltime {}; use e.g. 90m, 12h, 1d6h or HH:MM:SS",
                py_repr(text)
            );
        };
        let mut v = [0u64; 4];
        for (slot, p) in v.iter_mut().zip(parts) {
            *slot = p.unwrap_or(Some(0)).ok_or_else(too_long)?;
        }
        total_seconds(v[0], v[1], v[2], v[3]).ok_or_else(too_long)?
    };
    if total == 0 {
        bail!("walltime {} must be > 0", py_repr(text));
    }
    Ok(total)
}

/// `--time` value: `HH:MM:SS` (hours may exceed 24).
pub fn slurm_time(seconds: u64) -> String {
    let (h, rest) = (seconds / 3600, seconds % 3600);
    format!("{h:02}:{:02}:{:02}", rest / 60, rest % 60)
}

/// Compact form for cards: `1d6h`, `12h`, `1h30m`, `90s`, `0s`.
pub fn human_duration(seconds: u64) -> String {
    let parts = [
        (seconds / 86400, 'd'),
        (seconds % 86400 / 3600, 'h'),
        (seconds % 3600 / 60, 'm'),
        (seconds % 60, 's'),
    ];
    let out: String = parts
        .iter()
        .filter(|(v, _)| *v != 0)
        .map(|(v, u)| format!("{v}{u}"))
        .collect();
    if out.is_empty() {
        "0s".into()
    } else {
        out
    }
}

/// Split a walltime longer than the partition limit into chained segments.
pub fn segment_plan(total_s: u64, max_s: u64, max_segments: u32) -> Result<Vec<u64>> {
    // Python: max(1, math.ceil(total_s / max_s)) with true division
    let n = ((total_s as f64 / max_s as f64).ceil() as u64).max(1);
    if n > u64::from(max_segments) {
        bail!(
            "walltime {} needs {n} segments of at most {} (partition limit), more than \
             max_segments = {max_segments}; shorten the run or raise max_segments in clusters.toml",
            human_duration(total_s),
            human_duration(max_s)
        );
    }
    let n = n as usize;
    let mut out = vec![max_s; n - 1];
    out.push(total_s.saturating_sub(max_s * (n as u64 - 1)));
    Ok(out)
}

// ---------------------------------------------------------------- billing

/// DeltaAI bills the max of GPUs, memory and CPUs, each in GPU shares (ceil with 1e-9 slack).
pub fn billed_gpus(
    gpus: u32,
    mem_gb: f64,
    cpus: u32,
    mem_per_gpu_gb: f64,
    cpus_per_gpu: u32,
) -> u32 {
    let eps = 1e-9;
    let mem = (mem_gb / mem_per_gpu_gb - eps).ceil();
    let cpu = (f64::from(cpus) / f64::from(cpus_per_gpu) - eps).ceil();
    // ceil of a tiny negative is -0.0, which casts to 0 like Python's int(-0.0)
    f64::from(gpus).max(mem).max(cpu) as u32
}

/// Python's `round(x, 2)`: the exact binary value rounded to 2 decimals, ties to even
/// (`{:.2}` does exactly that since Rust 1.67), read back as the nearest float.
pub fn py_round2(x: f64) -> f64 {
    format!("{x:.2}").parse().expect("formatted float parses")
}

// ---------------------------------------------------------------- request plan

/// What the submitter asked for (stored as is in `request_json` `ask`; the plan is
/// re-derived from it). Serializes like Python `dataclasses.asdict(Ask)`, `null`s included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub command: Vec<String>,
    pub gpus: u32,
    pub nodes: u32,
    pub walltime_s: u64,
    pub partition: String,
    pub machine: String,
    /// Per node; None = gpus × mem_per_gpu_gb.
    pub mem_gb: Option<u32>,
    /// Per node; None = gpus × cpus_per_gpu.
    pub cpus: Option<u32>,
    /// `--repeat` nonce (goes into the card hash).
    pub repeat: Option<String>,
}

/// `Plan.env`: what the job's environment is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEnv {
    pub arch: String,
    pub prelude: Vec<String>,
    pub env_activate: Vec<String>,
    pub paths: BTreeMap<String, String>,
}

/// A validated request against a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub ask: Ask,
    pub project: ProjectId,
    pub account: String,
    pub partition: String,
    pub charge: f64,
    pub max_walltime_s: u64,
    /// Requested per node.
    pub gpus_per_node: u32,
    pub cpus_per_node: u32,
    pub mem_gb_per_node: u32,
    pub billed_per_node: u32,
    /// Total over nodes.
    pub billed_gpus: u32,
    pub segments: Vec<u64>,
    pub env: PlanEnv,
}

impl Plan {
    /// Billed more GPUs than requested (memory / CPUs over the share).
    pub fn over_billed(&self) -> bool {
        self.billed_gpus > self.ask.gpus
    }

    /// Upper bound in SU: `round(Σ billed × s / 3600 × charge, 2)` (see module docs).
    pub fn est_su(&self) -> f64 {
        // Python: sum(billed * s / 3600 * charge for s in segments); billed * s is an
        // exact int, `/ 3600` a correctly rounded true division, summed left to right
        let sum = self.segments.iter().fold(0.0, |acc, &s| {
            acc + (u64::from(self.billed_gpus) * s) as f64 / 3600.0 * self.charge
        });
        py_round2(sum)
    }

    /// What is stored in `runs.request_json` (Python `Plan.request()`; key set and value
    /// types are part of the contract).
    pub fn request(&self) -> Value {
        json!({
            "ask": self.ask,
            "project": self.project,
            "command": self.ask.command,
            "machine": self.ask.machine,
            "partition": self.partition,
            "nodes": self.ask.nodes,
            "gpus": self.ask.gpus,
            "walltime_s": self.ask.walltime_s,
            "walltime": human_duration(self.ask.walltime_s),
            "account": self.account,
            "gpus_per_node": self.gpus_per_node,
            "cpus_per_node": self.cpus_per_node,
            "mem_gb_per_node": self.mem_gb_per_node,
            "billed_gpus": self.billed_gpus,
            "charge": self.charge,
            "segments": self.segments,
            "env": self.env,
            "est_su": self.est_su(),
        })
    }
}

/// Validate `ask` against the profile; refuse anything that cannot be billed right.
pub fn build_plan(profile: &Profile, ask: &Ask, project: &ProjectId) -> Result<Plan> {
    if ask.command.is_empty() || ask.command.iter().any(String::is_empty) {
        bail!("no command: put it after `--`, e.g. arb submit ... -- python train.py");
    }
    if !profile.account_set() {
        let source = if profile.source.is_empty() {
            "clusters.toml"
        } else {
            &profile.source
        };
        bail!(
            "machine {} has no account; set `account` in {source} (from `accounts` on the cluster)",
            py_repr(&profile.name)
        );
    }
    if ask.nodes < 1 {
        bail!("--nodes must be >= 1");
    }
    if ask.gpus < 1 {
        bail!("--gpus must be >= 1");
    }
    if !ask.gpus.is_multiple_of(ask.nodes) {
        bail!(
            "--gpus {} is not divisible by --nodes {}",
            ask.gpus,
            ask.nodes
        );
    }
    let per_node = ask.gpus / ask.nodes;
    if per_node > profile.gpus_per_node {
        bail!(
            "{per_node} GPUs per node > {} on {}; use more --nodes",
            profile.gpus_per_node,
            profile.name
        );
    }
    if ask.walltime_s < MIN_WALLTIME_S {
        bail!("--time must be at least {MIN_WALLTIME_S}s");
    }
    let part = profile.partition(Some(&ask.partition))?;
    // never rounded up: 4 × 110.898 → 443 (the node has 4 × 113560 MB)
    let mem = ask
        .mem_gb
        .unwrap_or_else(|| (f64::from(per_node) * profile.mem_per_gpu_gb).floor() as u32);
    let cpus = ask.cpus.unwrap_or(per_node * profile.cpus_per_gpu);
    if mem < 1 || cpus < 1 {
        bail!("--mem and --cpus must be >= 1");
    }
    let billed_node = billed_gpus(
        per_node,
        f64::from(mem),
        cpus,
        profile.mem_per_gpu_gb,
        profile.cpus_per_gpu,
    );
    if billed_node > profile.gpus_per_node {
        bail!(
            "--mem {mem}G / --cpus {cpus} per node is more than a whole {} node ({} GPU shares)",
            profile.name,
            profile.gpus_per_node
        );
    }
    let segments = segment_plan(ask.walltime_s, part.max_walltime_s, profile.max_segments)?;
    let env = PlanEnv {
        arch: profile.arch.clone(),
        prelude: profile.prelude.clone(),
        env_activate: profile.env_activate.clone(),
        paths: profile.paths.clone(),
    };
    Ok(Plan {
        ask: ask.clone(),
        project: project.clone(),
        account: profile.account.clone(),
        partition: part.name.clone(),
        charge: part.charge,
        max_walltime_s: part.max_walltime_s,
        gpus_per_node: per_node,
        cpus_per_node: cpus,
        mem_gb_per_node: mem,
        billed_per_node: billed_node,
        billed_gpus: billed_node * ask.nodes,
        segments,
        env,
    })
}

// ---------------------------------------------------------------- command overrides

/// An override value before stringification (Python accepted str / int / float / bool / None).
#[derive(Debug, Clone, PartialEq)]
pub enum OverrideValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

/// `^[+~]{0,2}[A-Za-z_][A-Za-z0-9_.\-/]*$` (Python `$`: one trailing `\n` passes).
fn override_key_ok(key: &str) -> bool {
    let b = py_dollar(key).as_bytes();
    let prefix = b.iter().take_while(|c| matches!(c, b'+' | b'~')).count();
    let rest = &b[prefix..];
    prefix <= 2
        && rest
            .first()
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        && rest[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'/'))
}

/// Validate one `k=v` override; bools become `true` / `false`, None `null`, floats
/// Python `str(float)`.
pub fn check_override(key: &str, value: &OverrideValue) -> Result<(String, String)> {
    if !override_key_ok(key) {
        bail!(
            "invalid override key {}: use letters, digits, '_', '.', '-', '/' (hydra prefixes + / ~ allowed)",
            py_repr(key)
        );
    }
    let value = match value {
        OverrideValue::Str(s) => s.clone(),
        OverrideValue::Int(i) => i.to_string(),
        OverrideValue::Float(f) => python_float_repr(*f),
        OverrideValue::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        OverrideValue::Null => "null".to_string(),
    };
    if value.contains(['\n', '\r', '\0']) {
        bail!("override {key}: value must be a single line");
    }
    Ok((key.to_string(), value))
}

/// `k=v` from `--set`.
pub fn parse_override(text: &str) -> Result<(String, String)> {
    let Some((key, value)) = text.split_once('=') else {
        bail!("invalid --set {}; use --set key=value", py_repr(text));
    };
    check_override(py_strip(key), &OverrideValue::Str(value.to_string()))
}

/// `base` with `new` applied: same key replaced in place, new keys appended.
pub fn merge_overrides(
    base: &[(String, String)],
    new: &[(String, String)],
) -> Vec<(String, String)> {
    let mut out = base.to_vec();
    for (k, v) in new {
        match out.iter_mut().find(|(key, _)| key == k) {
            Some(item) => item.1 = v.clone(),
            None => out.push((k.clone(), v.clone())),
        }
    }
    out
}

/// hydra → `k=v`; argparse / tyro → `--k v`.
pub fn render_overrides(style: &str, overrides: &[(String, String)]) -> Result<Vec<String>> {
    match style {
        "hydra" => Ok(overrides.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        "argparse" | "tyro" => {
            let mut out = Vec::with_capacity(overrides.len() * 2);
            for (k, v) in overrides {
                if k.starts_with(['+', '~']) {
                    bail!(
                        "override key {}: + / ~ prefixes are hydra syntax, not {style}",
                        py_repr(k)
                    );
                }
                out.push(format!("--{k}"));
                out.push(v.clone());
            }
            Ok(out)
        }
        _ => bail!("unknown override style {}", py_repr(style)),
    }
}

// ---------------------------------------------------------------- names and paths

/// `arb-<project>-<run>-s<k>`.
pub fn job_name(project: &ProjectId, run_id: &RunId, segment: u32) -> Result<String> {
    let name = format!("arb-{project}-{run_id}-s{segment}");
    if parse_job_name(&name).is_err() {
        bail!(
            "job name {} is invalid or longer than {MAX_JOB_NAME} chars",
            py_repr(&name)
        );
    }
    Ok(name)
}

/// `(project, run part, segment)` of an arboria job name. Python's
/// `^arb-([a-z0-9][a-z0-9-]{0,30})-([a-z0-9]+)-s([1-9][0-9]*)$`: the run part is the
/// last `-`-free piece before `-s<k>` (any `[a-z0-9]+`, not only a [`RunId`]); names
/// over [`MAX_JOB_NAME`] chars are refused.
pub fn parse_job_name(name: &str) -> Result<(ProjectId, String, u32)> {
    let refuse = || Error::Arb(format!("not an arboria job name: {}", py_repr(name)));
    if name.chars().count() > MAX_JOB_NAME {
        return Err(refuse());
    }
    let t = py_dollar(name);
    let body = t.strip_prefix("arb-").ok_or_else(refuse)?;
    // -s<k>: k is the trailing ASCII digits, no leading zero
    let k_start = body.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let k = &body[k_start..];
    let middle = body[..k_start].strip_suffix("-s");
    let (Some(middle), false, false) = (middle, k.is_empty(), k.starts_with('0')) else {
        return Err(refuse());
    };
    // the run part has no '-', so it is what follows the last '-'
    let (project, run) = middle.rsplit_once('-').ok_or_else(refuse)?;
    let run_ok = !run.is_empty()
        && run
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    // ProjectId::parse lets one trailing '\n' through (Python `$`); inside a job name
    // the group is followed by '-', so a newline there never matches
    if !run_ok || project.ends_with('\n') {
        return Err(refuse());
    }
    let project = ProjectId::parse(project).map_err(|_| refuse())?;
    let segment = k.parse().map_err(|_| refuse())?;
    Ok((project, run.to_string(), segment))
}

/// The project of a job name, or None (e.g. a pre-project `arboria-r001-s1`).
pub fn job_project(name: &str) -> Option<ProjectId> {
    parse_job_name(name).ok().map(|(p, _, _)| p)
}

fn role<'a>(paths: &'a BTreeMap<String, String>, role: &str) -> &'a str {
    paths
        .get(role)
        .unwrap_or_else(|| panic!("profile paths lack {role:?} (profiles are validated)"))
}

/// `<runs>/<project>/<run>`.
pub fn run_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    format!("{}/{project}/{run_id}", role(paths, "runs"))
}

/// `<run dir>/code`.
pub fn code_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    format!("{}/code", run_dir(paths, project, run_id))
}

/// `<run dir>/sbatch/s<k>.sh`.
pub fn script_path(
    paths: &BTreeMap<String, String>,
    project: &ProjectId,
    run_id: &RunId,
    segment: u32,
) -> String {
    format!("{}/sbatch/s{segment}.sh", run_dir(paths, project, run_id))
}

/// `<run dir>/logs/s<k>.%j.out` (`$USER` stays a shell variable).
pub fn log_pattern(
    paths: &BTreeMap<String, String>,
    project: &ProjectId,
    run_id: &RunId,
    segment: u32,
) -> String {
    format!("{}/logs/s{segment}.%j.out", run_dir(paths, project, run_id))
}

/// [`log_pattern`] with `%j` = `job_id`.
pub fn log_path(
    paths: &BTreeMap<String, String>,
    project: &ProjectId,
    run_id: &RunId,
    segment: u32,
    job_id: &str,
) -> String {
    log_pattern(paths, project, run_id, segment).replace("%j", job_id)
}

/// `<out>/<project>/<run>` (`ARBORIA_OUT`).
pub fn out_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    format!("{}/{project}/{run_id}", role(paths, "out"))
}

/// `<ckpt>/<project>/<run>` (`ARBORIA_CKPT_DIR`).
pub fn ckpt_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    format!("{}/{project}/{run_id}", role(paths, "ckpt"))
}

/// `<project>-<run>` (`ARBORIA_WANDB_ID`; the run id is not validated, as in Python).
pub fn wandb_id(project: &ProjectId, run_id: &str) -> String {
    format!("{project}-{run_id}")
}

/// `#SBATCH` lines are not shell-expanded: `$USER` / `${USER}` → `%u`.
pub fn sbatch_value(path: &str) -> String {
    // Python: re.sub(r"\$\{USER\}|\$USER(?![A-Za-z0-9_])", "%u", path)
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(r) = tail.strip_prefix("${USER}") {
            out.push_str("%u");
            rest = r;
        } else if let Some(r) = tail
            .strip_prefix("$USER")
            .filter(|r| !r.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
        {
            out.push_str("%u");
            rest = r;
        } else {
            out.push('$');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Double-quote a validated profile path so `$USER` still expands; refuse `"` `\` `` ` `` `$(`.
pub fn dq(path: &str) -> Result<String> {
    if path.contains(['"', '\\', '`']) || path.contains("$(") {
        bail!("unsafe remote path {}", py_repr(path));
    }
    Ok(format!("\"{path}\""))
}

/// Python `shlex.quote`.
pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = s.bytes().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    });
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

/// Python `shlex.join`.
pub fn shlex_join<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter()
        .map(|a| shlex_quote(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The batch script of one segment (dependencies go to sbatch, not into the script).
pub fn render_sbatch(plan: &Plan, run_id: &RunId, idea: &IdeaName, segment: u32) -> Result<String> {
    let paths = &plan.env.paths;
    let n = plan.segments.len();
    if segment < 1 || segment as usize > n {
        bail!("segment {segment} out of range 1..{n}");
    }
    let project = &plan.project;
    let log = sbatch_value(&log_pattern(paths, project, run_id, segment));
    let mut lines = vec![
        "#!/bin/bash".to_string(),
        format!(
            "# arboria project {project}, run {run_id}, segment {segment}/{n}, idea {idea}. \
             Generated by arb; do not edit."
        ),
        format!("#SBATCH --job-name={}", job_name(project, run_id, segment)?),
        format!("#SBATCH --account={}", plan.account),
        format!("#SBATCH --partition={}", plan.partition),
        format!("#SBATCH --nodes={}", plan.ask.nodes),
        "#SBATCH --ntasks-per-node=1".to_string(),
        format!("#SBATCH --gpus-per-node={}", plan.gpus_per_node),
        format!("#SBATCH --cpus-per-task={}", plan.cpus_per_node),
        format!("#SBATCH --mem={}G", plan.mem_gb_per_node),
        format!(
            "#SBATCH --time={}",
            slurm_time(plan.segments[segment as usize - 1])
        ),
        format!("#SBATCH --output={log}"),
        format!("#SBATCH --error={log}"),
        "set -euo pipefail".to_string(),
    ];
    if !plan.env.prelude.is_empty() || !plan.env.env_activate.is_empty() {
        // module / conda activation scripts often read unset variables
        lines.push("set +u".into());
        lines.extend(plan.env.prelude.iter().cloned());
        lines.extend(plan.env.env_activate.iter().cloned());
        lines.push("set -u".into());
    }
    lines.extend([
        format!("export ARBORIA_RUN_ID={}", shlex_quote(run_id.as_str())),
        format!(
            "export ARBORIA_WANDB_ID={}",
            shlex_quote(&wandb_id(project, run_id.as_str()))
        ),
        format!("export ARBORIA_IDEA={}", shlex_quote(idea.as_str())),
        format!("export ARBORIA_SEGMENT={segment}"),
        format!("export ARBORIA_DATA={}", dq(role(paths, "data"))?),
        format!("export ARBORIA_CACHE={}", dq(role(paths, "cache"))?),
        "export HF_HOME=\"$ARBORIA_CACHE/hf\"".to_string(),
        format!(
            "export ARBORIA_CKPT_DIR={}",
            dq(&ckpt_dir(paths, project, run_id))?
        ),
        format!(
            "export ARBORIA_OUT={}",
            dq(&out_dir(paths, project, run_id))?
        ),
        "mkdir -p \"$ARBORIA_CACHE\" \"$HF_HOME\" \"$ARBORIA_CKPT_DIR\" \"$ARBORIA_OUT\""
            .to_string(),
        format!("cd {}", dq(&code_dir(paths, project, run_id))?),
        shlex_join(&plan.ask.command),
    ]);
    let mut text = lines.join("\n");
    text.push('\n');
    Ok(text)
}
