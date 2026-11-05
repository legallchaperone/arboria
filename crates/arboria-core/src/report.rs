//! Read-side views: `arb status` (full / `--brief`, also the hook's text), the home's
//! status, and the Markdown export. Ports `arboria.report`.
//!
//! Invariants:
//! - Pure reads; never writes the ledger.
//! - "Needs you" counts use one rule everywhere (the hook, `arb status`, the app's inbox):
//!   pending approvals and [`FINISHED_UNSEEN_SQL`] (runs that finished after the last
//!   human note of their idea).
//! - Text from the ledger is the human's or length-checked LLM text; the brief form is
//!   capped by the hook ([`crate::hook::MAX_CONTEXT_CHARS`]).
//! - v8 adds a line for probes waiting to be re-enabled ("待重新启用", ADR 0003).

use serde::Serialize;
use serde_json::{Map, Value};

use crate::claudemd::{accepted_lessons, lesson_line};
use crate::db::{now, row_map, Ledger};
use crate::ideas::{self, SeedState};
use crate::model::Lesson;
use crate::paths::{Home, RepoContext};
use crate::records::py_isspace;
use crate::{tasks, Result};

/// The rules line appended to the brief status (the hook's context).
pub const BRIEF_FOOTER: &str = concat!(
    "Ledger rules: experiment state lives in the arboria ledger, written only by `arb`. ",
    "Work comes from the todos: finish one with `arb todo done tNNN --result \"...\"`; ",
    "for a command todo read `arb commands guide` first, for an inspect todo ",
    "`arb inspect guide` (deliver with `arb inspect done`, not `arb todo done`); ",
    "`notebook.py` is the human's notebook: read it, never edit it; `arb note` is human-only. ",
    "Use `arb status`, `arb run summary RUN \"...\"`, ",
    "`arb submit --reason \"...\" --gpus N --time T [-- CMD]`; never edit .arboria/ by hand. ",
    "Approval happens only in the human's own terminal: you cannot run `arb approve` or ",
    "`arb lesson accept`. Read cluster logs with `arb logs RUN` / `arb remote ls RUN`, not ssh. ",
    "Propose reusable pitfalls with `arb lesson propose \"...\"`."
);
/// Section headers of the todo lists.
pub const TODOS_HEADER: &str = "来自 notebook 的待办：";
/// See [`TODOS_HEADER`].
pub const COMMAND_TODOS_HEADER: &str = "命令待办：";
/// See [`TODOS_HEADER`].
pub const INSPECT_TODOS_HEADER: &str = "读代码待办：";
/// Accepted lessons shown in status / the hook.
pub const STATUS_LESSONS: usize = 10;
/// Shown when something waits for the human.
pub const BOARD_LINE: &str = "看板：arb ui";
/// Count of finished runs of open ideas that ended (latest `run_final` event, else
/// `updated_at`) after the idea's last human note (notes without an author predate v4
/// and were the human's). Python `report.FINISHED_UNSEEN_SQL`, verbatim; the dashboard
/// and the app's inbox use the same rule.
pub const FINISHED_UNSEEN_SQL: &str = concat!(
    "SELECT COUNT(*) AS n FROM runs r JOIN ideas i ON i.id = r.idea_id",
    " WHERE i.status = 'open' AND r.status IN ('succeeded', 'failed', 'cancelled')",
    " AND COALESCE((SELECT MAX(e.ts) FROM events e WHERE e.kind = 'run_final'",
    "               AND e.subject = r.id), r.updated_at)",
    "   > COALESCE((SELECT MAX(n.created_at) FROM notes n WHERE n.idea_id = r.idea_id",
    "               AND (n.author IS NULL OR n.author = 'human')), '')"
);

