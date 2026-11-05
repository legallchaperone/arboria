//! The background agent (ADR 0005): `claude -p` works through the agent queue
//! ([`crate::tasks`]: open → queued → running → done / check_failed / needs_human).
//!
//! Invariants:
//! - Only the human's own logged-in `claude` runs (`$ARBORIA_CLAUDE`, the settings'
//!   `claude`, else `claude` on PATH); the core never sees a token.
//! - One todo per process, one process per project at a time (`.arboria/agent.lock`).
//! - The agent only closes todos through the existing checked commands (`arb todo done`,
//!   `arb inspect done`, the command file); `done` is never set here. A todo it did not
//!   close is `check_failed` (resumed in the same session next time), then `needs_human`.
//! - Its tools are limited (`--allowedTools` / `--disallowedTools`); its Bash runs with
//!   `CLAUDECODE` set, so `arb approve`, `arb note` and seed writes refuse as in any
//!   Claude Code session. Nothing here approves or submits (`tests/guards.rs`).
//! - Each attempt's stream (stream-json, as `claude` wrote it) is kept in
//!   `.arboria/agent/<todo>-<attempt>.jsonl` for the agent panel.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Ledger;
use crate::model::{EventKind, IdeaStage, TaskId, TaskKind, TaskStatus};
use crate::paths::{self, Home, RepoContext};
use crate::tasks::{self, TaskView, Usage};
use crate::{bail, Error, Result};

/// `ARBORIA_CLAUDE`: the `claude` to run.
pub const CLAUDE_ENV: &str = "ARBORIA_CLAUDE";
/// The settings file's name under the config dir.
pub const SETTINGS_NAME: &str = "agent.toml";

/// `<config>/agent.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Queue command and inspect todos as soon as they appear.
    pub auto: bool,
    pub model: String,
    pub max_turns: u32,
    /// Attempts per day, all projects counted per project ledger.
    pub daily_limit: u32,
    /// Seconds before an attempt is stopped.
    pub timeout_s: u64,
    /// The `claude` command (default: `claude` on PATH).
    pub claude: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto: true,
            model: "sonnet".into(),
            max_turns: 20,
            daily_limit: 30,
            timeout_s: 20 * 60,
            claude: None,
        }
    }
}

pub fn settings_path() -> PathBuf {
    crate::clusters::config_dir().join(SETTINGS_NAME)
}

