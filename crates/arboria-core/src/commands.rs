//! Notebook commands: small analysis functions the agent writes and the human calls from a
//! notebook (`lab.<name>(...)`, `/name args`). Ports `arboria.commands` (the file side;
//! loading and running a command stays in the human's Python kernel).
//!
//! Invariants:
//! - **The core never executes command code.** Listing and describing read the file
//!   (Python: `ast`); only the human's kernel imports and runs a command.
//! - Two scopes: project `<state dir>/commands/<name>.py`, global
//!   `$ARBORIA_CONFIG_DIR/commands/<name>.py`; the project file wins on a name clash.
//!   Names match `^[a-z][a-z0-9_]{0,40}$` and must not shadow a built-in (`lab.run`, …).
//! - A call to a missing command becomes a todo (`command_new` / `command_revise`, the
//!   call in `payload_json`, written by code); the same call reuses the open todo.
//! - Project commands are versioned by the backup repo (`git -C .arboria log -p
//!   commands/`); `promote` copies one into the global library (`command_promote`
//!   event, backup commit) and refuses to replace a different global file unless `force`.
//!
//! [`describe`] reads the source with Python's own parser: `python -I -c <script>` (the
//! interpreter is `$ARBORIA_PYTHON`, default `python3`) runs `ast.parse` on the file and
//! prints JSON. The file is never imported or executed; isolated mode keeps the cwd and
//! user site-packages out of `sys.path`.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cards::sha256_hex;
use crate::db::Ledger;
use crate::model::{EventKind, Idea, TaskKind};
use crate::records::{py_repr, TextField};
use crate::tasks::{self, TaskView};
use crate::{bail, Error, Result};

/// `commands/` under the state dir or the config dir.
pub const COMMANDS_DIR: &str = "commands";
/// Keyword arguments `lab` consumes itself (`scope=`, `ask="…"`).
pub const RESERVED_KWARGS: &[&str] = &["scope", "ask"];
/// A long default is clipped to this many characters in signatures.
pub const DEFAULT_CLIP: usize = 24;
/// The interpreter [`describe`] runs (default `python3`).
pub const PYTHON_ENV: &str = "ARBORIA_PYTHON";
/// How long [`describe`] waits for the parser.
pub const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(20);

/// The functions `lab` itself offers, in `lab.commands()` order, with their one line.
pub const BUILTINS: &[(&str, &str)] = &[
    ("runs", "这个 idea 的 run 一览"),
    ("get", "按 id 取一个 run"),
    ("compare", "几个 run 的 wandb 曲线（长表）"),
    ("plot", "几个 run 的一条 wandb 曲线画在一起"),
    ("diff", "两个 run 哪里不同：命令、资源、代码、config"),
    ("spend", "各 idea 用了多少 SU 和 GPU 时；给 idea 就逐个 run"),
    ("run", "生成快照和审核单（不批准）"),
    ("fork", "从这个 idea 分出新 idea"),
    (
        "inspect",
        "请 agent 讲一段代码：代码摘录加批注、伪代码、简化代码或架构图",
    ),
    ("todo", "给 agent 留一条待办"),
    ("todos", "待办列表"),
    ("conf", "改置信度"),
    ("metric", "改指标"),
    ("prune", "放弃这个 idea"),
];

/// Names in `arboria.lab`'s namespace that are valid command names (Python
/// `reserved_names()`: a command may not take them). `tests/commands.rs` compares this
/// with the Python package.
pub const LAB_NAMES: &[&str] = &[
    "analysis",
    "annotations",
    "commands",
    "compare",
    "conf",
    "dataclass",
    "diff",
    "field",
    "fork",
    "get",
    "ideas",
    "inspect",
    "job_state_label",
    "json",
    "locate",
    "metric",
    "notebooks",
    "os",
    "plot",
    "project_config",
    "prune",
    "records",
    "resolve",
    "run",
    "run_rows",
    "runs",
    "seed_of_notebook",
    "sessions",
    "slurm",
    "spend",
    "status_label",
    "submission",
    "sys",
    "tasks",
    "todo",
    "todos",
    "use",
];

/// Where a command file lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// This project (`项目`).
    Project,
    /// The global library (`全局`).
    Global,
}