/// The seed home's rules line (the hook's context there).
pub const HOME_FOOTER: &str = concat!(
    "Seed home rules: this folder is the human's seed garden (ideas without a repo yet). ",
    "`seeds/<name>.py` are the human's marimo notebooks: read them for context, never edit ",
    "them; never edit .arboria/. Here (outside any repo) `arb idea` works on the seeds: read ",
    "them with `arb idea list` / `arb idea show NAME`; seeds are created, changed, dropped ",
    "and planted only by the human (`arb idea new/set/drop/revive/plant`). ",
    "Work comes from the todos: `arb todo list`, finish with `arb todo done tNNN --result \"...\"`; ",
    "a command todo here is a global command (read `arb commands guide` first)."
);
/// Seeds shown in the brief home status.
pub const HOME_SHOWN: usize = 5;
/// `inspection.VIEW_LABELS`.
pub const VIEW_LABELS: &[(&str, &str)] = &[
    ("code", "代码摘录"),
    ("pseudo", "伪代码"),
    ("simple", "简化代码"),
    ("map", "架构图"),
];

/// What `arb status --json` prints (Python `status_data`): `scope` = `idea` | `overview`
/// with the keys of that scope, plus lessons, `pending_approvals`, `finished_unseen`.
/// Kept as JSON (ledger rows as Python reads them) until the app's views settle its shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusData(pub Value);

type Row = Map<String, Value>;

fn rows(ledger: &Ledger, sql: &str, params: impl rusqlite::Params) -> Result<Vec<Row>> {
    let mut stmt = ledger.conn().prepare(sql)?;
    let mut q = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = q.next()? {
        out.push(row_map(r)?);
    }
    Ok(out)
}

fn count(ledger: &Ledger, sql: &str) -> Result<i64> {
    Ok(ledger.conn().query_row(sql, [], |r| r.get(0))?)
}

/// Runs awaiting the human's approval.
pub fn pending_approvals(ledger: &Ledger) -> Result<u32> {
    Ok(count(
        ledger,
        "SELECT COUNT(*) FROM runs WHERE status = 'awaiting_approval'",
    )? as u32)
}

/// See [`FINISHED_UNSEEN_SQL`].
pub fn finished_unseen(ledger: &Ledger) -> Result<u32> {
    Ok(count(ledger, FINISHED_UNSEEN_SQL)? as u32)
}

fn open_decisions(ledger: &Ledger, idea_id: Option<i64>) -> Result<Vec<Row>> {
    match idea_id {
        None => rows(
            ledger,
            "SELECT * FROM decisions WHERE choice IS NULL ORDER BY id",
            [],
        ),
        Some(id) => rows(
            ledger,
            "SELECT * FROM decisions WHERE choice IS NULL AND (\
             (subject_type = 'idea' AND subject_id IN (?1, (SELECT name FROM ideas WHERE id = ?2)))\
             OR (subject_type = 'run' AND subject_id IN (SELECT id FROM runs WHERE idea_id = ?2))\
             ) ORDER BY id",
            rusqlite::params![id.to_string(), id],
        ),
    }
}

fn jobs(ledger: &Ledger, run_id: &str) -> Result<Vec<Row>> {
    rows(
        ledger,
        "SELECT segment, job_name, job_id, state, elapsed_s, billed_su FROM jobs \
         WHERE run_id = ?1 ORDER BY segment",
        [run_id],
    )
}

