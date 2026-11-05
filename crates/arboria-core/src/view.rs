//! Everything the app shows (ADR 0004): one read-only pass over a project's ledger (or the
//! seed home's) into JSON-ready structs. Ports what `arboria.ui.data` loads for the marimo
//! dashboard, reshaped for the app's three screens (inbox, overview, idea).
//!
//! Invariants (the same rule as the Python dashboard, `tests/test_ui.py`):
//! - Read-only: the ledger is opened with [`crate::db::open_read_only`]; this module
//!   calls no write function, no backend, no subprocess and no network (a source scan in
//!   `tests/view.rs` checks the imports). The card is recomputed from local files only.
//! - Text from the ledger is data: the app renders it as text, never as HTML.
//! - The inbox rule for finished runs is [`crate::report::FINISHED_UNSEEN_SQL`]'s: ended
//!   after the idea's last human note.

use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::model::{Idea, IdeaStage, Job, Lesson, Note, Run, RunStatus, Task, TaskStatus};
use crate::paths::{Home, RepoContext, STATE_DIR_NAME};
use crate::{cards, config, db, ideas, probe, Result};

/// Lines of `notifications.log` shown (newest first).
pub const NOTIFICATIONS_SHOWN: usize = 20;
/// Bytes read from the end of `notifications.log`.
pub const NOTIFICATIONS_TAIL_BYTES: u64 = 64 * 1024;
/// Done todos stay in the inbox this long (when no human note came after them).
pub const AGENT_DONE_DAYS: i64 = 7;

/// Which kind of place was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    /// A repo's main worktree.
    Project,
    /// The seed home (种子园).
    Home,
}

/// What was read and when.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectInfo {
    pub root: PathBuf,
    /// The directory name (the home is “种子园”).
    pub name: String,
    pub kind: Place,
    pub project_id: Option<String>,
    /// `[budget] total_su` (0: none).
    pub total_su: f64,
    /// `arboria.toml` could not be read: why.
    pub config_error: Option<String>,
    pub loaded_at: String,
}

/// An idea (or a seed: `stage = "seed"`, or `"planted"` once it is in a repo).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IdeaView {
    pub id: i64,
    pub name: String,
    pub stage: String,
    pub confidence: String,
    pub gist: Option<String>,
    pub hypothesis: Option<String>,
    pub metric: Option<String>,
    pub parent: Option<String>,
    /// The run it was forked from (the `idea_new` event).
    pub from_run: Option<String>,
    pub created_at: String,
    pub closed_at: Option<String>,
    /// Latest of created, closed, its runs' updates and its notes.
    pub last_active: String,
    pub su_used: f64,
    pub runs: u32,
    pub worktree: Option<String>,
    /// The notebook file, when it exists: the app's `.md`, else a marimo `.py` (not
    /// migrated yet).
    pub notebook: Option<PathBuf>,
}

/// One Slurm job (segment) of a run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobView {
    pub segment: u32,
    pub job_id: Option<String>,
    pub state: String,
    pub elapsed_s: Option<i64>,
    pub billed_su: Option<f64>,
}

/// A run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunView {
    pub id: String,
    pub idea: String,
    pub parent_run: Option<String>,
    pub status: String,
    pub change_reason: String,
    pub machine: Option<String>,
    pub partition: Option<String>,
    pub gpus: Option<i64>,
    pub walltime: Option<String>,
    pub est_su: Option<f64>,
    /// Sum of the jobs' billed SU.
    pub billed_su: f64,
    pub summary: Option<String>,
    pub wandb_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Final runs: the latest `run_final` event, else `updated_at`.
    pub finished_at: Option<String>,
    pub jobs: Vec<JobView>,
}

/// What an inbox item asks of the human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    /// A run waits for the human's signature (要签字).
    Approve,
    /// A run failed or was cancelled (失败).
    Failed,
    /// A run succeeded (跑完了).
    Finished,
    /// A todo the agent finished (agent 写好了).
    AgentDone,
    /// A proposed lesson (待确认经验).
    Lesson,
    /// A probe file changed since it was enabled (待重新启用).
    ProbeReenable,
}

/// One thing that needs the human.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InboxItem {
    pub kind: InboxKind,
    pub when: String,
    pub title: String,
    pub meta: String,
    pub idea: Option<String>,
    pub run: Option<String>,
    pub task: Option<String>,
    pub lesson: Option<String>,
    pub probe: Option<String>,
    /// The command the human can run instead (approve, lesson accept, probe enable).
    pub command: Option<String>,
}