/// The settings (defaults when the file is missing; an unreadable file is an error).
pub fn load_settings() -> Result<Settings> {
    match std::fs::read_to_string(settings_path()) {
        Ok(t) => toml::from_str(&t)
            .map_err(|e| Error::Arb(format!("{}: {e}", settings_path().display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_settings(s: &Settings) -> Result<()> {
    if s.model.trim().is_empty() || s.model.chars().any(char::is_whitespace) {
        bail!("模型名不对：{:?}", s.model);
    }
    if !(1..=200).contains(&s.max_turns) {
        bail!("轮数要在 1 到 200 之间");
    }
    let path = settings_path();
    std::fs::create_dir_all(path.parent().expect("config dir"))?;
    let text = toml::to_string(s).map_err(|e| Error::Arb(e.to_string()))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn claude_exe(s: &Settings) -> String {
    std::env::var(CLAUDE_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| s.claude.clone().filter(|c| !c.is_empty()))
        .unwrap_or_else(|| "claude".into())
}

/// The kinds queued by themselves when `auto` is on: what a notebook block asked for.
pub const AUTO_KINDS: &[TaskKind] = &[
    TaskKind::CommandNew,
    TaskKind::CommandRevise,
    TaskKind::Inspect,
];

/// Tools the agent may use.
pub const ALLOWED_TOOLS: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "Edit",
    "Write",
    "Bash(arb commands:*)",
    "Bash(arb inspect:*)",
    "Bash(arb todo:*)",
    "Bash(arb probe check:*)",
    "Bash(arb probe guide:*)",
    "Bash(arb idea show:*)",
    "Bash(arb status:*)",
    "Bash(git diff:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(ls:*)",
    "Bash(python:*)",
    "Bash(python3:*)",
];

/// Never, whatever the project's settings say.
pub const DISALLOWED_TOOLS: &[&str] = &[
    "Bash(arb approve:*)",
    "Bash(arb lesson accept:*)",
    "Bash(arb note:*)",
    "Bash(arb submit:*)",
    "Bash(arb idea migrate:*)",
    "Bash(arb idea style:*)",
    "Bash(sbatch:*)",
    "Bash(ssh:*)",
    "Edit(**/.arboria/notebooks/**)",
    "Write(**/.arboria/notebooks/**)",
    "Edit(**/notebook.md)",
    "Write(**/notebook.md)",
    "Edit(**/notebook.py)",
    "Write(**/notebook.py)",
    "Edit(**/seeds/**)",
    "Write(**/seeds/**)",
    "Edit(**/.arboria/ledger.db*)",
    "Write(**/.arboria/ledger.db*)",
];

/// The system prompt added to Claude Code's own for every attempt.
pub const SYSTEM: &str = "你在 Arboria 的后台队列里运行（claude -p），没有人在看。只做下面这一个待办，做完就停：不要顺手改别的东西，不要问问题（问不了），实在做不了就用 `arb todo drop` 写明原因。你不能批准、提交作业、写 note、改 notebook 或种子，这些命令会被拒绝。";

/// Where a todo's agent works: its owner's folder and ledger.
#[derive(Debug, Clone)]
pub struct Place {
    pub state_dir: PathBuf,
    /// The idea's worktree (or the main worktree), or the seed home.
    pub cwd: PathBuf,
}

fn agent_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("agent")
}

/// The stream log of one attempt.
pub fn log_path(state_dir: &Path, task: &TaskId, attempt: u32) -> PathBuf {
    agent_dir(state_dir).join(format!("{task}-{attempt}.jsonl"))
}

fn stop_marker(state_dir: &Path, task: &TaskId) -> PathBuf {
    agent_dir(state_dir).join(format!("{task}.stop"))
}

fn pid_file(state_dir: &Path, task: &TaskId) -> PathBuf {
    agent_dir(state_dir).join(format!("{task}.pid"))
}

/// The ledger and folder of the project at `root`, or of the seed home.
pub fn place_of(root: &Path) -> Result<(PathBuf, Option<RepoContext>, Option<Home>)> {
    let home = paths::home();
    if paths::resolve(root) == paths::resolve(&home.root) {
        return Ok((home.state_dir(), None, Some(home)));
    }
    let ctx = paths::locate(Some(root))?.require_initialized()?;
    Ok((ctx.state_dir(), Some(ctx), None))
}

fn cwd_for(
    ledger: &Ledger,
    ctx: Option<&RepoContext>,
    home: Option<&Home>,
    t: &TaskView,
) -> Result<PathBuf> {
    if let Some(h) = home {
        return Ok(h.seeds_dir());
    }
    let ctx = ctx.expect("a project");
    if let Some(name) = t.idea.as_deref() {
        let idea = crate::ideas::get_idea(ledger, &crate::model::IdeaName::parse(name)?)?;
        if let Some(wt) = idea
            .worktree_path
            .filter(|w| idea.stage == IdeaStage::Open && Path::new(w).is_dir())
        {
            return Ok(PathBuf::from(wt));
        }
    }
    Ok(ctx.main.clone())
}

/// Queue the open todos of [`AUTO_KINDS`]; the ids queued.
pub fn enqueue_auto(ledger: &mut Ledger) -> Result<Vec<TaskId>> {
    let open = tasks::list_tasks(ledger, None, false)?;
    let mut out = Vec::new();
    for t in open {
        if AUTO_KINDS.contains(&t.task.kind) {
            tasks::enqueue(ledger, &t.task.id)?;
            out.push(t.task.id);
        }
    }
    Ok(out)
}

/// needs_human / check_failed → queued again (the human's "retry").
pub fn requeue(ledger: &mut Ledger, task: &TaskId) -> Result<TaskView> {
    let t = tasks::get_task(ledger, task)?;
    match t.task.status {
        TaskStatus::Open => tasks::enqueue(ledger, task),
        TaskStatus::NeedsHuman | TaskStatus::CheckFailed => {
            ledger.write(|tx| {
                tx.execute(
                    "UPDATE tasks SET status = 'queued', attempts = 0, session_id = NULL WHERE id = ?1",
                    [task],
                )?;
                tx.event(
                    EventKind::TodoQueue,
                    task.as_str(),
                    &serde_json::json!({"kind": t.task.kind.as_str(), "again": true}),
                )
            })?;
            tasks::get_task(ledger, task)
        }
        other => bail!("todo {task} is {other}: nothing to retry"),
    }
}

/// Attempts started today (UTC) in this ledger.
pub fn attempts_today(ledger: &Ledger) -> Result<u32> {
    let today = &crate::db::now()[..10];
    let n: i64 = ledger.conn().query_row(
        "SELECT COUNT(*) FROM events WHERE kind = 'todo_attempt' AND ts >= ?1 \
         AND json_extract(payload_json, '$.action') = 'start'",
        [format!("{today}T00:00:00")],
        |r| r.get(0),
    )?;
    Ok(n as u32)
}

/// The prompt of one todo.
pub fn prompt_for(t: &TaskView) -> String {
    let id = t.task.id.as_str();
    let (guide, close) = match t.task.kind {
        TaskKind::CommandNew | TaskKind::CommandRevise => (
            crate::commands::guide(),
            format!("写好命令文件后用 `arb todo done {id} --result \"…\"` 关掉它（≤600 字）。"),
        ),
        TaskKind::Inspect => (
            crate::inspection::guide(),
            format!("用 `arb inspect check {id} --file -` 检查答案，再用 `arb inspect done {id} --file -` 交付。"),
        ),
        TaskKind::ProbeNew | TaskKind::ProbeRevise => (
            crate::probe::guide().to_string(),
            format!("写好后用 `arb todo done {id} --result \"…\"` 关掉它。"),
        ),
        TaskKind::Plain => (
            String::new(),
            format!("做完用 `arb todo done {id} --result \"…\"` 关掉它（≤600 字），做不了用 `arb todo drop {id}`。"),
        ),
    };
    let mut p = format!(
        "待办 {id}（{}，来自 {}）：\n\n{}\n",
        t.task.kind.as_str(),
        t.owner_name(),
        t.task.text
    );
    if let Some(payload) = t.task.payload_json.as_deref().filter(|p| !p.is_empty()) {
        p.push_str(&format!("\npayload: {payload}\n"));
    }
    if !guide.is_empty() {
        p.push_str(&format!("\n按下面的约定做：\n\n{guide}\n"));
    }
    p.push_str(&format!("\n{close}"));
    p
}

/// `claude` arguments for one attempt.
pub fn claude_args(s: &Settings, prompt: &str, resume: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(),
        prompt.into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--model".into(),
        s.model.clone(),
        "--max-turns".into(),
        s.max_turns.to_string(),
        "--append-system-prompt".into(),
        SYSTEM.into(),
        "--permission-mode".into(),
        "acceptEdits".into(),
        "--allowedTools".into(),
        ALLOWED_TOOLS.join(","),
        "--disallowedTools".into(),
        DISALLOWED_TOOLS.join(","),
    ];
    if let Some(sid) = resume {
        a.push("--resume".into());
        a.push(sid.into());
    }
    a
}

/// What the `result` event of a stream said.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Outcome {
    pub session_id: String,
    pub usage: Usage,
    pub is_error: bool,
    /// The agent's last message.
    pub result: String,
}

/// Read a stream-json log: the last `result` event (the session id also from `init`).
pub fn parse_stream(text: &str) -> Outcome {
    let mut out = Outcome::default();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(sid) = v["session_id"].as_str() {
            out.session_id = sid.to_string();
        }
        if v["type"] == "result" {
            let u = &v["usage"];
            out.usage = Usage {
                input_tokens: u["input_tokens"].as_u64().unwrap_or(0)
                    + u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                output_tokens: u["output_tokens"].as_u64().unwrap_or(0),
                cache_read_tokens: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
                turns: v["num_turns"].as_u64().unwrap_or(0) as u32,
                cost_usd: v["total_cost_usd"].as_f64().unwrap_or(0.0),
            };
            out.is_error = v["is_error"].as_bool().unwrap_or(false)
                || v["subtype"].as_str().is_some_and(|s| s != "success");
            out.result = v["result"].as_str().unwrap_or("").to_string();
        }
    }
    out
}

/// What one call to [`run_one`] did.
#[derive(Debug, Clone, Serialize)]
pub enum Ran {
    /// Nothing queued.
    Idle,
    /// Another `arb agent` holds this project's lock.
    Busy,
    /// Today's attempts reached `daily_limit`.
    Limit(u32),
    /// One attempt; the todo afterwards.
    Attempt(Box<TaskView>, Outcome),
}

fn env_clean(cmd: &mut Command) {
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("CLAUDE_CODE_MESSAGING") {
            cmd.env_remove(&k);
        }
    }
}