fn runs(ledger: &Ledger, idea_id: i64) -> Result<Vec<Row>> {
    let mut out = rows(
        ledger,
        "SELECT id, status, change_reason, snapshot_commit, tree_hash, summary, wandb_url, \
         machine, \"partition\", gpus, walltime, est_cost, actual_cost, \
         created_at, updated_at FROM runs WHERE idea_id = ?1 ORDER BY id",
        [idea_id],
    )?;
    for r in &mut out {
        let id = s(r, "id").to_string();
        r.insert(
            "jobs".into(),
            Value::from(
                jobs(ledger, &id)?
                    .into_iter()
                    .map(Value::Object)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    Ok(out)
}

fn notes(ledger: &Ledger, idea_id: i64, limit: Option<usize>) -> Result<Vec<Row>> {
    let mut sql = String::from("SELECT * FROM notes WHERE idea_id = ?1 ORDER BY id DESC");
    if let Some(n) = limit {
        sql.push_str(&format!(" LIMIT {n}"));
    }
    rows(ledger, &sql, [idea_id])
}

const IDEA_KEYS: &[&str] = &[
    "name",
    "hypothesis",
    "metric",
    "confidence",
    "status",
    "branch",
    "worktree_path",
    "created_at",
    "closed_at",
];

fn idea_public(idea: &Row) -> Row {
    IDEA_KEYS
        .iter()
        .map(|k| (k.to_string(), idea.get(*k).cloned().unwrap_or(Value::Null)))
        .collect()
}

fn objects(v: Vec<Row>) -> Value {
    Value::Array(v.into_iter().map(Value::Object).collect())
}

fn lesson_data(ledger: &Ledger) -> Result<Row> {
    let total = count(
        ledger,
        "SELECT COUNT(*) FROM lessons WHERE status = 'accepted'",
    )?;
    let proposed = count(
        ledger,
        "SELECT COUNT(*) FROM lessons WHERE status = 'proposed'",
    )?;
    let lessons = accepted_lessons(ledger, Some(STATUS_LESSONS))?;
    let mut m = Row::new();
    m.insert("lessons".into(), serde_json::to_value(lessons)?);
    m.insert("accepted_lessons".into(), total.into());
    m.insert("proposed_lessons".into(), proposed.into());
    Ok(m)
}

fn task_values(ledger: &Ledger, owner: Option<crate::model::IdeaId>) -> Result<Value> {
    Ok(serde_json::to_value(tasks::list_tasks(
        ledger, owner, false,
    )?)?)
}

/// `arb status --json`.
pub fn status_data(ctx: &RepoContext, ledger: &Ledger) -> Result<StatusData> {
    let mut data = scope_data(ctx, ledger)?;
    data.extend(lesson_data(ledger)?);
    data.insert(
        "pending_approvals".into(),
        pending_approvals(ledger)?.into(),
    );
    data.insert("finished_unseen".into(), finished_unseen(ledger)?.into());
    let stale = probes_to_reenable(ctx, ledger)?;
    if !stale.is_empty() {
        // v8 (ADR 0003); absent otherwise, so the JSON stays Python's
        data.insert("probes_to_reenable".into(), stale.into());
    }
    Ok(StatusData(Value::Object(data)))
}

/// Probes enabled once whose file changed since ("待重新启用").
pub fn probes_to_reenable(ctx: &RepoContext, ledger: &Ledger) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for f in crate::probe::discover(Some(&ctx.state_dir()))? {
        if crate::probe::state(ledger.conn(), &f)? == crate::probe::ProbeState::NeedsReenable {
            out.push(f.key.name.clone());
        }
    }
    Ok(out)
}

fn scope_data(ctx: &RepoContext, ledger: &Ledger) -> Result<Row> {
    let mut m = Row::new();
    if let Some(idea) = ideas::current_idea(ctx, ledger)? {
        let id = idea.id.0;
        let row = rows(ledger, "SELECT * FROM ideas WHERE id = ?1", [id])?
            .pop()
            .expect("the current idea");
        let parent = match idea.parent_id {
            Some(p) => rows(ledger, "SELECT name FROM ideas WHERE id = ?1", [p.0])?
                .pop()
                .and_then(|r| r.get("name").cloned()),
            None => None,
        };
        let mut public = idea_public(&row);
        public.insert("parent".into(), parent.unwrap_or(Value::Null));
        m.insert("scope".into(), "idea".into());
        m.insert("idea".into(), Value::Object(public));
        m.insert("runs".into(), objects(runs(ledger, id)?));
        m.insert("notes".into(), objects(notes(ledger, id, Some(5))?));
        m.insert("todos".into(), task_values(ledger, Some(idea.id))?);
        m.insert(
            "open_decisions".into(),
            objects(open_decisions(ledger, Some(id))?),
        );
        return Ok(m);
    }
    // seeds (stage `seed`) never live in a repo's ledger; `stage` keeps them out anyway
    let ideas = rows(
        ledger,
        "SELECT i.*, (SELECT COUNT(*) FROM runs r WHERE r.idea_id = i.id) AS run_count, \
         (SELECT text FROM notes n WHERE n.idea_id = i.id ORDER BY n.id DESC LIMIT 1) AS last_note \
         FROM ideas i WHERE i.stage = 'open' ORDER BY i.id DESC",
        [],
    )?;
    let closed = count(ledger, "SELECT COUNT(*) FROM ideas WHERE status != 'open'")?;
    let ideas: Vec<Row> = ideas
        .iter()
        .map(|i| {
            let mut p = idea_public(i);
            p.insert("run_count".into(), i["run_count"].clone());
            p.insert("last_note".into(), i["last_note"].clone());
            p
        })
        .collect();
    m.insert("scope".into(), "overview".into());
    m.insert(
        "main_worktree".into(),
        ctx.main.display().to_string().into(),
    );
    m.insert("ideas".into(), objects(ideas));
    m.insert("closed_ideas".into(), closed.into());
    m.insert("todos".into(), task_values(ledger, None)?);
    m.insert(
        "open_decisions".into(),
        objects(open_decisions(ledger, None)?),
    );
    Ok(m)
}

// ---------------------------------------------------------------- text

/// A string field ("" when missing or null).
fn s<'a>(m: &'a Row, k: &str) -> &'a str {
    m.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Python truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// Python `str()` of a JSON value from the ledger.
fn py_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".into(),
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => crate::canonical::python_float_repr(n.as_f64().unwrap_or(0.0)),
        },
        Some(v) => v.to_string(),
    }
}