/// The card of a run awaiting approval, as `arb card` prints it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CardView {
    pub run: String,
    pub text: Option<String>,
    pub card_hash: Option<String>,
    /// The recomputed hash equals the ledger's (else `arb approve` refuses).
    pub matches: Option<bool>,
    pub est_su: Option<f64>,
    pub resources: Vec<String>,
    pub estimate: Vec<String>,
    pub budget: Vec<String>,
    pub error: Option<String>,
}

/// A todo (the agent's queue).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TodoView {
    pub id: String,
    pub idea: String,
    pub kind: String,
    pub status: String,
    pub text: String,
    pub created_at: String,
    pub done_at: Option<String>,
    pub result: Option<String>,
}

/// A note.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteView {
    pub id: i64,
    pub idea: String,
    pub run: Option<String>,
    pub text: String,
    /// `human` / `agent`; None predates v4 (the human's).
    pub author: Option<String>,
    pub created_at: String,
}

/// A proposed lesson.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LessonView {
    pub id: String,
    pub text: String,
    pub created_at: String,
}

/// One line of `notifications.log`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotificationView {
    pub at: Option<String>,
    pub text: String,
}

/// SU of one idea.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IdeaSpend {
    pub idea: String,
    pub su: f64,
}

/// Money.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Spend {
    pub used: f64,
    pub total: f64,
    pub by_idea: Vec<IdeaSpend>,
}

/// The latest result of one probe on one machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeView {
    pub machine: String,
    pub probe: String,
    pub at: String,
    pub result: Value,
}

/// One read of a project (or the seed home).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Board {
    pub project: ProjectInfo,
    pub ideas: Vec<IdeaView>,
    pub runs: Vec<RunView>,
    pub inbox: Vec<InboxItem>,
    pub cards: Vec<CardView>,
    pub todos: Vec<TodoView>,
    pub notes: Vec<NoteView>,
    pub lessons: Vec<LessonView>,
    pub notifications: Vec<NotificationView>,
    pub spend: Spend,
    pub probes: Vec<ProbeView>,
}

/// One row of the project list: its inbox, or why it cannot be read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectSummary {
    pub root: PathBuf,
    pub name: String,
    pub kind: Place,
    pub error: Option<String>,
    pub open_ideas: u32,
    pub seeds: u32,
    pub inbox: Vec<InboxItem>,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// `1.5` → `1.5`, `12.0` → `12`, like the card's numbers.
fn su_text(x: f64) -> String {
    let r = round2(x);
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r}")
    }
}

fn clip(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or_default();
    if line.chars().count() > n || s.lines().nth(1).is_some() {
        line.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    } else {
        line.to_string()
    }
}

/// A context for config / card reads without running git (the root is known).
fn context(root: &Path) -> RepoContext {
    RepoContext {
        cwd: root.to_path_buf(),
        common_dir: root.join(".git"),
        main: root.to_path_buf(),
        toplevel: Some(root.to_path_buf()),
    }
}

fn is_home(root: &Path) -> bool {
    Home {
        root: root.to_path_buf(),
    }
    .initialized()
}

/// Read the project whose main worktree is `root`, or the seed home at `root`.
pub fn board(root: &Path) -> Result<Board> {
    let root = crate::paths::resolve(root);
    let kind = if is_home(&root) {
        Place::Home
    } else {
        Place::Project
    };
    let ctx = context(&root);
    let mut info = ProjectInfo {
        name: match kind {
            Place::Home => "种子园".to_string(),
            Place::Project => root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        },
        root: root.clone(),
        kind,
        project_id: None,
        total_su: 0.0,
        config_error: None,
        loaded_at: db::now(),
    };
    if kind == Place::Project {
        match config::project_config(&ctx) {
            Ok(cfg) => {
                info.project_id = cfg.project_id.map(|p| p.as_str().to_string());
                info.total_su = cfg.total_su;
            }
            Err(e) => info.config_error = Some(format!("{}: {e}", crate::paths::CONFIG_NAME)),
        }
    }
    let state_dir = root.join(STATE_DIR_NAME);
    let conn = db::open_read_only(&state_dir.join(db::DB_NAME))?;
    let mut b = load(&conn, info, &state_dir)?;
    if kind == Place::Project {
        b.cards = cards_of(&ctx, &conn, &b)?;
    }
    b.inbox = inbox(&conn, &b, &ctx)?;
    b.notifications = notifications(&state_dir);
    Ok(b)
}