impl Scope {
    /// `project` / `global`.
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Global => "global",
        }
    }

    /// `项目` / `全局` (`SCOPE_LABELS`).
    pub fn label(self) -> &'static str {
        match self {
            Scope::Project => "项目",
            Scope::Global => "全局",
        }
    }

    /// Parse `project` / `global` (`validate_scope`).
    pub fn parse(s: &str) -> Result<Scope> {
        match s {
            "project" => Ok(Scope::Project),
            "global" => Ok(Scope::Global),
            _ => bail!(
                "invalid command scope {}; choose one of: project, global",
                py_repr(s)
            ),
        }
    }
}

/// `^[a-z][a-z0-9_]{0,40}$`, whole string.
fn name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 41
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// Validate a command name (`check_reserved`: also refuse names `lab` already has).
pub fn validate_name(name: &str, check_reserved: bool) -> Result<()> {
    if !name_ok(name) {
        bail!(
            "invalid command name {}: lowercase letters, digits and '_', starting with a letter, at most 41 chars",
            py_repr(name)
        );
    }
    if check_reserved && LAB_NAMES.contains(&name) {
        bail!(
            "{} is already part of lab (lab.{name}); pick another command name",
            py_repr(name)
        );
    }
    Ok(())
}

/// `$ARBORIA_CONFIG_DIR/commands` (default `~/.config/arboria/commands`).
pub fn global_dir() -> PathBuf {
    crate::clusters::config_dir().join(COMMANDS_DIR)
}

/// `<state dir>/commands`.
pub fn project_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(COMMANDS_DIR)
}

/// The folder of a scope (project commands need a repo).
pub fn scope_dir(scope: Scope, state_dir: Option<&Path>) -> Result<PathBuf> {
    match (scope, state_dir) {
        (Scope::Global, _) => Ok(global_dir()),
        (Scope::Project, Some(s)) => Ok(project_dir(s)),
        (Scope::Project, None) => {
            bail!("project commands need an arboria repo (run `arb init`)")
        }
    }
}

/// A command file on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandFile {
    pub name: String,
    pub scope: Scope,
    pub path: PathBuf,
    /// sha256 hex of the bytes; the version shown is its first 8 characters.
    pub sha256: String,
}

impl CommandFile {
    /// The first 8 hex digits of the hash.
    pub fn version(&self) -> &str {
        &self.sha256[..8]
    }
}

fn file(name: &str, scope: Scope, state_dir: Option<&Path>) -> Option<CommandFile> {
    let path = scope_dir(scope, state_dir).ok()?.join(format!("{name}.py"));
    if !path.is_file() {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    Some(CommandFile {
        name: name.to_string(),
        scope,
        path,
        sha256: sha256_hex(&bytes),
    })
}

/// The command `name` as the notebook sees it: project first, then global.
pub fn find(name: &str, state_dir: Option<&Path>) -> Result<Option<CommandFile>> {
    validate_name(name, false)?;
    Ok(file(name, Scope::Project, state_dir).or_else(|| file(name, Scope::Global, state_dir)))
}

/// One parameter after `ctx`, read from the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    /// positional | vararg | keyword | varkw
    pub kind: String,
    /// Source text of the default.
    pub default: Option<String>,
}

/// What the source says about a command, without running it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Described {
    pub description: Option<String>,
    pub problem: Option<String>,
    /// None when there is no matching `def`.
    pub params: Option<Vec<Param>>,
}

impl Described {
    /// The docstring's first line.
    pub fn summary(&self) -> String {
        self.description
            .as_deref()
            .map(crate::records::py_strip)
            .and_then(|d| {
                crate::records::py_splitlines(d)
                    .first()
                    .map(|l| l.to_string())
            })
            .unwrap_or_default()
    }

    /// `name(…)` without a def, else [`format_signature`].
    pub fn signature(&self, name: &str, limit: Option<usize>) -> String {
        match &self.params {
            None => format!("{name}(…)"),
            Some(p) => format_signature(name, p, limit),
        }
    }
}