/// Take the oldest queued todo of the project (or seed home) at `root` and run one
/// `claude -p` attempt on it. `on_line` sees each stream line as it comes.
pub fn run_one(root: &Path, s: &Settings, on_line: &mut dyn FnMut(&str)) -> Result<Ran> {
    let (state_dir, ctx, home) = place_of(root)?;
    std::fs::create_dir_all(agent_dir(&state_dir))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("agent.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(Ran::Busy),
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    let mut ledger = Ledger::open(&state_dir)?;
    if s.auto {
        enqueue_auto(&mut ledger)?;
    }
    let today = attempts_today(&ledger)?;
    if today >= s.daily_limit {
        let queued: i64 = ledger.conn().query_row(
            "SELECT COUNT(*) FROM tasks WHERE status = 'queued'",
            [],
            |r| r.get(0),
        )?;
        return Ok(if queued > 0 {
            Ran::Limit(today)
        } else {
            Ran::Idle
        });
    }
    let Some(task) = tasks::claim_next(&mut ledger, &s.model)? else {
        return Ok(Ran::Idle);
    };
    let id = task.task.id.clone();
    let attempt = task.task.attempts;
    let cwd = cwd_for(&ledger, ctx.as_ref(), home.as_ref(), &task)?;
    let resume = task
        .task
        .session_id
        .clone()
        .filter(|_| task.task.status == TaskStatus::Running && attempt > 1);
    let prompt = if resume.is_some() {
        format!(
            "待办 {id} 还没有关掉（上一次结束时它还是打开的）。检查哪里没做完，修好，然后照约定关掉它。\n\n{}",
            prompt_for(&task)
        )
    } else {
        prompt_for(&task)
    };
    let log = log_path(&state_dir, &id, attempt);
    let _ = std::fs::remove_file(stop_marker(&state_dir, &id));
    let mut file = std::fs::File::create(&log)?;
    let mut cmd = Command::new(claude_exe(s));
    cmd.args(claude_args(s, &prompt, resume.as_deref()))
        .current_dir(&cwd)
        .env("ARBORIA_AGENT_TASK", id.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    env_clean(&mut cmd);
    let spawned = cmd.spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let why = format!(
                "cannot run {} ({e}); set ARBORIA_CLAUDE or install Claude Code",
                claude_exe(s)
            );
            writeln!(
                file,
                "{}",
                serde_json::json!({"type": "arboria", "error": why})
            )?;
            let t = tasks::finish_attempt(
                &mut ledger,
                &id,
                "",
                &Usage::default(),
                TaskStatus::NeedsHuman,
            )?;
            return Ok(Ran::Attempt(
                Box::new(t),
                Outcome {
                    is_error: true,
                    result: why,
                    ..Default::default()
                },
            ));
        }
    };
    std::fs::write(pid_file(&state_dir, &id), child.id().to_string())?;
    let stderr = child.stderr.take().expect("piped");
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        for l in BufReader::new(stderr)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if s.len() < 20_000 {
                s.push_str(&l);
                s.push('\n');
            }
        }
        s
    });
    let started = Instant::now();
    let timeout = Duration::from_secs(s.timeout_s);
    let pid = child.id();
    let finished = Arc::new(AtomicBool::new(false));
    let seen = finished.clone();
    // stops an attempt that runs too long (never a process that already ended)
    std::thread::spawn(move || {
        while started.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(250));
            if seen.load(Ordering::SeqCst) {
                return;
            }
        }
        if !seen.load(Ordering::SeqCst) {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    });
    let mut text = String::new();
    let stdout = child.stdout.take().expect("piped");
    for line in BufReader::new(stdout)
        .lines()
        .map_while(std::result::Result::ok)
    {
        writeln!(file, "{line}")?;
        on_line(&line);
        text.push_str(&line);
        text.push('\n');
    }
    let status = child.wait()?;
    finished.store(true, Ordering::SeqCst);
    let err = err_thread.join().unwrap_or_default();
    if !err.trim().is_empty() {
        writeln!(
            file,
            "{}",
            serde_json::json!({"type": "arboria", "stderr": err})
        )?;
    }
    let _ = std::fs::remove_file(pid_file(&state_dir, &id));
    let mut outcome = parse_stream(&text);
    if !status.success() && outcome.result.is_empty() {
        outcome.is_error = true;
        outcome.result = format!(
            "claude exited with {status}: {}",
            err.trim().lines().last().unwrap_or("")
        );
    }
    let stopped = stop_marker(&state_dir, &id);
    let next = if stopped.exists() {
        let _ = std::fs::remove_file(&stopped);
        TaskStatus::NeedsHuman
    } else {
        TaskStatus::CheckFailed
    };
    let t = tasks::finish_attempt(&mut ledger, &id, &outcome.session_id, &outcome.usage, next)?;
    Ok(Ran::Attempt(Box::new(t), outcome))
}