fn load(conn: &Connection, project: ProjectInfo, state_dir: &Path) -> Result<Board> {
    let kind = project.kind;
    let root = project.root.clone();
    let mut from_run: HashMap<String, String> = HashMap::new();
    let mut finished: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT kind, subject, ts, payload_json FROM events \
             WHERE kind IN ('idea_new', 'run_final') ORDER BY id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let kind: String = r.get(0)?;
            let subject: Option<String> = r.get(1)?;
            let Some(subject) = subject else { continue };
            if kind == "run_final" {
                finished.insert(subject, r.get(2)?);
                continue;
            }
            let payload: Option<String> = r.get(3)?;
            let fr = payload
                .and_then(|p| serde_json::from_str::<Value>(&p).ok())
                .and_then(|v| v["from_run"].as_str().map(str::to_string));
            if let Some(fr) = fr {
                from_run.insert(subject, fr);
            }
        }
    }
    let ideas: Vec<Idea> = db::all(conn, "SELECT * FROM ideas ORDER BY id", [])?;
    let names: HashMap<i64, String> = ideas
        .iter()
        .map(|i| (i.id.0, i.name.as_str().to_string()))
        .collect();
    let name_of = |id: i64| names.get(&id).cloned().unwrap_or_default();
    let mut jobs: HashMap<String, Vec<Job>> = HashMap::new();
    for j in db::all::<Job>(conn, "SELECT * FROM jobs ORDER BY run_id, segment", [])? {
        jobs.entry(j.run_id.as_str().to_string())
            .or_default()
            .push(j);
    }
    let runs: Vec<Run> = db::all(conn, "SELECT * FROM runs ORDER BY id", [])?;
    let run_views: Vec<RunView> = runs
        .iter()
        .map(|r| {
            let id = r.id.as_str().to_string();
            let js = jobs.remove(&id).unwrap_or_default();
            let billed = round2(js.iter().filter_map(|j| j.billed_su).sum());
            let final_ = RunStatus::FINAL.contains(&r.status);
            RunView {
                idea: name_of(r.idea_id.0),
                parent_run: r.parent_run.as_ref().map(|p| p.as_str().to_string()),
                status: r.status.as_str().to_string(),
                change_reason: r.change_reason.clone(),
                machine: r.machine.clone(),
                partition: r.partition.clone(),
                gpus: r.gpus,
                walltime: r.walltime.clone(),
                est_su: r.est_cost,
                billed_su: billed,
                summary: r.summary.clone(),
                wandb_url: r.wandb_url.clone(),
                created_at: r.created_at.clone(),
                updated_at: r.updated_at.clone(),
                finished_at: final_
                    .then(|| finished.get(&id).cloned().unwrap_or(r.updated_at.clone())),
                jobs: js
                    .into_iter()
                    .map(|j| JobView {
                        segment: j.segment,
                        job_id: j.job_id,
                        state: j.state.as_str().to_string(),
                        elapsed_s: j.elapsed_s,
                        billed_su: j.billed_su,
                    })
                    .collect(),
                id,
            }
        })
        .collect();
    let notes: Vec<Note> = db::all(conn, "SELECT * FROM notes ORDER BY id DESC", [])?;
    // a planted seed is still a `seed` idea in the home's ledger; `arb idea list` says
    // 已种下 from its plantings, and so does the view
    let planted: std::collections::HashSet<i64> = if kind == Place::Home {
        let mut stmt = conn.prepare("SELECT DISTINCT seed_idea_id FROM plantings")?;
        let ids = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        ids.collect::<rusqlite::Result<_>>()?
    } else {
        Default::default()
    };
    let idea_views: Vec<IdeaView> = ideas
        .iter()
        .map(|i| {
            let name = i.name.as_str().to_string();
            let mine: Vec<&RunView> = run_views.iter().filter(|r| r.idea == name).collect();
            let mut last = i.created_at.clone();
            let stamps = i
                .closed_at
                .iter()
                .chain(mine.iter().map(|r| &r.updated_at))
                .chain(
                    notes
                        .iter()
                        .filter(|n| n.idea_id == i.id)
                        .map(|n| &n.created_at),
                );
            for s in stamps {
                if *s > last {
                    last = s.clone();
                }
            }
            // the app's notebook (`.md`) when there is one, else the marimo file
            let (md, py) = match kind {
                Place::Home => {
                    let seeds = root.join(crate::paths::SEEDS_DIR);
                    (
                        crate::nbdoc::seed_path(&seeds, &name),
                        seeds.join(format!("{name}.py")),
                    )
                }
                Place::Project => (
                    crate::nbdoc::idea_path(state_dir, &name),
                    ideas::notebook_path(state_dir, &name),
                ),
            };
            let notebook = if md.is_file() { md } else { py };
            IdeaView {
                id: i.id.0,
                stage: if i.stage == IdeaStage::Seed && planted.contains(&i.id.0) {
                    "planted".to_string()
                } else {
                    i.stage.as_str().to_string()
                },
                confidence: i.confidence.as_str().to_string(),
                gist: i.gist.clone(),
                hypothesis: i.hypothesis.clone(),
                metric: i.metric.clone(),
                parent: i.parent_id.map(|p| name_of(p.0)),
                from_run: from_run.get(&name).cloned(),
                created_at: i.created_at.clone(),
                closed_at: i.closed_at.clone(),
                last_active: last,
                su_used: round2(mine.iter().map(|r| r.billed_su).sum()),
                runs: mine.len() as u32,
                worktree: i.worktree_path.clone(),
                notebook: notebook.is_file().then_some(notebook),
                name,
            }
        })
        .collect();
    let tasks: Vec<Task> = db::all(conn, "SELECT * FROM tasks ORDER BY id", [])?;
    let todos = tasks
        .iter()
        .filter(|t| !matches!(t.status, TaskStatus::Dropped))
        .map(|t| TodoView {
            id: t.id.as_str().to_string(),
            idea: name_of(t.idea_id.0),
            kind: t.kind.as_str().to_string(),
            status: t.status.as_str().to_string(),
            text: t.text.clone(),
            created_at: t.created_at.clone(),
            done_at: t.done_at.clone(),
            result: t.result.clone(),
        })
        .collect();
    let lessons: Vec<Lesson> = db::all(
        conn,
        "SELECT * FROM lessons WHERE status = 'proposed' ORDER BY id",
        [],
    )?;
    let used = round2(run_views.iter().map(|r| r.billed_su).sum());
    let mut by_idea: Vec<IdeaSpend> = idea_views
        .iter()
        .filter(|i| i.su_used > 0.0)
        .map(|i| IdeaSpend {
            idea: i.name.clone(),
            su: i.su_used,
        })
        .collect();
    by_idea.sort_by(|a, b| b.su.total_cmp(&a.su));
    let total = project.total_su;
    Ok(Board {
        project,
        ideas: idea_views,
        runs: run_views,
        inbox: Vec::new(),
        cards: Vec::new(),
        todos,
        notes: notes
            .iter()
            .map(|n| NoteView {
                id: n.id.0,
                idea: name_of(n.idea_id.0),
                run: n.run_id.as_ref().map(|r| r.as_str().to_string()),
                text: n.text.clone(),
                author: n.author.clone(),
                created_at: n.created_at.clone(),
            })
            .collect(),
        lessons: lessons
            .iter()
            .map(|l| LessonView {
                id: l.id.as_str().to_string(),
                text: l.text.clone(),
                created_at: l.created_at.clone(),
            })
            .collect(),
        notifications: Vec::new(),
        spend: Spend {
            used,
            total,
            by_idea,
        },
        probes: probes(conn)?,
    })
}