/// Python `commands.describe` with `ast`, run by the interpreter (prints the JSON).
const DESCRIBE_SCRIPT: &str = r#"
import ast, json, sys
path, name = sys.argv[1], sys.argv[2]
RESERVED = ("scope", "ask")
def params(fn):
    a = fn.args
    positional = [*a.posonlyargs, *a.args]
    defaults = [None] * (len(positional) - len(a.defaults)) + list(a.defaults)
    out = [(arg.arg, "positional", None if d is None else ast.unparse(d)) for arg, d in zip(positional, defaults)]
    if a.vararg is not None:
        out.append((a.vararg.arg, "vararg", None))
    out += [(arg.arg, "keyword", None if d is None else ast.unparse(d)) for arg, d in zip(a.kwonlyargs, a.kw_defaults)]
    if a.kwarg is not None:
        out.append((a.kwarg.arg, "varkw", None))
    return out[1:] if out and out[0][1] == "positional" else out
def describe():
    try:
        with open(path, encoding="utf-8") as fh:
            tree = ast.parse(fh.read(), filename=path)
    except (OSError, UnicodeDecodeError, SyntaxError) as exc:
        return None, f"cannot parse: {exc}", None
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name:
            doc = ast.get_docstring(node)
            if isinstance(node, ast.AsyncFunctionDef):
                return doc, f"{name} must be a plain def, not async def", None
            ps = params(node)
            problem = None if doc else "no docstring"
            if not (node.args.posonlyargs or node.args.args):
                problem = f"{name} must take ctx as its first argument"
            clash = [p[0] for p in ps if p[0] in RESERVED]
            if clash:
                problem = f"parameter {clash[0]!r} is lab's own (it never reaches the command); rename it"
            return doc, problem, ps
    return ast.get_docstring(tree), f"no top-level `def {name}(ctx, ...)`", None
doc, problem, ps = describe()
out = {"description": doc, "problem": problem,
       "params": None if ps is None else [{"name": n, "kind": k, "default": d} for n, k, d in ps]}
sys.stdout.write(json.dumps(out))
"#;

fn python() -> String {
    std::env::var(PYTHON_ENV)
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "python3".into())
}

/// Read a command's signature and docstring from its source with Python's `ast` (see the
/// module notes); never imports it. A file Python cannot parse is a `problem`, not an
/// error; a missing interpreter is an error.
pub fn describe(path: &Path, name: &str) -> Result<Described> {
    let exe = python();
    let mut child = Command::new(&exe)
        .args(["-I", "-c", DESCRIBE_SCRIPT])
        .arg(path)
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            Error::Arb(format!(
                "cannot run {exe} to read {} ({e}); set {PYTHON_ENV}",
                path.display()
            ))
        })?;
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > DESCRIBE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{exe} took more than {}s to read {}",
                DESCRIBE_TIMEOUT.as_secs(),
                path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut out = String::new();
    child
        .stdout
        .take()
        .expect("piped")
        .read_to_string(&mut out)?;
    if !status.success() {
        let mut err = String::new();
        child
            .stderr
            .take()
            .expect("piped")
            .read_to_string(&mut err)?;
        let last = crate::records::py_splitlines(crate::records::py_strip(&err))
            .last()
            .map(|l| l.to_string())
            .unwrap_or_default();
        bail!("{exe} could not read {}: {last}", path.display());
    }
    Ok(serde_json::from_str(&out)?)
}

fn clip_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

/// `svd(run, *, k=20, log=False)`: no annotations, defaults as written. With `limit`,
/// trailing parameters that do not fit become `…`.
pub fn format_signature(name: &str, params: &[Param], limit: Option<usize>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut star = false;
    for p in params {
        match p.kind.as_str() {
            "vararg" => {
                parts.push(format!("*{}", p.name));
                star = true;
                continue;
            }
            "varkw" => {
                parts.push(format!("**{}", p.name));
                continue;
            }
            "keyword" if !star => {
                parts.push("*".into());
                star = true;
            }
            _ => {}
        }
        match &p.default {
            None => parts.push(p.name.clone()),
            Some(d) => parts.push(format!("{}={}", p.name, clip_chars(d, DEFAULT_CLIP))),
        }
    }
    let len = |s: &str| s.chars().count();
    let full = format!("{name}({})", parts.join(", "));
    match limit {
        Some(l) if len(&full) > l => {
            let mut kept: Vec<String> = Vec::new();
            for part in &parts {
                let mut trial = kept.clone();
                trial.push(part.clone());
                trial.push("…".into());
                if len(&format!("{name}({})", trial.join(", "))) > l {
                    break;
                }
                kept.push(part.clone());
            }
            kept.push("…".into());
            format!("{name}({})", kept.join(", "))
        }
        _ => full,
    }
}