/// Stop the running attempt of a todo: it becomes `needs_human`.
pub fn stop(root: &Path, task: &TaskId) -> Result<bool> {
    let (state_dir, _, _) = place_of(root)?;
    let pid = match std::fs::read_to_string(pid_file(&state_dir, task)) {
        Ok(p) => p.trim().to_string(),
        Err(_) => return Ok(false),
    };
    std::fs::write(stop_marker(&state_dir, task), "")?;
    Ok(Command::new("kill").arg(&pid).status()?.success())
}

/// One line of the agent panel's log view, from a stream line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogLine {
    /// `text` (the agent wrote), `tool` (a tool call), `result` (a tool's answer),
    /// `done` (the final result), `error`.
    pub kind: String,
    pub text: String,
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// The readable lines of a stream log.
pub fn log_lines(text: &str) -> Vec<LogLine> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("assistant") => {
                for c in v["message"]["content"].as_array().into_iter().flatten() {
                    match c["type"].as_str() {
                        Some("text") => out.push(LogLine {
                            kind: "text".into(),
                            text: clip(c["text"].as_str().unwrap_or(""), 4000),
                        }),
                        Some("tool_use") => {
                            let input = &c["input"];
                            let what = input["command"]
                                .as_str()
                                .or(input["file_path"].as_str())
                                .or(input["pattern"].as_str())
                                .map(str::to_string)
                                .unwrap_or_else(|| input.to_string());
                            out.push(LogLine {
                                kind: "tool".into(),
                                text: clip(
                                    &format!("{} · {what}", c["name"].as_str().unwrap_or("?")),
                                    600,
                                ),
                            });
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for c in v["message"]["content"].as_array().into_iter().flatten() {
                    if c["type"] == "tool_result" {
                        let body = match &c["content"] {
                            Value::String(s) => s.clone(),
                            Value::Array(a) => a
                                .iter()
                                .filter_map(|x| x["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n"),
                            _ => String::new(),
                        };
                        out.push(LogLine {
                            kind: if c["is_error"].as_bool() == Some(true) {
                                "error"
                            } else {
                                "result"
                            }
                            .into(),
                            text: clip(body.trim(), 600),
                        });
                    }
                }
            }
            Some("result") => out.push(LogLine {
                kind: "done".into(),
                text: clip(v["result"].as_str().unwrap_or(""), 4000),
            }),
            Some("arboria") => out.push(LogLine {
                kind: "error".into(),
                text: clip(
                    v["error"].as_str().or(v["stderr"].as_str()).unwrap_or(""),
                    2000,
                ),
            }),
            _ => {}
        }
    }
    out
}

/// The latest attempt's log of a todo, as lines.
pub fn latest_log(state_dir: &Path, task: &TaskView) -> Vec<LogLine> {
    let mut attempt = task.task.attempts;
    while attempt > 0 {
        if let Ok(t) = std::fs::read_to_string(log_path(state_dir, &task.task.id, attempt)) {
            return log_lines(&t);
        }
        attempt -= 1;
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = r#"{"type":"system","subtype":"init","session_id":"s-1"}
{"type":"assistant","message":{"content":[{"type":"text","text":"我来写 svd"},{"type":"tool_use","name":"Bash","input":{"command":"arb commands guide"}}]},"session_id":"s-1"}
{"type":"user","message":{"content":[{"type":"tool_result","content":"约定……","is_error":false}]},"session_id":"s-1"}
{"type":"result","subtype":"success","is_error":false,"num_turns":3,"result":"写好了","session_id":"s-1","total_cost_usd":0.12,"usage":{"input_tokens":100,"cache_creation_input_tokens":50,"cache_read_input_tokens":3000,"output_tokens":400}}
"#;

    #[test]
    fn streams_are_read() {
        let o = parse_stream(STREAM);
        assert_eq!(o.session_id, "s-1");
        assert!(!o.is_error);
        assert_eq!(o.result, "写好了");
        assert_eq!(
            o.usage,
            Usage {
                input_tokens: 150,
                output_tokens: 400,
                cache_read_tokens: 3000,
                turns: 3,
                cost_usd: 0.12
            }
        );
        let lines = log_lines(STREAM);
        let kinds: Vec<_> = lines.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(kinds, ["text", "tool", "result", "done"]);
        assert_eq!(lines[1].text, "Bash · arb commands guide");
    }

    #[test]
    fn the_agent_cannot_sign_or_edit_notebooks() {
        let a = claude_args(&Settings::default(), "p", Some("s-1"));
        let joined = a.join(" ");
        assert!(joined.contains("--resume s-1"));
        for d in [
            "Bash(arb approve:*)",
            "Edit(**/notebook.md)",
            "Write(**/.arboria/notebooks/**)",
        ] {
            assert!(DISALLOWED_TOOLS.contains(&d), "{d}");
        }
        assert!(!ALLOWED_TOOLS
            .iter()
            .any(|t| t.contains("approve") || t.contains("submit")));
    }
}