fn probes(conn: &Connection) -> Result<Vec<ProbeView>> {
    let mut stmt = conn.prepare(
        "SELECT machine, probe, at, result_json FROM probe_results p \
         WHERE id = (SELECT MAX(id) FROM probe_results q \
                     WHERE q.machine = p.machine AND q.probe = p.probe) \
         ORDER BY machine, probe",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (machine, probe, at, json) = row?;
        out.push(ProbeView {
            machine,
            probe,
            at,
            result: serde_json::from_str(&json).unwrap_or(Value::Null),
        });
    }
    Ok(out)
}

fn card_of(ctx: &RepoContext, conn: &Connection, run: &Run, idea: &Idea) -> Result<cards::Card> {
    let (profile, plan) = cards::replan(run)?;
    cards::build_card(ctx, conn, run, idea, &profile, &plan, &[])
}

fn cards_of(ctx: &RepoContext, conn: &Connection, b: &Board) -> Result<Vec<CardView>> {
    let mut out = Vec::new();
    for r in b.runs.iter().filter(|r| r.status == "awaiting_approval") {
        let run: Run =
            db::one(conn, "SELECT * FROM runs WHERE id = ?1", [&r.id])?.expect("listed run");
        let idea: Idea = db::one(conn, "SELECT * FROM ideas WHERE id = ?1", [run.idea_id.0])?
            .expect("the run's idea");
        let view = match card_of(ctx, conn, &run, &idea) {
            Ok(c) => CardView {
                run: r.id.clone(),
                text: Some(c.text()),
                matches: Some(Some(&c.card_hash) == run.card_hash.as_ref()),
                card_hash: Some(c.card_hash),
                est_su: Some(c.est_su),
                resources: c.resources,
                estimate: c.estimate,
                budget: c.budget,
                error: None,
            },
            Err(e @ (crate::Error::Sqlite(_) | crate::Error::Io(_))) => return Err(e),
            Err(e) => CardView {
                run: r.id.clone(),
                text: None,
                card_hash: run.card_hash.clone(),
                matches: None,
                est_su: None,
                resources: Vec::new(),
                estimate: Vec::new(),
                budget: Vec::new(),
                error: Some(e.to_string()),
            },
        };
        out.push(view);
    }
    Ok(out)
}