/// An `arb commands list` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandRow {
    pub name: String,
    pub scope: Scope,
    pub version: String,
    pub path: String,
    pub signature: String,
    pub description: String,
    pub problem: Option<String>,
    /// The one `lab.<name>` uses (a project file shadows a global one).
    pub active: bool,
}

/// `arb commands list`: every command file of both scopes, by name (project first).
pub fn discover(state_dir: Option<&Path>) -> Result<Vec<CommandRow>> {
    let mut rows = Vec::new();
    for scope in [Scope::Project, Scope::Global] {
        if scope == Scope::Project && state_dir.is_none() {
            continue;
        }
        let folder = scope_dir(scope, state_dir)?;
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "py"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(name) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            if !name_ok(&name) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let d = describe(&path, &name)?;
            let problem = if LAB_NAMES.contains(&name.as_str()) {
                Some(format!("shadowed by lab.{name}; rename it"))
            } else {
                d.problem.clone()
            };
            rows.push(CommandRow {
                version: sha256_hex(&bytes)[..8].to_string(),
                path: path.display().to_string(),
                signature: d.signature(&name, None),
                description: d.summary(),
                problem,
                active: true,
                name,
                scope,
            });
        }
    }
    let project: Vec<String> = rows
        .iter()
        .filter(|r| r.scope == Scope::Project)
        .map(|r| r.name.clone())
        .collect();
    for r in &mut rows {
        r.active = r.scope == Scope::Project || !project.contains(&r.name);
    }
    rows.sort_by(|a, b| {
        (&a.name, a.scope != Scope::Project).cmp(&(&b.name, b.scope != Scope::Project))
    });
    Ok(rows)
}

/// What [`promote`] did (Python's dict).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Promoted {
    pub name: String,
    pub version: String,
    pub path: String,
    /// False: the global library already had these bytes.
    pub changed: bool,
    /// The version it replaced (`--force`).
    pub replaced: Option<String>,
}

/// `arb commands promote`: copy a project command into the global library. Refuses to
/// replace a different global file unless `force`; the project file stays (and still
/// wins there).
pub fn promote(ledger: &mut Ledger, name: &str, force: bool) -> Result<Promoted> {
    validate_name(name, true)?;
    let state = ledger.state_dir().to_path_buf();
    let Some(src) = file(name, Scope::Project, Some(&state)) else {
        bail!(
            "no project command {} ({}/{name}.py)",
            py_repr(name),
            project_dir(&state).display()
        );
    };
    let dest = global_dir().join(format!("{name}.py"));
    let old = file(name, Scope::Global, None);
    if old.as_ref().is_some_and(|o| o.sha256 == src.sha256) {
        return Ok(Promoted {
            name: name.into(),
            version: src.version().into(),
            path: dest.display().to_string(),
            changed: false,
            replaced: None,
        });
    }
    if let (Some(o), false) = (&old, force) {
        bail!(
            "the global library already has {name} (v{}, {}); pass --force to replace it",
            o.version(),
            dest.display()
        );
    }
    std::fs::create_dir_all(dest.parent().expect("has a parent"))?;
    let data = std::fs::read(&src.path)?;
    let version = sha256_hex(&data)[..8].to_string();
    let tmp = dest.with_file_name(format!(".{name}.py.tmp"));
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, &dest)?;
    let replaced = old.map(|o| o.version().to_string());
    ledger.write(|tx| {
        tx.event(
            EventKind::CommandPromote,
            name,
            &json!({"version": version, "from": src.path.display().to_string(),
                    "to": dest.display().to_string(), "replaced": replaced}),
        )
    })?;
    Ok(Promoted {
        name: name.into(),
        version,
        path: dest.display().to_string(),
        changed: true,
        replaced,
    })
}

/// `lab.<name>(a, k=v)` from the arguments' (safe) reprs.
pub fn call_text(name: &str, args: &[String], kwargs: &[(String, String)]) -> String {
    let parts: Vec<String> = args
        .iter()
        .cloned()
        .chain(kwargs.iter().map(|(k, v)| format!("{k}={v}")))
        .collect();
    format!("lab.{name}({})", parts.join(", "))
}

