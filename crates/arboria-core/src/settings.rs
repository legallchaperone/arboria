//! The app's settings pages (ADR 0004): the machines of `clusters.toml` and a project's
//! `arboria.toml` as forms, so nobody has to edit TOML by hand. New in the Rust core.
//!
//! Invariants:
//! - A form is read leniently (a file `arb` would refuse still opens, so it can be fixed)
//!   and saved strictly: the new text is checked exactly as `arb` reads it
//!   ([`crate::clusters::check_text`], [`crate::config::parse_project_config`]) and nothing
//!   is written when it fails.
//! - Saving edits the file in place with `toml_edit`: comments, key order and the keys a
//!   form does not show (`[project] id`, `[review]`, `[claudemd]`…) are kept. The previous
//!   file is kept as `<name>.bak`; the write is atomic.
//! - Nothing here approves, submits or runs anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::clusters::{self, PATH_ROLES};
use crate::config::{self, OVERRIDE_STYLES};
use crate::{bail, Error, Result};

/// One `[<machine>.partitions.<name>]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartitionForm {
    pub name: String,
    /// `48h`, `2:00:00`…
    pub max_walltime: String,
    pub charge: f64,
}

/// One machine of clusters.toml.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineForm {
    pub name: String,
    /// `slurm` / `fake`.
    pub kind: String,
    pub host: String,
    pub account: String,
    pub arch: String,
    pub gpus_per_node: i64,
    pub cpus_per_gpu: i64,
    pub mem_per_gpu_gb: f64,
    pub default_partition: String,
    pub max_segments: i64,
    pub billing_per_su: f64,
    pub balance_cmd: String,
    pub prelude: Vec<String>,
    pub env_activate: Vec<String>,
    pub partitions: Vec<PartitionForm>,
    /// role → remote path ([`PATH_ROLES`]).
    pub paths: BTreeMap<String, String>,
    /// FakeSlurm's local state dir (kind `fake` only).
    pub root: Option<String>,
}

/// A machine as the settings page shows it: the form, and what `arb` says about it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MachineView {
    pub form: MachineForm,
    pub error: Option<String>,
}

/// The machines page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Machines {
    pub path: PathBuf,
    pub machines: Vec<MachineView>,
    /// The file is not TOML: why (the page then offers the text editor).
    pub syntax_error: Option<String>,
}

