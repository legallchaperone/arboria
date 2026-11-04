//! `arb install-claude`: merge arboria's SessionStart hook and permission rules into a
//! Claude Code `settings.json`. Ports `arboria.install`.
//!
//! Invariants:
//! - Merging is additive and idempotent: existing user settings are kept; a malformed
//!   section is an error naming the file, never overwritten.
//! - Deny rules are an extra layer, not the guarantee (that is the guarded submit and the
//!   human-only checks in code): cluster commands, approvals, the human's files, seed
//!   writes, notebook restyling, and (v8, ADR 0003) `arb probe enable`.
//! - The global commands dir is machine-specific: in a project it goes into the
//!   uncommitted `.claude/settings.local.json` (excluded via `info/exclude`).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::canonical::python_pretty;
use crate::{bail, Error, Result};

/// The hook command.
pub const HOOK_COMMAND: &str = "arb hook session-start";
/// Approvals happen only in the human's own terminal.
pub const APPROVAL_RULES: &[&str] = &["Bash(arb approve:*)", "Bash(arb lesson accept:*)"];
/// Keep the agent off the cluster directly.
pub const CLUSTER_RULES: &[&str] = &[
    "Bash(sbatch:*)",
    "Bash(srun:*)",
    "Bash(salloc:*)",
    "Bash(scancel:*)",
    "Bash(ssh:*)",
    "Bash(scp:*)",
    "Bash(rsync:*)",
];
/// Files only `arb` or the human write.
pub const PROTECTED_PATHS: &[&str] = &[
    "**/.arboria/notebooks/**",
    "**/.arboria/ledger.db*",
    "**/.arboria/backup/**",
    "**/.arboria/*.toml",
    "**/.arboria/sessions/**",
    "**/notebook.py",
];
/// The tools denied on [`PROTECTED_PATHS`].
pub const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
/// Commands that only ever write seeds.
pub const SEED_RULES: &[&str] = &[
    "Bash(arb seed new:*)",
    "Bash(arb seed set:*)",
    "Bash(arb seed drop:*)",
    "Bash(arb seed revive:*)",
    "Bash(arb seed plant:*)",
    "Bash(arb idea set:*)",
    "Bash(arb idea revive:*)",
    "Bash(arb idea plant:*)",
];
/// `arb idea style` rewrites the human's notebooks.
pub const NOTEBOOK_RULES: &[&str] = &["Bash(arb idea style:*)"];
/// v8 (ADR 0003): enabling a probe is the human's Decision.
pub const PROBE_RULES: &[&str] = &["Bash(arb probe enable:*)"];
/// In the seed home, `arb idea new/drop` write seeds too.
pub const HOME_IDEA_RULES: &[&str] = &["Bash(arb idea new:*)", "Bash(arb idea drop:*)"];
/// The seed home's protected paths.
pub const HOME_PROTECTED_PATHS: &[&str] = &["**/seeds/**", "**/.arboria/**"];
/// `.claude/settings.json`.
pub const SETTINGS_NAME: &str = "settings.json";
/// `.claude/settings.local.json` (per machine, not committed).
pub const LOCAL_NAME: &str = "settings.local.json";

/// What `info/exclude` gets so the local file stays out of git.
pub const LOCAL_EXCLUDE: &str = "/.claude/settings.local.json";
/// Its comment line.
pub const LOCAL_COMMENT: &str = "arboria: Claude Code settings of this machine (install-claude)";

fn file_rules(paths: &'static [&'static str]) -> impl Iterator<Item = String> {
    paths
        .iter()
        .flat_map(|p| EDIT_TOOLS.iter().map(move |t| format!("{t}({p})")))
}

fn strings(rules: &[&[&str]]) -> Vec<String> {
    rules
        .iter()
        .flat_map(|r| r.iter().map(|s| s.to_string()))
        .collect()
}

/// Every deny rule of a project, in Python's order (`DENY_RULES`) plus [`PROBE_RULES`].
pub fn deny_rules() -> Vec<String> {
    let mut out = strings(&[CLUSTER_RULES, APPROVAL_RULES]);
    out.extend(file_rules(PROTECTED_PATHS));
    out.extend(strings(&[SEED_RULES, NOTEBOOK_RULES, PROBE_RULES]));
    out
}