/// The todo text of a command request (Python `todo_text`).
pub fn todo_text(
    kind: TaskKind,
    name: &str,
    scope: Scope,
    description: Option<&str>,
    call: &str,
) -> String {
    let limit = TextField::Todo.limit();
    let head = if kind == TaskKind::CommandNew {
        format!(
            "写 notebook 命令 {name}（{}）：{}",
            scope.label(),
            description.unwrap_or("（没有说明，看调用）")
        )
    } else {
        format!(
            "修改 notebook 命令 {name}（{}）：{}",
            scope.label(),
            description.unwrap_or("None")
        )
    };
    let tail = format!("\n调用：{call}\n约定见 `arb commands guide`");
    let tail_len = tail.chars().count();
    clip_chars(&head, limit - tail_len.min(limit / 2)) + &clip_chars(&tail, limit / 2)
}

/// A call to a missing (or to-be-revised) command, as the notebook made it.
#[derive(Debug, Clone, Default)]
pub struct Call<'a> {
    /// Safe reprs of the positional arguments.
    pub args: &'a [String],
    /// Keyword arguments with safe reprs of their values, in call order.
    pub kwargs: &'a [(String, String)],
    /// Where the file should go (default: `<scope dir>/<name>.py`).
    pub path: Option<&'a str>,
}

/// Record (or reuse) the todo for a missing or to-be-revised command call; returns the
/// todo and whether it is new. An open `command_new` todo for the same name is reused;
/// a revise todo only for the same instruction. Seeds have no repo: global only.
pub fn request(
    ledger: &mut Ledger,
    idea: &Idea,
    kind: TaskKind,
    name: &str,
    scope: Scope,
    description: Option<&str>,
    call: Call<'_>,
) -> Result<(TaskView, bool)> {
    validate_name(name, true)?;
    let seed = tasks::is_seed(idea);
    if seed && scope != Scope::Global {
        bail!("a seed has no repo: only global commands (scope=\"global\")");
    }
    if !matches!(kind, TaskKind::CommandNew | TaskKind::CommandRevise) {
        bail!("not a command todo kind: {}", py_repr(kind.as_str()));
    }
    let description = description
        .map(crate::records::py_strip)
        .filter(|d| !d.is_empty())
        .map(|d| clip_chars(d, TextField::Todo.limit()));
    if kind == TaskKind::CommandRevise && description.is_none() {
        bail!("say what to change, e.g. lab.svd.revise(\"改成 log 坐标\")");
    }
    for task in tasks::open_command_tasks(ledger, name, Some(kind))? {
        let same = tasks::payload(&task.task)?
            .get("description")
            .and_then(Value::as_str)
            .map(String::from)
            == description;
        if kind == TaskKind::CommandNew || same {
            return Ok((task, false));
        }
    }
    let text_call = call_text(name, call.args, call.kwargs);
    let path = match call.path {
        Some(p) => p.to_string(),
        None => scope_dir(scope, Some(ledger.state_dir()))?
            .join(format!("{name}.py"))
            .display()
            .to_string(),
    };
    let mut kwargs = serde_json::Map::new();
    for (k, v) in call.kwargs {
        kwargs.insert(k.clone(), v.clone().into());
    }
    let mut payload = serde_json::Map::new();
    payload.insert("name".into(), name.into());
    payload.insert("scope".into(), scope.as_str().into());
    payload.insert("args".into(), call.args.to_vec().into());
    payload.insert("kwargs".into(), Value::Object(kwargs));
    payload.insert("description".into(), description.clone().into());
    payload.insert(
        (if seed { "seed" } else { "idea" }).into(),
        idea.name.as_str().into(),
    );
    payload.insert("call".into(), text_call.clone().into());
    payload.insert("path".into(), path.into());
    let text = todo_text(kind, name, scope, description.as_deref(), &text_call);
    let task = tasks::add(
        ledger,
        idea,
        &text,
        kind,
        Some(&Value::Object(payload)),
        None,
    )?;
    Ok((task, true))
}

/// The guide's template (`{project}`, `{global_}`; `{{` / `}}` are literal braces).
const GUIDE: &str = include_str!("commands_guide.md");

/// `arb commands guide`: the conventions for writing a command (for the agent).
pub fn guide() -> String {
    GUIDE
        .replace("{project}", ".arboria/commands")
        .replace("{global_}", &global_dir().display().to_string())
        .replace("{{", "{")
        .replace("}}", "}")
}