fn s(v: Option<&toml::Value>) -> String {
    v.and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

fn int(v: Option<&toml::Value>, default: i64) -> i64 {
    match v {
        Some(toml::Value::Integer(i)) => *i,
        Some(toml::Value::Float(f)) => *f as i64,
        _ => default,
    }
}

fn num(v: Option<&toml::Value>, default: f64) -> f64 {
    v.and_then(config::toml_number).unwrap_or(default)
}

fn strings(v: Option<&toml::Value>) -> Vec<String> {
    match v {
        Some(toml::Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        Some(toml::Value::String(x)) if !x.is_empty() => vec![x.clone()],
        _ => Vec::new(),
    }
}

/// A machine table read leniently (missing or wrong values become defaults).
pub fn machine_form(name: &str, raw: &toml::Value) -> MachineForm {
    let empty = toml::Table::new();
    let t = raw.as_table().unwrap_or(&empty);
    let partitions = t
        .get("partitions")
        .and_then(|p| p.as_table())
        .map(|p| {
            p.iter()
                .map(|(n, v)| PartitionForm {
                    name: n.clone(),
                    max_walltime: s(v.get("max_walltime")),
                    charge: num(v.get("charge"), 1.0),
                })
                .collect()
        })
        .unwrap_or_default();
    let paths_raw = t.get("paths").and_then(|p| p.as_table());
    let paths = PATH_ROLES
        .iter()
        .map(|r| (r.to_string(), s(paths_raw.and_then(|p| p.get(*r)))))
        .collect();
    let kind = t.get("kind").and_then(|v| v.as_str()).unwrap_or("slurm").to_string();
    MachineForm {
        name: name.to_string(),
        host: s(t.get("host")),
        account: s(t.get("account")),
        arch: s(t.get("arch")),
        gpus_per_node: int(t.get("gpus_per_node"), 4),
        cpus_per_gpu: int(t.get("cpus_per_gpu"), 1),
        mem_per_gpu_gb: num(t.get("mem_per_gpu_gb"), 0.0),
        default_partition: s(t.get("default_partition")),
        max_segments: int(t.get("max_segments"), 4),
        billing_per_su: num(t.get("billing_per_su"), 0.0),
        balance_cmd: s(t.get("balance_cmd")),
        prelude: strings(t.get("prelude")),
        env_activate: strings(t.get("env_activate")),
        partitions,
        paths,
        root: t.get("root").and_then(|v| v.as_str()).map(String::from),
        kind,
    }
}

/// A new machine's starting form (the page's 添加机器).
pub fn blank_machine(name: &str) -> MachineForm {
    let mut f = machine_form(name, &toml::Value::Table(toml::Table::new()));
    f.partitions = vec![PartitionForm {
        name: "gpu".into(),
        max_walltime: "24h".into(),
        charge: 1.0,
    }];
    f.default_partition = "gpu".into();
    f.max_segments = 4;
    f
}

/// Every machine of clusters.toml, read leniently, with `arb`'s verdict on each.
pub fn machines() -> Result<Machines> {
    let path = clusters::config_path();
    let text = if path.is_file() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let checked = clusters::check_text(&text);
    let table: toml::Table = text.parse().unwrap_or_default();
    let machines = table
        .iter()
        .filter(|(_, v)| v.is_table())
        .map(|(name, raw)| MachineView {
            form: machine_form(name, raw),
            error: checked
                .profiles
                .iter()
                .find(|p| &p.name == name)
                .and_then(|p| p.error.clone()),
        })
        .collect();
    Ok(Machines {
        path,
        machines,
        syntax_error: checked.syntax_error,
    })
}

fn whole(x: f64) -> Item {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        value(x as i64)
    } else {
        value(x)
    }
}

fn str_array(xs: &[String]) -> Item {
    let mut a = Array::new();
    for x in xs {
        a.push(x.as_str());
    }
    value(a)
}

/// Set `key` in `t`, keeping the item's decor (comments) when it exists.
fn put(t: &mut Table, key: &str, item: Item) {
    match t.get_mut(key) {
        Some(old) if old.is_value() && item.is_value() => {
            let decor = old.as_value().map(|v| v.decor().clone());
            *old = item;
            if let (Some(d), Some(v)) = (decor, old.as_value_mut()) {
                *v.decor_mut() = d;
            }
        }
        _ => {
            t.insert(key, item);
        }
    }
}

fn write_machine(t: &mut Table, f: &MachineForm) {
    put(t, "kind", value(f.kind.as_str()));
    if f.kind == "fake" {
        put(t, "root", value(f.root.clone().unwrap_or_default()));
        t.remove("host");
    } else {
        t.remove("root");
        put(t, "host", value(f.host.as_str()));
    }
    put(t, "account", value(f.account.as_str()));
    if f.arch.is_empty() {
        t.remove("arch");
    } else {
        put(t, "arch", value(f.arch.as_str()));
    }
    put(t, "gpus_per_node", value(f.gpus_per_node));
    put(t, "cpus_per_gpu", value(f.cpus_per_gpu));
    put(t, "mem_per_gpu_gb", whole(f.mem_per_gpu_gb));
    put(t, "default_partition", value(f.default_partition.as_str()));
    put(t, "max_segments", value(f.max_segments));
    put(t, "billing_per_su", whole(f.billing_per_su));
    if f.balance_cmd.is_empty() {
        t.remove("balance_cmd");
    } else {
        put(t, "balance_cmd", value(f.balance_cmd.as_str()));
    }
    put(t, "prelude", str_array(&f.prelude));
    put(t, "env_activate", str_array(&f.env_activate));
    // partitions: keep the tables (and their comments) of partitions that stay
    let mut parts = t
        .remove("partitions")
        .and_then(|i| i.into_table().ok())
        .unwrap_or_default();
    parts.set_implicit(true);
    let keep: Vec<&str> = f.partitions.iter().map(|p| p.name.as_str()).collect();
    let gone: Vec<String> = parts
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| !keep.contains(&k.as_str()))
        .collect();
    for k in gone {
        parts.remove(&k);
    }
    for p in &f.partitions {
        let entry = parts.entry(&p.name).or_insert(Item::Table(Table::new()));
        if let Some(pt) = entry.as_table_mut() {
            put(pt, "max_walltime", value(p.max_walltime.as_str()));
            if p.charge == 1.0 {
                pt.remove("charge");
            } else {
                put(pt, "charge", whole(p.charge));
            }
        }
    }
    t.insert("partitions", Item::Table(parts));
    let mut paths = t
        .remove("paths")
        .and_then(|i| i.into_table().ok())
        .unwrap_or_default();
    for role in PATH_ROLES {
        put(
            &mut paths,
            role,
            value(f.paths.get(*role).cloned().unwrap_or_default()),
        );
    }
    t.insert("paths", Item::Table(paths));
}