/// The seed home's deny rules (`HOME_DENY_RULES`) plus [`PROBE_RULES`].
pub fn home_deny_rules() -> Vec<String> {
    let mut out = strings(&[
        CLUSTER_RULES,
        APPROVAL_RULES,
        SEED_RULES,
        NOTEBOOK_RULES,
        HOME_IDEA_RULES,
    ]);
    out.extend(file_rules(HOME_PROTECTED_PATHS));
    out.extend(strings(&[PROBE_RULES]));
    out
}

/// Python's `type(v).__name__` of what `json.loads` makes of `v`.
fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `parent.setdefault(key, kind())`, refusing another type.
fn section<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
    list: bool,
    where_: &str,
) -> Result<&'a mut Value> {
    let v = parent.entry(key.to_string()).or_insert_with(|| {
        if list {
            Value::Array(Vec::new())
        } else {
            Value::Object(Map::new())
        }
    });
    let ok = if list { v.is_array() } else { v.is_object() };
    if !ok {
        let want = if list { "list" } else { "dict" };
        bail!("{where_}: `{key}` must be a {want}, found {}", py_type(v));
    }
    Ok(v)
}

fn obj<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<&'a mut Map<String, Value>> {
    Ok(section(parent, key, false, where_)?
        .as_object_mut()
        .expect("checked"))
}

fn list<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<&'a mut Vec<Value>> {
    Ok(section(parent, key, true, where_)?
        .as_array_mut()
        .expect("checked"))
}

fn has_hook(entries: &[Value]) -> bool {
    entries.iter().any(|entry| {
        entry
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| {
                hooks
                    .iter()
                    .any(|h| h.get("command").and_then(Value::as_str) == Some(HOOK_COMMAND))
            })
    })
}

fn root_map<'a>(settings: &'a mut Value, where_: &str) -> Result<&'a mut Map<String, Value>> {
    match settings {
        Value::Object(m) => Ok(m),
        _ => bail!("{where_} must contain a JSON object"),
    }
}

/// Merge the hook, `deny` and (when given) `add_dirs` into `settings`; returns the
/// changes made. Approval rules an older arb put under `ask` are taken out of it.
pub fn merge_settings(
    settings: &mut Value,
    where_: &str,
    add_dirs: Option<&[String]>,
    deny: &[String],
) -> Result<Vec<String>> {
    let mut changes = Vec::new();
    let root = root_map(settings, where_)?;
    let hooks = obj(root, "hooks", where_)?;
    let start = list(hooks, "SessionStart", where_)?;
    if !has_hook(start) {
        start.push(serde_json::json!({"hooks": [{"type": "command", "command": HOOK_COMMAND}]}));
        changes.push(format!("hooks.SessionStart += {HOOK_COMMAND}"));
    }
    let perms = obj(root, "permissions", where_)?;
    let have = list(perms, "deny", where_)?;
    for rule in deny {
        if !have.iter().any(|r| r.as_str() == Some(rule)) {
            have.push(Value::String(rule.clone()));
            changes.push(format!("permissions.deny += {rule}"));
        }
    }
    if let Some(dirs) = add_dirs.filter(|d| !d.is_empty()) {
        changes.extend(add_directories(settings, dirs, where_)?);
    }
    let root = root_map(settings, where_)?;
    let perms = obj(root, "permissions", where_)?;
    if let Some(Value::Array(ask)) = perms.get_mut("ask") {
        for rule in APPROVAL_RULES {
            while let Some(i) = ask.iter().position(|r| r.as_str() == Some(rule)) {
                ask.remove(i);
                changes.push(format!("permissions.ask -= {rule}"));
            }
        }
    }
    Ok(changes)
}

/// Add `add_dirs` to `permissions.additionalDirectories`.
pub fn add_directories(
    settings: &mut Value,
    add_dirs: &[String],
    where_: &str,
) -> Result<Vec<String>> {
    let root = root_map(settings, where_)?;
    let perms = obj(root, "permissions", where_)?;
    let dirs = list(perms, "additionalDirectories", where_)?;
    let mut changes = Vec::new();
    for d in add_dirs {
        if !dirs.iter().any(|x| x.as_str() == Some(d)) {
            dirs.push(Value::String(d.clone()));
            changes.push(format!("permissions.additionalDirectories += {d}"));
        }
    }
    Ok(changes)
}