/// The latest human note per idea id (notes without an author predate v4: the human's).
fn last_human_note(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT i.name, MAX(n.created_at) FROM notes n JOIN ideas i ON i.id = n.idea_id \
         WHERE n.author IS NULL OR n.author = 'human' GROUP BY i.name",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn days_before(ts: &str, days: i64) -> String {
    let secs = parse_utc(ts).unwrap_or(0) - days * 86_400;
    db::format_utc(secs)
}

/// `2026-11-02T21:24:00Z` → unix seconds.
pub fn parse_utc(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| ts.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, s) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // days from civil (Howard Hinnant)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + s)
}

fn inbox(conn: &Connection, b: &Board, ctx: &RepoContext) -> Result<Vec<InboxItem>> {
    let mut out = Vec::new();
    let item = |kind, when: &str, title: String, meta: String| InboxItem {
        kind,
        when: when.to_string(),
        title,
        meta,
        idea: None,
        run: None,
        task: None,
        lesson: None,
        probe: None,
        command: None,
    };
    let left = (b.project.total_su > 0.0).then_some(b.project.total_su - b.spend.used);
    for r in b.runs.iter().filter(|r| r.status == "awaiting_approval") {
        let mut meta = vec![r.idea.clone()];
        if let (Some(g), Some(t)) = (r.gpus, &r.walltime) {
            meta.push(format!("{g} GPU × {t}"));
        }
        if let Some(m) = &r.machine {
            meta.push(m.clone());
        }
        if let Some(est) = b
            .cards
            .iter()
            .find(|c| c.run == r.id)
            .and_then(|c| c.est_su)
            .or(r.est_su)
        {
            meta.push(format!("≈ {} SU", su_text(est)));
        }
        if let Some(left) = left {
            meta.push(format!("预算剩 {} SU", su_text(left)));
        }
        let mut it = item(
            InboxKind::Approve,
            &r.updated_at,
            format!("{} 待签字", r.id),
            meta.join(" · "),
        );
        it.idea = Some(r.idea.clone());
        it.run = Some(r.id.clone());
        it.command = Some(format!("arb approve {}", r.id));
        out.push(it);
    }
    let seen = last_human_note(conn)?;
    let open: Vec<&str> = b
        .ideas
        .iter()
        .filter(|i| i.stage == IdeaStage::Open.as_str() || i.stage == IdeaStage::Seed.as_str())
        .map(|i| i.name.as_str())
        .collect();
    for r in &b.runs {
        let Some(ended) = &r.finished_at else {
            continue;
        };
        if !open.contains(&r.idea.as_str()) {
            continue;
        }
        if seen.get(&r.idea).is_some_and(|s| ended <= s) {
            continue;
        }
        let ok = r.status == RunStatus::Succeeded.as_str();
        let (kind, word) = if ok {
            (InboxKind::Finished, "跑完了")
        } else if r.status == RunStatus::Cancelled.as_str() {
            (InboxKind::Failed, "已取消")
        } else {
            (InboxKind::Failed, "失败")
        };
        let mut meta = vec![r.idea.clone(), clip(&r.change_reason, 40)];
        if let Some(s) = &r.summary {
            meta.push(clip(s, 60));
        }
        meta.push(format!(
            "{} {} SU",
            if ok { "花了" } else { "用了" },
            su_text(r.billed_su)
        ));
        let mut it = item(kind, ended, format!("{} {word}", r.id), meta.join(" · "));
        it.idea = Some(r.idea.clone());
        it.run = Some(r.id.clone());
        out.push(it);
    }
    let since = days_before(&b.project.loaded_at, AGENT_DONE_DAYS);
    for t in &b.todos {
        let Some(done) = &t.done_at else { continue };
        if t.status != TaskStatus::Done.as_str() || *done < since {
            continue;
        }
        if !open.contains(&t.idea.as_str()) || seen.get(&t.idea).is_some_and(|s| done <= s) {
            continue;
        }
        let what = match t.kind.as_str() {
            "command_new" | "command_revise" => "命令",
            "inspect" => "读代码",
            "probe_new" | "probe_revise" => "probe",
            _ => "待办",
        };
        let mut it = item(
            InboxKind::AgentDone,
            done,
            format!("agent 写好了{what}（{}）", t.id),
            format!(
                "{} · {}{}",
                t.idea,
                clip(&t.text, 40),
                t.result
                    .as_deref()
                    .map(|r| format!(" · {}", clip(r, 60)))
                    .unwrap_or_default()
            ),
        );
        it.idea = Some(t.idea.clone());
        it.task = Some(t.id.clone());
        out.push(it);
    }
    for l in &b.lessons {
        let mut it = item(
            InboxKind::Lesson,
            &l.created_at,
            format!("经验待确认（{}）", l.id),
            clip(&l.text, 80),
        );
        it.lesson = Some(l.id.clone());
        it.command = Some(format!("arb lesson accept {}", l.id));
        out.push(it);
    }
    if b.project.kind == Place::Project {
        for f in probe::discover(Some(&ctx.state_dir()))? {
            if probe::state(conn, &f)? == probe::ProbeState::NeedsReenable {
                let mut it = item(
                    InboxKind::ProbeReenable,
                    &b.project.loaded_at,
                    format!("probe {} 待重新启用", f.key.name),
                    format!("{} 在启用之后改过了，改过的文件不会运行", f.path.display()),
                );
                it.probe = Some(f.key.name.clone());
                it.command = Some(format!("arb probe enable {}", f.key.name));
                out.push(it);
            }
        }
    }
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| b.when.cmp(&a.when)));
    Ok(out)
}