fn doc_of(path: &Path) -> Result<DocumentMut> {
    let text = if path.is_file() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    text.parse::<DocumentMut>().map_err(|e| {
        Error::Arb(format!(
            "{} is not valid TOML ({}); fix it in the text editor first",
            path.display(),
            e.message()
        ))
    })
}

/// Save machine `form` (renamed from `original` when that differs) into clusters.toml,
/// only when `arb` accepts it.
pub fn save_machine(original: Option<&str>, form: &MachineForm) -> Result<Machines> {
    if !crate::clusters::name_ok(&form.name, b"._-") {
        bail!(
            "machine name {} must be letters, digits, '.', '_' or '-'",
            crate::records::py_repr(&form.name)
        );
    }
    let path = clusters::config_path();
    let mut doc = doc_of(&path)?;
    let renamed = original.is_some_and(|o| o != form.name);
    if (original.is_none() || renamed) && doc.contains_key(&form.name) {
        bail!("there is already a machine named {}", form.name);
    }
    let mut table = match original.and_then(|o| doc.remove(o)) {
        Some(Item::Table(t)) => t,
        _ => Table::new(),
    };
    write_machine(&mut table, form);
    doc.insert(&form.name, Item::Table(table));
    let text = doc.to_string();
    let checked = clusters::check_text(&text);
    if let Some(e) = checked
        .profiles
        .iter()
        .find(|p| p.name == form.name)
        .and_then(|p| p.error.clone())
    {
        bail!("{e}; nothing was saved");
    }
    write_with_backup(&path, &text)?;
    machines()
}

/// Remove machine `name` from clusters.toml.
pub fn remove_machine(name: &str) -> Result<Machines> {
    let path = clusters::config_path();
    let mut doc = doc_of(&path)?;
    if doc.remove(name).is_none() {
        bail!("no machine named {name}");
    }
    write_with_backup(&path, &doc.to_string())?;
    machines()
}

fn write_with_backup(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path.is_file() {
        std::fs::copy(path, dir.join(format!("{name}.bak")))?;
    }
    let tmp = dir.join(format!(".{name}.{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------- arboria.toml

/// The parts of a project's arboria.toml the settings page shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectForm {
    pub default_machine: String,
    /// 0 = no budget.
    pub total_su: f64,
    pub wandb_entity: String,
    pub wandb_project: String,
    /// `[run] command` as written (split like a shell line).
    pub run_command: String,
    pub override_style: String,
    pub observation_minutes: i64,
    pub fail_patterns: Vec<String>,
}

/// A project's settings page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectSettings {
    pub root: PathBuf,
    pub path: PathBuf,
    pub project_id: Option<String>,
    pub form: ProjectForm,
    /// What `arb` says about the file now.
    pub error: Option<String>,
    /// The machines it can choose from (clusters.toml).
    pub machines: Vec<String>,
    pub override_styles: Vec<String>,
}

fn section<'a>(t: &'a toml::Table, name: &str) -> Option<&'a toml::Value> {
    t.get(name)
}