fn prefix(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `" ".join(text.split())`, cut to `n` code points with `…`.
pub fn clip(text: Option<&str>, n: usize) -> String {
    let joined = text
        .unwrap_or("")
        .split(py_isspace)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.chars().count() <= n {
        joined
    } else {
        prefix(&joined, n - 1) + "…"
    }
}

fn decision_lines(decisions: &[Value]) -> Vec<String> {
    decisions
        .iter()
        .filter_map(Value::as_object)
        .map(|d| {
            format!(
                "- {} ({} {}): {}",
                s(d, "id"),
                s(d, "subject_type"),
                s(d, "subject_id"),
                clip(d.get("question").and_then(Value::as_str), 160)
            )
        })
        .collect()
}

/// `s1 1001 completed, s2 1002 running; 3.20 SU` (None for runs never submitted).
pub fn jobs_line(run: &Row) -> Option<String> {
    let jobs: Vec<&Row> = run
        .get("jobs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    let est = run.get("est_cost").and_then(Value::as_f64);
    if jobs.is_empty() && est.is_none() {
        return None;
    }
    let parts: Vec<String> = jobs
        .iter()
        .map(|j| {
            let id = j.get("job_id").filter(|v| truthy(Some(v)));
            format!(
                "s{} {} {}",
                py_str(j.get("segment")),
                id.map_or("-".to_string(), |v| py_str(Some(v))),
                py_str(j.get("state"))
            )
        })
        .collect();
    let mut money = match run.get("actual_cost").and_then(Value::as_f64) {
        Some(c) => format!("{c:.2} SU"),
        None => "0 SU".into(),
    };
    if let Some(e) = est {
        money.push_str(&format!("（估计上限 {e:.2}）"));
    }
    let where_ = if truthy(run.get("machine")) {
        format!(
            "{}/{} {} GPU {}",
            py_str(run.get("machine")),
            py_str(run.get("partition")),
            py_str(run.get("gpus")),
            py_str(run.get("walltime"))
        )
    } else {
        String::new()
    };
    let joined = parts.join(", ");
    Some(
        [where_.as_str(), joined.as_str(), money.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn owner_name(t: &Row) -> String {
    t.get("idea")
        .and_then(Value::as_str)
        .or_else(|| t.get("seed").and_then(Value::as_str))
        .unwrap_or("?")
        .to_string()
}

fn task_payload(t: &Row) -> Row {
    match t.get("payload_json").and_then(Value::as_str) {
        Some(raw) if !raw.is_empty() => serde_json::from_str(raw).unwrap_or_default(),
        _ => Row::new(),
    }
}

/// Open todos the human left in a notebook (the work the agent should pick up).
pub fn todo_lines(todos: &[Value], brief: bool, with_idea: bool) -> Vec<String> {
    let todos: Vec<&Row> = todos.iter().filter_map(Value::as_object).collect();
    let kind = |t: &Row| {
        t.get("kind")
            .and_then(Value::as_str)
            .unwrap_or("plain")
            .to_string()
    };
    let mut lines = Vec::new();
    let cmds: Vec<&&Row> = todos
        .iter()
        .filter(|t| matches!(kind(t).as_str(), "command_new" | "command_revise"))
        .collect();
    if !cmds.is_empty() {
        let items: Vec<String> = cmds
            .iter()
            .map(|t| {
                let p = task_payload(t);
                let name = p.get("name").map_or("?".to_string(), |v| py_str(Some(v)));
                let extra = if kind(t) == "command_revise" {
                    "，修改"
                } else {
                    ""
                };
                let where_ = if with_idea {
                    format!("，{}", owner_name(t))
                } else {
                    String::new()
                };
                format!("{name}（{}{extra}{where_}）", s(t, "id"))
            })
            .collect();
        lines.push(format!(
            "{COMMAND_TODOS_HEADER}{}——按 `arb commands guide` 的约定写，详情见 `arb todo list --json`",
            items.join("、")
        ));
    }
    let reads: Vec<&&Row> = todos.iter().filter(|t| kind(t) == "inspect").collect();
    if !reads.is_empty() {
        let items: Vec<String> = reads
            .iter()
            .map(|t| {
                let p = task_payload(t);
                let view = p
                    .get("view")
                    .and_then(Value::as_str)
                    .and_then(|v| VIEW_LABELS.iter().find(|(k, _)| *k == v))
                    .map_or("?", |(_, label)| label);
                let where_ = if with_idea {
                    format!("，{}", owner_name(t))
                } else {
                    String::new()
                };
                format!(
                    "{}（{}，{view}{where_}）",
                    clip(p.get("question").and_then(Value::as_str), 60),
                    s(t, "id")
                )
            })
            .collect();
        lines.push(format!(
            "{INSPECT_TODOS_HEADER}{}——按 `arb inspect guide` 的约定读代码，用 `arb inspect done` 交付",
            items.join("、")
        ));
    }
    let plain: Vec<&&Row> = todos.iter().filter(|t| kind(t) == "plain").collect();
    if !plain.is_empty() {
        lines.push(format!(
            "{TODOS_HEADER}（做完用 `arb todo done tNNN --result \"...\"`）"
        ));
        for t in plain {
            let where_ = if with_idea {
                format!(" [{}]", owner_name(t))
            } else {
                String::new()
            };
            lines.push(format!(
                "- {}{where_}: {}",
                s(t, "id"),
                clip(
                    t.get("text").and_then(Value::as_str),
                    if brief { 300 } else { 10_000 }
                )
            ));
        }
    }
    lines
}

fn lessons_of(data: &Row) -> Vec<Lesson> {
    data.get("lessons")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Accepted lessons (newest first) and the count awaiting the human.
pub fn lesson_lines(data: &Row) -> Vec<String> {
    let mut lines = Vec::new();
    let lessons = lessons_of(data);
    if !lessons.is_empty() {
        lines.push(format!("经验（已确认，最新 {} 条）:", lessons.len()));
        lines.extend(lessons.iter().map(lesson_line));
        let total = data
            .get("accepted_lessons")
            .and_then(Value::as_i64)
            .unwrap_or(lessons.len() as i64);
        let more = total - lessons.len() as i64;
        if more > 0 {
            lines.push(format!("  … 另有 {more} 条，见 `arb lesson list`"));
        }
    }
    let pending = data
        .get("proposed_lessons")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if pending != 0 {
        lines.push(format!(
            "待你确认的经验：{pending} 条，用 `arb lesson list` 查看"
        ));
    }
    lines
}

/// `arb status` text (`brief`: the hook's compact form with [`BRIEF_FOOTER`]).
pub fn format_status(data: &StatusData, brief: bool) -> String {
    let (head, tail) = status_parts(data, brief);
    [head, tail]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn arr<'a>(m: &'a Row, k: &str) -> Vec<&'a Value> {
    m.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// `(state, lessons + footer)`: the hook truncates the first part, never the second.
pub fn status_parts(data: &StatusData, brief: bool) -> (String, String) {
    let empty = Row::new();
    let data = data.0.as_object().unwrap_or(&empty);
    let clipn = if brief { 160 } else { 10_000 };
    let long = |b: bool| if b { 240 } else { 10_000 };
    let mut lines: Vec<String> = Vec::new();
    let todos: Vec<Value> = arr(data, "todos").into_iter().cloned().collect();
    if s(data, "scope") == "idea" {
        let idea = data
            .get("idea")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let parent = if truthy(idea.get("parent")) {
            format!(", parent {}", py_str(idea.get("parent")))
        } else {
            String::new()
        };
        lines.push(format!(
            "[arboria] idea `{}` — status {}, confidence {}, branch {}{parent}",
            py_str(idea.get("name")),
            py_str(idea.get("status")),
            py_str(idea.get("confidence")),
            py_str(idea.get("branch"))
        ));
        if !brief {
            lines.push(format!("worktree: {}", py_str(idea.get("worktree_path"))));
        }
        lines.push(format!(
            "Hypothesis: {}",
            clip(idea.get("hypothesis").and_then(Value::as_str), clipn)
        ));
        lines.push(format!(
            "Metric: {}",
            clip(idea.get("metric").and_then(Value::as_str), clipn)
        ));
        lines.extend(todo_lines(&todos, brief, false));
        let runs: Vec<&Row> = arr(data, "runs")
            .into_iter()
            .filter_map(Value::as_object)
            .collect();
        let shown: &[&Row] = if brief && runs.len() > 8 {
            &runs[runs.len() - 8..]
        } else {
            &runs
        };
        lines.push(format!(
            "Runs ({}){}",
            runs.len(),
            if runs.is_empty() { ": none" } else { ":" }
        ));
        if brief && runs.len() > shown.len() {
            lines.push(format!(
                "  … {} older runs omitted",
                runs.len() - shown.len()
            ));
        }
        for r in shown {
            lines.push(format!(
                "- {} [{}] {} @{}",
                s(r, "id"),
                s(r, "status"),
                clip(r.get("change_reason").and_then(Value::as_str), clipn),
                prefix(s(r, "snapshot_commit"), 10)
            ));
            if let Some(jl) = jobs_line(r) {
                lines.push(format!("    jobs: {jl}"));
            }
            if truthy(r.get("summary")) {
                lines.push(format!(
                    "    summary: {}",
                    clip(r.get("summary").and_then(Value::as_str), long(brief))
                ));
            }
        }
        let notes: Vec<&Row> = arr(data, "notes")
            .into_iter()
            .filter_map(Value::as_object)
            .collect();
        lines.push(format!(
            "Recent notes{}",
            if notes.is_empty() { ": none" } else { ":" }
        ));
        for n in notes {
            let mut extra = String::new();
            if truthy(n.get("run_id")) {
                extra.push_str(&format!(" (run {})", py_str(n.get("run_id"))));
            }
            if truthy(n.get("confidence")) {
                extra.push_str(&format!(" [confidence→{}]", py_str(n.get("confidence"))));
            }
            lines.push(format!(
                "- {}{extra}: {}",
                prefix(s(n, "created_at"), 10),
                clip(n.get("text").and_then(Value::as_str), long(brief))
            ));
        }
    } else {
        let ideas: Vec<&Row> = arr(data, "ideas")
            .into_iter()
            .filter_map(Value::as_object)
            .collect();
        lines.push(format!(
            "[arboria] main worktree {} — {} open idea(s), {} closed",
            s(data, "main_worktree"),
            ideas.len(),
            py_str(data.get("closed_ideas"))
        ));
        for i in &ideas {
            lines.push(format!(
                "- {} [{}] runs: {} — {}",
                s(i, "name"),
                py_str(i.get("confidence")),
                py_str(i.get("run_count")),
                clip(i.get("hypothesis").and_then(Value::as_str), clipn)
            ));
            if !brief {
                lines.push(format!("    worktree: {}", py_str(i.get("worktree_path"))));
            }
            if truthy(i.get("last_note")) {
                lines.push(format!(
                    "    last note: {}",
                    clip(i.get("last_note").and_then(Value::as_str), clipn)
                ));
            }
        }
        if ideas.is_empty() {
            lines.push(
                "No open ideas. Start one with `arb idea new NAME --hypothesis ... --metric ...`."
                    .into(),
            );
        }
        lines.extend(todo_lines(&todos, brief, true));
    }
    let decisions: Vec<Value> = arr(data, "open_decisions").into_iter().cloned().collect();
    lines.push(format!(
        "Open decisions{}",
        if decisions.is_empty() { ": none" } else { ":" }
    ));
    lines.extend(decision_lines(&decisions));
    let mut tail = lesson_lines(data);
    let pending = data
        .get("pending_approvals")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if pending != 0 {
        tail.insert(
            0,
            format!("待你批准的 run：{pending} 个（`arb card RUN` 看审核单，由你运行 `arb approve RUN`）"),
        );
    }
    let stale: Vec<&str> = arr(data, "probes_to_reenable")
        .into_iter()
        .filter_map(Value::as_str)
        .collect();
    if !stale.is_empty() {
        tail.push(format!(
            "probe 待重新启用（文件改过，停用了）：{}——由你运行 `arb probe enable NAME`",
            stale.join("、")
        ));
    }
    if pending != 0
        || !stale.is_empty()
        || truthy(data.get("proposed_lessons"))
        || truthy(data.get("finished_unseen"))
    {
        tail.push(BOARD_LINE.into()); // something waits for the human
    }
    if brief {
        tail.push(BRIEF_FOOTER.into());
    }
    (lines.join("\n"), tail.join("\n"))
}

/// `~/x` for a path under the home directory, else the path (`notebooks.display_path`).
pub fn display_path(path: &std::path::Path) -> String {
    match std::env::home_dir().and_then(|h| path.strip_prefix(h).ok().map(|p| p.to_path_buf())) {
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

/// `arb status` in the seed home: counts, the most recently edited seeds, open todos.
pub fn home_status(home: &Home, ledger: &Ledger, brief: bool) -> Result<String> {
    let all = ideas::list_seeds(home, ledger, true)?;
    let n = |st: SeedState| all.iter().filter(|s| s.seed_status == st).count();
    let seeds: Vec<_> = all
        .iter()
        .filter(|s| s.seed_status != SeedState::Dropped)
        .collect();
    let mut lines = vec![format!(
        "[arboria] 种子园 {} — {} 个在思考中，{} 个已种下，{} 个已放弃",
        display_path(&home.root),
        n(SeedState::Thinking),
        n(SeedState::Planted),
        n(SeedState::Dropped)
    )];
    let shown = if brief {
        &seeds[..seeds.len().min(HOME_SHOWN)]
    } else {
        &seeds[..]
    };
    if !shown.is_empty() {
        lines.push(
            if brief {
                "最近编辑的种子："
            } else {
                "种子（最近编辑的在前）："
            }
            .into(),
        );
    }
    for sv in shown {
        let planted: String = sv
            .planted_to
            .iter()
            .map(|p| format!(" → {}", p.idea))
            .collect();
        let planted = if planted.is_empty() {
            String::new()
        } else {
            format!("，已种下{planted}")
        };
        let gist = match sv.idea.gist.as_deref().filter(|g| !g.is_empty()) {
            Some(g) => format!("：{}", clip(Some(g), if brief { 160 } else { 10_000 })),
            None => String::new(),
        };
        lines.push(format!(
            "- {}（{}{planted}）{gist}",
            sv.idea.name,
            prefix(&sv.last_edited, 10)
        ));
        if let Some(h) = sv
            .idea
            .hypothesis
            .as_deref()
            .filter(|h| !brief && !h.is_empty())
        {
            lines.push(format!("    假设：{h}"));
        }
    }
    if brief && seeds.len() > shown.len() {
        lines.push(format!(
            "  … 另有 {} 个，见 `arb idea list`",
            seeds.len() - shown.len()
        ));
    }
    if seeds.is_empty() {
        lines.push(
            "还没有种子（人在仓库外用 `arb idea new NAME \"一句话想法\"` 记下第一颗）".into(),
        );
    }
    let todos = task_values(ledger, None)?;
    lines.extend(todo_lines(
        todos.as_array().map_or(&[][..], |v| v),
        brief,
        true,
    ));
    if brief {
        lines.push(HOME_FOOTER.into());
    }
    Ok(lines.join("\n"))
}

/// `arb export`: Markdown of all ideas, notes and runs, newest first.
pub fn export_markdown(ctx: &RepoContext, ledger: &Ledger) -> Result<String> {
    let ideas = rows(
        ledger,
        "SELECT i.*, p.name AS parent_name FROM ideas i LEFT JOIN ideas p ON p.id = i.parent_id \
         ORDER BY i.created_at DESC, i.id DESC",
        [],
    )?;
    let name = ctx
        .main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out: Vec<String> = vec![
        format!("# Arboria research log: {name}"),
        String::new(),
        format!(
            "_Exported {} — {} idea(s), newest first._",
            now(),
            ideas.len()
        ),
        String::new(),
        "## 经验".into(),
        String::new(),
    ];
    let accepted = accepted_lessons(ledger, None)?;
    if accepted.is_empty() {
        out.push("_暂无_".into());
    }
    out.extend(accepted.iter().map(lesson_line));
    let pending = rows(
        ledger,
        "SELECT * FROM lessons WHERE status = 'proposed' ORDER BY id DESC",
        [],
    )?;
    if !pending.is_empty() {
        out.push(String::new());
        out.push("待确认：".into());
        out.extend(
            pending
                .iter()
                .map(|l| format!("- {}: {}", s(l, "id"), s(l, "text"))),
        );
    }
    out.push(String::new());
    for idea in &ideas {
        let id = idea.get("id").and_then(Value::as_i64).unwrap_or(0);
        out.push(format!(
            "## {} — {}, confidence: {}",
            s(idea, "name"),
            s(idea, "status"),
            s(idea, "confidence")
        ));
        out.push(String::new());
        out.push(format!(
            "- **Hypothesis:** {}",
            py_str(idea.get("hypothesis"))
        ));
        out.push(format!("- **Metric:** {}", py_str(idea.get("metric"))));
        if truthy(idea.get("parent_name")) {
            out.push(format!("- **Parent idea:** {}", s(idea, "parent_name")));
        }
        let mut dates = format!("created {}", s(idea, "created_at"));
        if truthy(idea.get("closed_at")) {
            dates.push_str(&format!(", closed {}", s(idea, "closed_at")));
        }
        out.push(format!(
            "- **Branch:** `{}` ({dates})",
            py_str(idea.get("branch"))
        ));
        out.push(String::new());
        out.push("### Notes".into());
        let ns = notes(ledger, id, None)?;
        if ns.is_empty() {
            out.push("_none_".into());
        }
        for n in &ns {
            let mut tags = Vec::new();
            if truthy(n.get("run_id")) {
                tags.push(format!("run {}", s(n, "run_id")));
            }
            if truthy(n.get("confidence")) {
                tags.push(format!("confidence → {}", s(n, "confidence")));
            }
            let tag = if tags.is_empty() {
                String::new()
            } else {
                format!(" ({})", tags.join(", "))
            };
            out.push(format!("- {}{tag}: {}", s(n, "created_at"), s(n, "text")));
        }
        out.push(String::new());
        out.push("### Runs".into());
        let rs = rows(
            ledger,
            "SELECT * FROM runs WHERE idea_id = ?1 ORDER BY id DESC",
            [id],
        )?;
        if rs.is_empty() {
            out.push("_none_".into());
        }
        for r in &rs {
            out.push(format!(
                "- **{}** [{}] {}, snapshot `{}` — {}",
                s(r, "id"),
                s(r, "status"),
                s(r, "created_at"),
                prefix(s(r, "snapshot_commit"), 10),
                s(r, "change_reason")
            ));
            let mut with_jobs = r.clone();
            with_jobs.insert("jobs".into(), objects(jobs(ledger, s(r, "id"))?));
            if let Some(jl) = jobs_line(&with_jobs) {
                out.push(format!("  - jobs: {jl}"));
            }
            if truthy(r.get("summary")) {
                for line in crate::records::py_splitlines(s(r, "summary")) {
                    out.push(format!("  > {line}"));
                }
            }
        }
        out.push(String::new());
    }
    Ok(out.join("\n").trim_end().to_string() + "\n")
}