/// An entry an earlier `install-claude` wrote: our dir, or someone's
/// `.../arboria/commands` (machine-specific, it never belonged in a committed file).
fn is_commands_dir(entry: &Value, ours: &[String]) -> bool {
    let Some(s) = entry.as_str() else {
        return false;
    };
    if ours.iter().any(|o| o == s) {
        return true;
    }
    let parts: Vec<_> = Path::new(s).components().collect();
    let n = parts.len();
    n >= 2 && parts[n - 2].as_os_str() == "arboria" && parts[n - 1].as_os_str() == "commands"
}

/// Take the global commands dir(s) out of a portable settings.json.
pub fn remove_directories(settings: &mut Value, ours: &[String]) -> Vec<String> {
    let Some(perms) = settings
        .get_mut("permissions")
        .and_then(Value::as_object_mut)
    else {
        return Vec::new();
    };
    let Some(Value::Array(dirs)) = perms.get("additionalDirectories") else {
        return Vec::new();
    };
    let gone: Vec<Value> = dirs
        .iter()
        .filter(|d| is_commands_dir(d, ours))
        .cloned()
        .collect();
    if gone.is_empty() {
        return Vec::new();
    }
    let kept: Vec<Value> = dirs.iter().filter(|d| !gone.contains(d)).cloned().collect();
    if kept.is_empty() {
        perms.shift_remove("additionalDirectories");
    } else {
        perms.insert("additionalDirectories".into(), Value::Array(kept));
    }
    gone.iter()
        .map(|d| {
            format!(
                "permissions.additionalDirectories -= {} (machine-specific; now in {LOCAL_NAME})",
                d.as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// The global commands dir (created, so Claude Code can add it as a working dir).
pub fn default_add_dirs() -> Vec<String> {
    let dir = crate::commands::global_dir();
    let dir = if dir.is_absolute() {
        dir
    } else {
        std::env::current_dir().unwrap_or_default().join(dir)
    };
    let _ = std::fs::create_dir_all(&dir);
    vec![dir.display().to_string()]
}

fn load(path: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Object(Map::new())),
        Err(e) => return Err(e.into()),
    };
    let text = if text.is_empty() { "{}" } else { &text };
    let v: Value = serde_json::from_str(text).map_err(|e| {
        Error::Arb(format!(
            "{} is not valid JSON ({e}); fix it first",
            path.display()
        ))
    })?;
    if !v.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    Ok(v)
}

fn save(path: &Path, settings: &Value) -> Result<()> {
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    std::fs::write(path, python_pretty(settings) + "\n")?;
    Ok(())
}

/// Where `arb install-claude` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The current worktree (default).
    Worktree,
    /// `--main`: the main worktree.
    Main,
    /// `--user`: `~/.claude/settings.json`.
    User,
    /// `--home`: the seed home.
    Home,
}

/// Merge into `<root>/.claude/settings.json` (created if missing); returns the settings
/// path and the changes. `add_dirs` None = [`default_add_dirs`]. With `local` (a
/// project) the machine-specific commands dir goes into `settings.local.json`, and an
/// entry an older arb wrote into settings.json moves there.
pub fn install(
    root: &Path,
    add_dirs: Option<&[String]>,
    local: bool,
    deny: &[String],
) -> Result<(PathBuf, Vec<String>)> {
    let add_dirs = match add_dirs {
        Some(d) => d.to_vec(),
        None => default_add_dirs(),
    };
    let path = root.join(".claude").join(SETTINGS_NAME);
    let mut settings = load(&path)?;
    let local_path = path.with_file_name(LOCAL_NAME);
    let mut local_settings = if local {
        load(&local_path)?
    } else {
        Value::Object(Map::new())
    };
    let where_ = path.display().to_string();
    let mut changes = merge_settings(
        &mut settings,
        &where_,
        (!local).then_some(add_dirs.as_slice()),
        deny,
    )?;
    let mut local_changes = Vec::new();
    if local {
        changes.extend(remove_directories(&mut settings, &add_dirs));
        local_changes = add_directories(
            &mut local_settings,
            &add_dirs,
            &local_path.display().to_string(),
        )?;
    }
    if !changes.is_empty() || !path.exists() {
        save(&path, &settings)?;
    }
    if !local_changes.is_empty() {
        save(&local_path, &local_settings)?;
        changes.extend(local_changes.iter().map(|c| format!("{LOCAL_NAME}: {c}")));
    }
    Ok((path, changes))
}