/// The project at `root` (its main worktree), read leniently.
pub fn project(root: &Path) -> Result<ProjectSettings> {
    let path = root.join(crate::paths::CONFIG_NAME);
    let text = if path.is_file() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let (t, mut error) = match text.parse::<toml::Table>() {
        Ok(t) => (t, None),
        Err(e) => (toml::Table::new(), Some(format!("not valid TOML: {}", e.message()))),
    };
    if error.is_none() {
        error = config::parse_project_config(&t, &path)
            .err()
            .map(|e| e.to_string());
    }
    let get = |sec: &str, key: &str| section(&t, sec).and_then(|v| v.get(key));
    let defaults = config::parse_project_config(&toml::Table::new(), &path).ok();
    let form = ProjectForm {
        default_machine: s(t.get("default_machine")),
        total_su: num(get("budget", "total_su"), 0.0),
        wandb_entity: s(get("wandb", "entity")),
        wandb_project: s(get("wandb", "project")),
        run_command: s(get("run", "command")),
        override_style: get("run", "override_style")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| defaults.as_ref().map(|d| d.run.override_style.clone()))
            .unwrap_or_else(|| "hydra".into()),
        observation_minutes: int(
            get("watch", "observation_minutes"),
            defaults
                .as_ref()
                .map_or(10, |d| i64::from(d.watch.observation_minutes)),
        ),
        fail_patterns: match get("watch", "fail_patterns") {
            Some(v) => strings(Some(v)),
            None => defaults
                .as_ref()
                .map(|d| d.watch.fail_patterns.clone())
                .unwrap_or_default(),
        },
    };
    let machines = clusters::load_all_raw(None)
        .map(|raw| {
            raw.iter()
                .filter(|(_, v)| v.is_table())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    Ok(ProjectSettings {
        root: root.to_path_buf(),
        project_id: section(&t, "project")
            .and_then(|p| p.get("id"))
            .and_then(|v| v.as_str())
            .map(String::from),
        path,
        form,
        error,
        machines,
        override_styles: OVERRIDE_STYLES.iter().map(|s| s.to_string()).collect(),
    })
}

fn sub<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if !doc.contains_table(name) {
        doc.insert(name, Item::Table(Table::new()));
    }
    doc[name].as_table_mut().expect("a table")
}

/// Save the project's form into its arboria.toml, only when `arb` accepts the result.
pub fn save_project(root: &Path, form: &ProjectForm) -> Result<ProjectSettings> {
    let path = root.join(crate::paths::CONFIG_NAME);
    if !path.is_file() {
        bail!(
            "{} does not exist: run `arb init` in {} first",
            path.display(),
            root.display()
        );
    }
    let mut doc = doc_of(&path)?;
    let top = doc.as_table_mut();
    put(top, "default_machine", value(form.default_machine.as_str()));
    put(sub(&mut doc, "budget"), "total_su", whole(form.total_su));
    let w = sub(&mut doc, "wandb");
    put(w, "entity", value(form.wandb_entity.as_str()));
    put(w, "project", value(form.wandb_project.as_str()));
    let r = sub(&mut doc, "run");
    put(r, "command", value(form.run_command.as_str()));
    put(r, "override_style", value(form.override_style.as_str()));
    let wa = sub(&mut doc, "watch");
    put(wa, "observation_minutes", value(form.observation_minutes));
    put(wa, "fail_patterns", str_array(&form.fail_patterns));
    let text = doc.to_string();
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| Error::Arb(e.message().to_string()))?;
    config::parse_project_config(&table, &path)
        .map_err(|e| Error::Arb(format!("{e}; nothing was saved")))?;
    if !form.default_machine.is_empty() {
        let known = clusters::load_all_raw(None)
            .map(|raw| raw.contains_key(&form.default_machine))
            .unwrap_or(false);
        if !known {
            bail!(
                "default machine {} is not in clusters.toml; add it on the 机器 page first; nothing was saved",
                form.default_machine
            );
        }
    }
    write_with_backup(&path, &text)?;
    project(root)
}
