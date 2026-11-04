//! The project's `arboria.toml`: project id, default machine, budget, watch patterns,
//! wandb, the run command, Claude settings. Ports `arboria.config`.
//!
//! Invariants:
//! - The project id (`[project] id`) names jobs and remote dirs and goes into the card
//!   hash; it matches [`crate::model::ProjectId`] and is generated once by `arb init`
//!   (`<slug ≤26>-<4 hex>`), never changed silently.
//! - `[watch] fail_patterns` are Python `re` syntax today (e.g. `(?i)\bloss\b…`); they
//!   are compiled with the `regex` crate, which takes the defaults below and most Python
//!   patterns. What it cannot compile (look-around, backreferences) is refused with the
//!   crate's message, never skipped silently.
//! - `[claude] push` is still parsed (old files have it) but has no effect: ADR 0001
//!   removes the session-socket push.

use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical::python_float_repr;
use crate::model::{py_dollar, ProjectId};
use crate::paths::RepoContext;
use crate::records::{py_repr, py_strip};
use crate::slurm::shlex_split;
use crate::{bail, Error, Result};

/// `[watch] observation_minutes` default.
pub const DEFAULT_OBSERVATION_MINUTES: u32 = 10;
/// `[watch] fail_patterns` default (Python `re` syntax).
pub const DEFAULT_FAIL_PATTERNS: &[&str] = &[
    r"(?i)\bloss\b[^\n]{0,40}\bnan\b",
    r"(?i)\bnan\b[^\n]{0,40}\bloss\b",
    r"CUDA out of memory",
    r"torch\.OutOfMemoryError",
];
/// `[run] override_style` values.
pub const OVERRIDE_STYLES: &[&str] = &["hydra", "argparse", "tyro"];
/// `[run] command` written by `arb init`.
pub const DEFAULT_RUN_COMMAND: &str = "python train.py";
/// `[run] override_style` default.
pub const DEFAULT_OVERRIDE_STYLE: &str = "hydra";
/// The comment `arb init` writes above `[project] id`.
pub const PROJECT_COMMENT: &str = "# 项目 id：作业名和集群上的目录都带上它，几个仓库共用一个机器配置也不会互相覆盖。由 arb init 生成；已有 run 提交后不要改。";
/// The slug part of a derived project id: + `-` + 4 hex = 31, the longest id allowed.
const SLUG_MAX: usize = 26;

/// `[watch]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchConfig {
    pub observation_minutes: u32,
    /// Validated pattern sources.
    pub fail_patterns: Vec<String>,
}

/// `[wandb]` ("" = not configured).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WandbConfig {
    pub entity: String,
    pub project: String,
}

impl WandbConfig {
    /// Both entity and project set.
    pub fn configured(&self) -> bool {
        !self.entity.is_empty() && !self.project.is_empty()
    }

    /// `https://wandb.ai/<entity>/<project>/runs/<id>` when configured.
    pub fn run_url(&self, wandb_id: &str) -> Option<String> {
        self.configured().then(|| {
            format!(
                "https://wandb.ai/{}/{}/runs/{wandb_id}",
                self.entity, self.project
            )
        })
    }
}

/// `[run]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    /// `shlex.split` of `[run] command`; empty = not configured.
    pub command: Vec<String>,
    pub override_style: String,
}

/// `[claude]` (parsed for old files; `push` is ignored, see module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeConfig {
    pub push: bool,
}

/// The whole `arboria.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub default_machine: String,
    /// `[budget] total_su`; 0 = no budget set.
    pub total_su: f64,
    /// None = the file predates project ids (`arb init` adds one).
    pub project_id: Option<ProjectId>,
    pub watch: WatchConfig,
    pub wandb: WandbConfig,
    pub run: RunConfig,
    pub claude: ClaudeConfig,
}

