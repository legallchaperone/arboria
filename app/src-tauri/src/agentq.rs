//! The agent queue in the app (ADR 0005): the panel's rows and logs, the settings, and
//! the background thread that runs queued todos with `claude -p` while the app is open
//! (`arboria_core::agent::run_one`, one attempt at a time over all listed projects).
//! Handing a todo to the agent, retrying and stopping go through `arb` (`actions.rs`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use arboria_core::agent::{self, LogLine, Ran, Settings};
use arboria_core::db::Ledger;
use arboria_core::model::{TaskId, TaskStatus};
use arboria_core::tasks::{self, TaskView, Usage};
use arboria_core::{human, paths, registry};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// One row of the agent panel.
#[derive(Debug, Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub task: TaskView,
    pub usage: Option<Usage>,
}

/// Every todo of the project (or seed home) at `root`: not finished ones, then the last
/// 20 finished.
pub fn rows(root: &Path) -> Result<Vec<Row>, String> {
    let (state_dir, _, _) = agent::place_of(root).map_err(err)?;
    if !state_dir.join(arboria_core::db::DB_NAME).is_file() {
        return Ok(Vec::new());
    }
    let ledger = Ledger::open(&state_dir).map_err(err)?;
    let all = tasks::list_tasks(&ledger, None, true).map_err(err)?;
    let (mut live, mut done): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|t| !matches!(t.task.status, TaskStatus::Done | TaskStatus::Dropped));
    done.sort_by(|a, b| b.task.done_at.cmp(&a.task.done_at));
    done.truncate(20);
    live.extend(done);
    Ok(live
        .into_iter()
        .map(|t| Row {
            usage: t
                .task
                .usage_json
                .as_deref()
                .and_then(|u| serde_json::from_str(u).ok()),
            task: t,
        })
        .collect())
}

pub fn log(root: &Path, task: &str) -> Result<Vec<LogLine>, String> {
    let (state_dir, _, _) = agent::place_of(root).map_err(err)?;
    let ledger = Ledger::open(&state_dir).map_err(err)?;
    let t = tasks::get_task(&ledger, &TaskId::parse(task).map_err(err)?).map_err(err)?;
    Ok(agent::latest_log(&state_dir, &t))
}

fn roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = registry::list()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.path)
        .collect();
    let home = paths::home();
    if home.initialized() {
        out.push(home.root);
    }
    out
}

/// One stream line of the running attempt in the project at `root`.
#[derive(Debug, Clone, Serialize)]
struct Line<'a> {
    root: &'a str,
    line: &'a str,
}

/// The dispatcher: every 10 s, one pass over the listed projects; each runs its queue
/// until empty. Not started when the app itself runs inside Claude Code.
pub fn start(app: AppHandle) {
    if human::in_claude_code().is_some() {
        return;
    }
    std::thread::spawn(move || loop {
        let settings: Settings = agent::load_settings().unwrap_or_default();
        for root in roots() {
            let root_s = root.display().to_string();
            loop {
                let mut first = true;
                let ran = agent::run_one(&root, &settings, &mut |line| {
                    if first {
                        // the todo is running now: the panel re-reads its rows
                        let _ = app.emit("agent-changed", &root_s);
                        first = false;
                    }
                    let _ = app.emit(
                        "agent-line",
                        Line {
                            root: &root_s,
                            line,
                        },
                    );
                });
                match ran {
                    Ok(Ran::Attempt(..)) => {
                        let _ = app.emit("agent-changed", &root_s);
                        continue;
                    }
                    Ok(_) => break,
                    Err(_) => break,
                }
            }
        }
        std::thread::sleep(Duration::from_secs(10));
    });
}