fn notifications(state_dir: &Path) -> Vec<NotificationView> {
    let path = state_dir.join(crate::monitor::NOTIFICATIONS);
    let Ok(mut f) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let size = f.seek(SeekFrom::End(0)).unwrap_or(0);
    let start = size.saturating_sub(NOTIFICATIONS_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // probably cut in the middle
    }
    let skip = lines.len().saturating_sub(NOTIFICATIONS_SHOWN);
    lines[skip..]
        .iter()
        .rev()
        .map(|line| match line.split_once(' ') {
            Some((stamp, rest)) if parse_utc(stamp).is_some() => NotificationView {
                at: Some(stamp.to_string()),
                text: rest.to_string(),
            },
            _ => NotificationView {
                at: None,
                text: (*line).to_string(),
            },
        })
        .collect()
}

/// Every listed project, then the seed home (when it exists): each one's inbox, or the
/// error that kept it from being read.
pub fn projects() -> Result<Vec<ProjectSummary>> {
    let mut roots: Vec<PathBuf> = crate::registry::list()?
        .into_iter()
        .map(|e| e.path)
        .collect();
    let home = crate::paths::home();
    if home.initialized() {
        roots.push(home.root.clone());
    }
    Ok(roots.iter().map(|r| summary(r)).collect())
}

/// [`board`] reduced to one row of the project list.
pub fn summary(root: &Path) -> ProjectSummary {
    match board(root) {
        Ok(b) => ProjectSummary {
            open_ideas: b.ideas.iter().filter(|i| i.stage == "open").count() as u32,
            seeds: b.ideas.iter().filter(|i| i.stage == "seed").count() as u32,
            root: b.project.root,
            name: b.project.name,
            kind: b.project.kind,
            error: None,
            inbox: b.inbox,
        },
        Err(e) => ProjectSummary {
            root: root.to_path_buf(),
            name: root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            kind: if is_home(root) {
                Place::Home
            } else {
                Place::Project
            },
            error: Some(e.to_string()),
            open_ideas: 0,
            seeds: 0,
            inbox: Vec::new(),
        },
    }
}