/// Python's `repr()` of what `tomllib` parses `v` into (for messages: `got 5`,
/// `got 'WaveFront'`, `got True`). Dates use their TOML text.
pub fn py_repr_toml(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => py_repr(s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) if f.is_nan() => "nan".into(),
        toml::Value::Float(f) if f.is_infinite() => if *f > 0.0 { "inf" } else { "-inf" }.into(),
        toml::Value::Float(f) => python_float_repr(*f),
        toml::Value::Boolean(b) => if *b { "True" } else { "False" }.into(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(a) => format!(
            "[{}]",
            a.iter().map(py_repr_toml).collect::<Vec<_>>().join(", ")
        ),
        toml::Value::Table(t) => format!(
            "{{{}}}",
            t.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(k), py_repr_toml(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// A TOML number as Python sees it (int or float, not bool).
pub(crate) fn toml_number(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Integer(i) => Some(*i as f64),
        toml::Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// Parse a TOML file (`what` names it in errors).
pub fn load_toml(path: &Path, what: &str) -> Result<toml::Table> {
    let bytes = std::fs::read(path)?;
    let invalid = |detail: String| {
        Error::Arb(format!(
            "{} is not valid TOML ({detail}); fix the {what} first",
            path.display()
        ))
    };
    let text = String::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?;
    text.parse::<toml::Table>()
        .map_err(|e| invalid(e.message().to_string()))
}

fn project_id_error(where_: &str, got: &str) -> Error {
    Error::Arb(format!(
        "{where_} must match ^[a-z0-9][a-z0-9-]{{0,30}}$ (lowercase letters, digits, '-', at most 31 chars); got {got}"
    ))
}

/// Validate a project id from a file or the command line.
pub fn validate_project_id(value: &toml::Value, where_: &str) -> Result<ProjectId> {
    match value {
        toml::Value::String(s) => validate_project_id_str(s, where_),
        other => Err(project_id_error(where_, &py_repr_toml(other))),
    }
}

/// [`validate_project_id`] of a string (`where_` is `project id` for the command line).
pub fn validate_project_id_str(value: &str, where_: &str) -> Result<ProjectId> {
    ProjectId::parse(value).map_err(|_| project_id_error(where_, &py_repr(value)))
}

/// A new project id from the main worktree's name plus 4 hex chars.
pub fn derive_project_id(ctx: &RepoContext) -> ProjectId {
    let main = ctx.main.canonicalize().unwrap_or_else(|_| ctx.main.clone());
    project_id_for(&main)
}

/// [`derive_project_id`] of an already resolved main worktree path: slug of the folder
/// name (≤26 chars, `proj` when nothing is left) + `-` + 4 hex chars of sha256(path).
pub fn project_id_for(main: &Path) -> ProjectId {
    let name = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")[:26].rstrip("-")
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(SLUG_MAX).collect();
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "proj" } else { slug };
    let digest = Sha256::digest(main.to_string_lossy().as_bytes());
    let hex: String = digest[..2].iter().map(|b| format!("{b:02x}")).collect();
    ProjectId::parse(&format!("{slug}-{hex}")).expect("a slug of [a-z0-9-] is a project id")
}

fn table<'a>(cfg: &'a toml::Table, name: &str, path: &Path) -> Result<Option<&'a toml::Table>> {
    match cfg.get(name) {
        None => Ok(None),
        Some(toml::Value::Table(t)) => Ok(Some(t)),
        Some(_) => bail!("{}: [{name}] must be a table", path.display()),
    }
}

fn project_id(cfg: &toml::Table, path: &Path) -> Result<Option<ProjectId>> {
    match table(cfg, "project", path)?.and_then(|t| t.get("id")) {
        None => Ok(None),
        Some(id) => validate_project_id(id, &format!("{}: [project] id", path.display())).map(Some),
    }
}

fn watch(cfg: &toml::Table, path: &Path) -> Result<WatchConfig> {
    let p = path.display();
    let t = table(cfg, "watch", path)?;
    let get = |k: &str| t.and_then(|t| t.get(k));
    let observation_minutes = match get("observation_minutes") {
        None => DEFAULT_OBSERVATION_MINUTES,
        Some(toml::Value::Integer(m)) if *m >= 1 => u32::try_from(*m)
            .map_err(|_| Error::Arb(format!("{p}: [watch] observation_minutes is too large")))?,
        Some(_) => bail!("{p}: [watch] observation_minutes must be an integer >= 1"),
    };
    let fail_patterns = match get("fail_patterns") {
        None => DEFAULT_FAIL_PATTERNS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Some(toml::Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let pattern = match item {
                    toml::Value::String(s) if !s.is_empty() => s,
                    _ => bail!("{p}: [watch] fail_patterns[{i}] must be a non-empty string"),
                };
                compile_fail_pattern(pattern).map_err(|e| {
                    Error::Arb(format!(
                        "{p}: [watch] fail_patterns[{i}] {} is not a valid regular expression: {e}",
                        py_repr(pattern)
                    ))
                })?;
                out.push(pattern.clone());
            }
            out
        }
        Some(_) => bail!("{p}: [watch] fail_patterns must be a list of regular expressions"),
    };
    Ok(WatchConfig {
        observation_minutes,
        fail_patterns,
    })
}

/// Compile one `[watch] fail_patterns` entry (the `regex` crate; see module docs).
pub fn compile_fail_pattern(pattern: &str) -> std::result::Result<Regex, regex::Error> {
    Regex::new(pattern)
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$` (Python `$`).
fn wandb_name_ok(s: &str) -> bool {
    let b = py_dollar(s).as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn wandb(cfg: &toml::Table, path: &Path) -> Result<WandbConfig> {
    let t = table(cfg, "wandb", path)?;
    let mut vals = [String::new(), String::new()];
    for (slot, key) in vals.iter_mut().zip(["entity", "project"]) {
        let v = t.and_then(|t| t.get(key));
        let ok = match v {
            None => Some(""),
            Some(toml::Value::String(s)) => {
                let s = py_strip(s);
                (s.is_empty() || wandb_name_ok(s)).then_some(s)
            }
            Some(_) => None,
        };
        let Some(s) = ok else {
            let got = v.map_or_else(|| "''".into(), py_repr_toml);
            bail!(
                "{}: [wandb] {key} must be a wandb name (letters, digits, '.', '_', '-'); got {got}",
                path.display()
            );
        };
        *slot = s.to_string();
    }
    let [entity, project] = vals;
    Ok(WandbConfig { entity, project })
}

fn run(cfg: &toml::Table, path: &Path) -> Result<RunConfig> {
    let p = path.display();
    let t = table(cfg, "run", path)?;
    let get = |k: &str| t.and_then(|t| t.get(k));
    let raw = match get("command") {
        None => "",
        Some(toml::Value::String(s)) => s.as_str(),
        Some(_) => bail!("{p}: [run] command must be a string, e.g. \"python train.py\""),
    };
    let command = shlex_split(raw).map_err(|e| {
        Error::Arb(format!(
            "{p}: [run] command {} cannot be split into arguments: {e}",
            py_repr(raw)
        ))
    })?;
    let override_style = match get("override_style") {
        None => DEFAULT_OVERRIDE_STYLE.to_string(),
        Some(toml::Value::String(s)) if OVERRIDE_STYLES.contains(&s.as_str()) => s.clone(),
        Some(other) => bail!(
            "{p}: [run] override_style must be one of: {} (hydra → k=v, argparse/tyro → --k v); got {}",
            OVERRIDE_STYLES.join(", "),
            py_repr_toml(other)
        ),
    };
    Ok(RunConfig {
        command,
        override_style,
    })
}

fn claude(cfg: &toml::Table, path: &Path) -> Result<ClaudeConfig> {
    match table(cfg, "claude", path)?.and_then(|t| t.get("push")) {
        None => Ok(ClaudeConfig { push: true }),
        Some(toml::Value::Boolean(push)) => Ok(ClaudeConfig { push: *push }),
        Some(_) => bail!("{}: [claude] push must be true or false", path.display()),
    }
}

/// Read `arboria.toml` (defaults when absent).
pub fn project_config(ctx: &RepoContext) -> Result<ProjectConfig> {
    let path = ctx.config_path();
    let cfg = if path.is_file() {
        load_toml(&path, "project config")?
    } else {
        toml::Table::new()
    };
    parse_project_config(&cfg, &path)
}

/// Validate a parsed `arboria.toml` (`path` names it in messages).
pub fn parse_project_config(cfg: &toml::Table, path: &Path) -> Result<ProjectConfig> {
    let p = path.display();
    let default_machine = match cfg.get("default_machine") {
        None => String::new(),
        Some(toml::Value::String(s)) => py_strip(s).to_string(),
        Some(_) => bail!("{p}: default_machine must be a string"),
    };
    let total_su = match table(cfg, "budget", path)?.and_then(|t| t.get("total_su")) {
        None => 0.0,
        Some(v) => match toml_number(v) {
            Some(n) if n >= 0.0 => n,
            _ => bail!("{p}: [budget] total_su must be a number >= 0"),
        },
    };
    Ok(ProjectConfig {
        default_machine,
        total_su,
        project_id: project_id(cfg, path)?,
        watch: watch(cfg, path)?,
        wandb: wandb(cfg, path)?,
        run: run(cfg, path)?,
        claude: claude(cfg, path)?,
    })
}

/// The project id, refusing a file without one (`run arb init`).
pub fn require_project_id(ctx: &RepoContext) -> Result<ProjectId> {
    match project_config(ctx)?.project_id {
        Some(id) => Ok(id),
        None => bail!(
            "{} has no [project] id (it names this repo's jobs and remote dirs so repos sharing \
             a machine cannot collide); run `arb init` to add it (the rest of the file is left as is)",
            ctx.config_path().display()
        ),
    }
}

/// The `[project]` table text for a new id.
pub fn project_table(project_id: &ProjectId) -> String {
    format!("[project]\n{PROJECT_COMMENT}\nid = \"{project_id}\"\n")
}

/// Python's `^[ \t]*\[[ \t]*project[ \t]*\][ \t]*(?:#[^\n]*)?$` on one line (no `\n`).
fn is_project_header(line: &str) -> bool {
    let blanks = |s: &'_ str| s.trim_start_matches([' ', '\t']).to_string();
    let rest = blanks(line);
    let Some(rest) = rest.strip_prefix('[').map(blanks) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("project").map(blanks) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(']').map(blanks) else {
        return false;
    };
    rest.is_empty() || rest.starts_with('#')
}

/// `text` with a `[project] id` added (comments and layout kept).
pub fn add_project_id(text: &str, project_id: &ProjectId) -> Result<String> {
    // byte offsets of the end of each header line (before its '\n')
    let mut headers = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        if is_project_header(line) {
            headers.push(start + line.len());
        }
        start += line.len() + 1;
    }
    let new = match headers.as_slice() {
        [] => {
            let sep = if text.is_empty() || text.ends_with("\n\n") {
                ""
            } else if text.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            format!("{text}{sep}{}", project_table(project_id))
        }
        [end] => format!(
            "{}\n{PROJECT_COMMENT}\nid = \"{project_id}\"{}",
            &text[..*end],
            &text[*end..]
        ),
        // invalid TOML anyway; the check below reports it
        _ => text.to_string(),
    };
    let added = new
        .parse::<toml::Table>()
        .ok()
        .and_then(|t| match t.get("project") {
            Some(toml::Value::Table(p)) => p.get("id").cloned(),
            _ => None,
        });
    if added.as_ref().and_then(toml::Value::as_str) != Some(project_id.as_str()) {
        bail!(
            "could not add [project] id to arboria.toml automatically; add\n{}by hand",
            project_table(project_id)
        );
    }
    Ok(new)
}
