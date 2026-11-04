//! 读代码 (`lab.inspect`): the agent explains code for the human; the answer lives in the
//! notebook and is cached. Ports `arboria.inspection` (the ledger, git and validation
//! side; rendering moves to the app, the kernel side stays Python).
//!
//! Invariants:
//! - An answer is bound to the code it describes: a run's snapshot commit, or the idea's
//!   worktree as it is now (uncommitted and untracked files included) snapshotted
//!   read-only with [`crate::snapshot::write_tree`] (a throw-away index; never the user's
//!   branch, index or files). The commit is made only when a todo is created, anchored at
//!   `refs/arboria/inspect/<todo>`. A changed tree shows the answer as stale ("代码已变");
//!   nothing is regenerated automatically; `refresh` asks again only when it changed (or
//!   the last todo was dropped).
//! - The agent gives only locations and notes. **Source lines are never written by an
//!   LLM**: [`deliver`] extracts them with `git cat-file blob <code_ref>:<path>` (bytes,
//!   `\r` kept) and stores them in `result_json`.
//! - Limits ([`validate_result`]): title ≤80, summary and span notes ≤600, line notes
//!   ≤200 (≤40), pseudocode / simplified code ≤8000, Mermaid ≤8000, ≤6 spans, ≤400
//!   source lines, files ≤2 MiB. Mermaid: the first line is a diagram type; no
//!   `%%{init}%%`, no `click`, no links, no HTML except `<br>`. Display always escapes.
//! - `arb todo done` refuses inspect todos; `inspect_done` triggers a backup commit.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::canonical::{python_dumps, python_repr};
use crate::db::{self, now, Ledger};
use crate::model::{
    EventKind, Idea, IdeaStage, InspectView, Inspection, RunId, TaskId, TaskKind, TaskStatus,
};
use crate::records::{self, py_isspace, py_repr, py_splitlines, py_strip, TextField};
use crate::tasks::{self, TaskView};
use crate::{bail, gitutil, snapshot, Error, Result};

/// `refs/arboria/inspect/<todo>`.
pub const REF_PREFIX: &str = "refs/arboria/inspect/";
/// Spans per answer.
pub const MAX_SPANS: usize = 6;
/// Source lines over all spans.
pub const MAX_LINES: usize = 400;
/// Line notes per answer.
pub const MAX_LINE_NOTES: usize = 40;
/// Nodes of a map.
pub const MAX_NODES: usize = 30;
/// Files larger than this cannot be quoted.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Mermaid diagram types allowed on the first line (Python `MERMAID_KINDS`).
pub const MERMAID_KINDS: &[&str] = &[
    "flowchart",
    "graph",
    "sequenceDiagram",
    "classDiagram",
    "classDiagram-v2",
    "stateDiagram",
    "stateDiagram-v2",
    "erDiagram",
    "journey",
    "gantt",
    "pie",
    "mindmap",
    "timeline",
    "block-beta",
    "architecture-beta",
    "C4Context",
    "C4Container",
    "C4Component",
    "quadrantChart",
    "requirementDiagram",
    "gitGraph",
    "sankey-beta",
    "xychart-beta",
    "packet-beta",
];

/// Short commit ids in messages.
pub const SHORT: usize = 7;

/// The view's label (`代码摘录`, `伪代码`, `简化代码`, `架构图`).
pub fn view_label(view: InspectView) -> &'static str {
    match view {
        InspectView::Code => "代码摘录",
        InspectView::Pseudo => "伪代码",
        InspectView::Simple => "简化代码",
        InspectView::Map => "架构图",
    }
}

/// `refs/arboria/inspect/<todo>`.
pub fn ref_name(task_id: &TaskId) -> String {
    format!("{REF_PREFIX}{task_id}")
}

/// The first [`SHORT`] characters of a commit id.
pub fn short(sha: &str) -> &str {
    sha.get(..SHORT).unwrap_or(sha)
}

/// Python `_num`: the numeric part of `t007` (0 when it is not digits).
fn num(task_id: &str) -> u64 {
    task_id.get(1..).and_then(|d| d.parse().ok()).unwrap_or(0)
}

fn keys(view: InspectView) -> &'static [&'static str] {
    match view {
        InspectView::Code => &["spans", "summary", "title"],
        InspectView::Pseudo => &["spans", "summary", "text", "title"],
        InspectView::Simple => &["lang", "spans", "summary", "text", "title"],
        InspectView::Map => &["mermaid", "nodes", "spans", "summary", "title"],
    }
}

const SPAN_KEYS: &[&str] = &["end", "id", "line_notes", "note", "path", "start"];

/// `^[A-Za-z0-9_-]{1,40}$`.
fn id_ok(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `^[a-z0-9+#-]{1,20}$`.
fn lang_ok(s: &str) -> bool {
    (1..=20).contains(&s.len())
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'+' | b'#' | b'-')
        })
}

// ---------------------------------------------------------------- the code an answer is about

/// The code a question is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Code {
    /// `idea` or a run id.
    pub target: String,
    pub tree: String,
    /// None: the idea's worktree, no commit made yet.
    pub commit: Option<String>,
    pub head: Option<String>,
    /// The idea's worktree (None for a run's snapshot).
    pub worktree: Option<PathBuf>,
}

impl Code {
    /// `当前代码` / `run r003 的快照`.
    pub fn label(&self) -> String {
        if self.target == "idea" {
            "当前代码".into()
        } else {
            format!("run {} 的快照", self.target)
        }
    }
}

/// What `lab.inspect` is about right now (read-only: no commit, no ref).
pub fn current_code(ledger: &Ledger, idea: &Idea, run: Option<&RunId>) -> Result<Code> {
    if let Some(run) = run {
        let row = records::get_run(ledger, run)?;
        return Ok(Code {
            target: row.id.to_string(),
            tree: row.tree_hash,
            commit: Some(row.snapshot_commit),
            head: None,
            worktree: None,
        });
    }
    let worktree = PathBuf::from(idea.worktree_path.as_deref().unwrap_or_default());
    if idea.worktree_path.is_none() || !worktree.is_dir() {
        bail!(
            "idea {} has no worktree ({}); ask about a run instead: lab.inspect(..., run=\"rNNN\")",
            idea.name,
            idea.worktree_path.as_deref().unwrap_or("None")
        );
    }
    let (tree, head, _) = snapshot::write_tree(&worktree, false, None, None, &mut |_| {})?;
    Ok(Code {
        target: "idea".into(),
        tree,
        commit: None,
        head,
        worktree: Some(worktree),
    })
}

/// A commit for `code` (an earlier run's with the same tree when there is one).
fn commit_of(ledger: &Ledger, code: &Code) -> Result<String> {
    if let Some(c) = &code.commit {
        return Ok(c.clone());
    }
    let wt = code
        .worktree
        .as_deref()
        .expect("an idea's code has its worktree");
    match snapshot::find_existing_commit(ledger.conn(), &code.tree, wt)? {
        Some(c) => Ok(c),
        None => snapshot::commit_tree(
            wt,
            &code.tree,
            code.head.as_deref(),
            "arboria inspect snapshot",
        ),
    }
}

// ---------------------------------------------------------------- the notebook side

/// `lab.inspect`'s question: [`TextField::Question`] (≤300), then whitespace runs
/// collapsed to one space (Python `str.split()`).
pub fn validate_question(question: &str) -> Result<String> {
    let q = records::validate_text(TextField::Question, Some(question), true)?.expect("required");
    Ok(q.split(py_isspace)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" "))
}

/// What `lab.inspect` shows.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub question: String,
    pub view: InspectView,
    pub code: Code,
    pub answer: Option<Inspection>,
    /// The open (pending) or dropped todo shown.
    pub todo: Option<TaskView>,
    pub created: bool,
}

/// `fresh` | `stale` | `pending` | `dropped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InspectStatus {
    Fresh,
    Stale,
    Pending,
    Dropped,
}

impl State {
    /// See [`InspectStatus`].
    pub fn status(&self) -> InspectStatus {
        match self.todo.as_ref().map(|t| t.task.status) {
            Some(TaskStatus::Open) => InspectStatus::Pending,
            Some(TaskStatus::Dropped) => InspectStatus::Dropped,
            _ if self.stale() => InspectStatus::Stale,
            _ => InspectStatus::Fresh,
        }
    }

    /// The answer describes another tree than the code now.
    pub fn stale(&self) -> bool {
        self.answer
            .as_ref()
            .is_some_and(|a| a.tree != self.code.tree)
    }

    /// The stored answer, parsed.
    pub fn result(&self) -> Option<Value> {
        self.answer
            .as_ref()
            .and_then(|a| serde_json::from_str(&a.result_json).ok())
    }
}

/// `(newest answer, newest todo of any status)` for this question.
fn latest(
    ledger: &Ledger,
    idea: &Idea,
    question: &str,
    view: InspectView,
    target: &str,
) -> Result<(Option<Inspection>, Option<TaskView>)> {
    let rows: Vec<Inspection> = db::all(
        ledger.conn(),
        "SELECT * FROM inspections WHERE idea_id = ?1 AND question = ?2 AND view = ?3 AND target = ?4",
        rusqlite::params![idea.id.0, question, view.as_str(), target],
    )?;
    // max() keeps the first of equal keys
    let mut answer: Option<Inspection> = None;
    for r in rows {
        if answer
            .as_ref()
            .is_none_or(|a| num(r.task_id.as_str()) > num(a.task_id.as_str()))
        {
            answer = Some(r);
        }
    }
    let mut todo: Option<TaskView> = None;
    for t in tasks::list_tasks(ledger, Some(idea.id), true)? {
        if t.task.kind != TaskKind::Inspect {
            continue;
        }
        let p = tasks::payload(&t.task)?;
        let same = p.get("question").and_then(Value::as_str) == Some(question)
            && p.get("view").and_then(Value::as_str) == Some(view.as_str())
            && p.get("target").and_then(Value::as_str) == Some(target);
        if same
            && todo
                .as_ref()
                .is_none_or(|o| num(t.task.id.as_str()) > num(o.task.id.as_str()))
        {
            todo = Some(t);
        }
    }
    Ok((answer, todo))
}

/// The todo text of a question.
pub fn todo_text(question: &str, view: InspectView, code: &Code, commit: &str) -> String {
    format!(
        "读代码（{}）：{question}\n代码：{} @ {}\n约定见 `arb inspect guide`；用 `arb inspect done tNNN --file …` 交付",
        view_label(view),
        code.label(),
        short(commit)
    )
}

/// Look up the answer, or ask the agent (an `inspect` todo); `create = false` never
/// writes. An answer for exactly this code is shown; an open todo for the question is
/// reused; an answer for older code (or a newer dropped todo) is shown unless
/// `refresh`; otherwise a new todo bound to the code as it is now.
pub fn ask(
    ledger: &mut Ledger,
    idea: &Idea,
    question: &str,
    view: InspectView,
    run: Option<&RunId>,
    refresh: bool,
    create: bool,
) -> Result<State> {
    let question = validate_question(question)?;
    let code = current_code(ledger, idea, run)?;
    let (answer, todo) = latest(ledger, idea, &question, view, &code.target)?;
    let mut state = State {
        question: question.clone(),
        view,
        code,
        answer,
        todo: None,
        created: false,
    };
    if state.answer.is_some() && !state.stale() {
        return Ok(state);
    }
    if let Some(t) = todo.as_ref().filter(|t| t.task.status == TaskStatus::Open) {
        state.todo = Some(t.clone());
        return Ok(state);
    }
    let newer_drop = todo.as_ref().is_some_and(|t| {
        t.task.status == TaskStatus::Dropped
            && state
                .answer
                .as_ref()
                .is_none_or(|a| num(t.task.id.as_str()) > num(a.task_id.as_str()))
    });
    if !refresh && (state.answer.is_some() || newer_drop) {
        state.todo = if newer_drop { todo } else { None };
        return Ok(state);
    }
    if !create {
        return Ok(state);
    }
    if idea.stage != IdeaStage::Open {
        bail!(
            "idea {} is {}: no new todos",
            py_repr(idea.name.as_str()),
            idea.status
        );
    }
    let commit = commit_of(ledger, &state.code)?;
    let data = json!({"question": question, "view": view.as_str(), "target": state.code.target,
                      "idea": idea.name.as_str(), "code_ref": commit, "tree": state.code.tree});
    let text = todo_text(&question, view, &state.code, &commit);
    let task = tasks::add(ledger, idea, &text, TaskKind::Inspect, Some(&data), None)?;
    if state.code.target == "idea" {
        // keep the snapshot commit reachable (a run's already is)
        let wt = state.code.worktree.clone().expect("an idea's worktree");
        snapshot::update_ref(
            &wt,
            &ref_name(&task.task.id),
            &commit,
            &format!("arboria: inspect {}", task.task.id),
        )?;
    }
    state.todo = Some(task);
    state.created = true;
    Ok(state)
}

// ---------------------------------------------------------------- reading code at code_ref

/// A span's file path: repo-relative POSIX, 1..300 characters, no leading `/`, no `\`,
/// no empty / `.` / `..` segment.
pub fn check_path(path: &Value) -> Result<String> {
    let s = match path.as_str() {
        Some(s) if !s.is_empty() && s.chars().count() <= 300 => s,
        _ => bail!(
            "path must be a repo-relative file path, got {}",
            python_repr(path)
        ),
    };
    if s.starts_with('/') || s.contains('\\') || s.split('/').any(|p| matches!(p, "" | "." | ".."))
    {
        bail!(
            "path {}: use a plain repo-relative path like src/train.py (no leading /, no . or ..)",
            py_repr(s)
        );
    }
    Ok(s.to_string())
}

/// One file's lines at `commit`, exactly as git stores them (split on `\n` only, so
/// `\r` stays; a final empty line dropped). Refuses binary files and files over
/// [`MAX_FILE_BYTES`].
pub fn blob_lines(repo: &Path, commit: &str, path: &str) -> Result<Vec<String>> {
    check_path(&Value::String(path.into()))?;
    let spec = format!("{commit}:{path}");
    let kind = gitutil::run(&["cat-file", "-t", &spec], repo, &[], false)?;
    if kind.status != 0 {
        bail!(
            "{path} does not exist at {} (see `arb inspect source tNNN` for the files)",
            short(commit)
        );
    }
    let kind = kind.text();
    if kind != "blob" {
        bail!("{path} is a {kind} at {}, not a file", short(commit));
    }
    let size: u64 = gitutil::out(&["cat-file", "-s", &spec], repo, &[])?
        .parse()
        .map_err(|_| Error::arb(format!("cannot read the size of {path}")))?;
    if size > MAX_FILE_BYTES {
        bail!("{path} is too large to show ({size} bytes)");
    }
    let blob = gitutil::run(&["cat-file", "blob", &spec], repo, &[], false)?;
    if blob.status != 0 {
        bail!(
            "cannot read {path} at {}: {}",
            short(commit),
            py_strip(&blob.stderr)
        );
    }
    let text = String::from_utf8_lossy(&blob.stdout);
    if text.contains('\0') {
        bail!("{path} is a binary file");
    }
    let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    Ok(lines)
}

/// The files at `commit` (`arb inspect source TASK`).
pub fn list_files(repo: &Path, commit: &str) -> Result<Vec<String>> {
    let out = gitutil::run(
        &["ls-tree", "-r", "-z", "--name-only", commit],
        repo,
        &[],
        true,
    )?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect())
}

// ---------------------------------------------------------------- the agent's answer

/// Python `_text`: a string field of the answer (None / absent allowed unless required).
fn text_field(field: TextField, value: Option<&Value>, required: bool) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => records::validate_text(field, None, required),
        Some(Value::String(s)) => records::validate_text(field, Some(s), required),
        Some(_) => bail!("{} must be a string", field.name()),
    }
}

fn int_field(value: Option<&Value>, what: &str) -> Result<i64> {
    match value.and_then(|v| if v.is_boolean() { None } else { v.as_i64() }) {
        Some(n) => Ok(n),
        None => bail!(
            "{what} must be an integer line number, got {}",
            python_repr(value.unwrap_or(&Value::Null))
        ),
    }
}

/// Python truthiness (`x or default`).
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

/// `[\x00-\x08\x0b-\x1f\x7f]`.
fn has_control(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '\x00'..='\x08' | '\x0b'..='\x1f' | '\x7f'))
}

/// Python `re.match(r"^click\b", line)`.
fn starts_with_click(line: &str) -> bool {
    match line.strip_prefix("click") {
        None => false,
        Some(rest) => !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_'),
    }
}

/// `text` with every `<br\s*/?>` (any case) removed.
fn strip_br(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<'
            && chars
                .get(i + 1)
                .is_some_and(|c| c.eq_ignore_ascii_case(&'b'))
            && chars
                .get(i + 2)
                .is_some_and(|c| c.eq_ignore_ascii_case(&'r'))
        {
            let mut j = i + 3;
            while chars.get(j).is_some_and(|c| py_isspace(*c)) {
                j += 1;
            }
            if chars.get(j) == Some(&'/') {
                j += 1;
            }
            if chars.get(j) == Some(&'>') {
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `<\s*/?\s*[A-Za-z!?]` anywhere.
fn has_tag(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len()).filter(|&i| chars[i] == '<').any(|i| {
        let mut j = i + 1;
        while chars.get(j).is_some_and(|c| py_isspace(*c)) {
            j += 1;
        }
        if chars.get(j) == Some(&'/') {
            j += 1;
        }
        while chars.get(j).is_some_and(|c| py_isspace(*c)) {
            j += 1;
        }
        chars
            .get(j)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == '!' || *c == '?')
    })
}

/// Validate a Mermaid source (see the module docs); returns it stripped.
pub fn check_mermaid(src: &str) -> Result<String> {
    let text = records::validate_text(TextField::Mermaid, Some(src), true)?.expect("required");
    if has_control(&text) {
        bail!("mermaid: control characters are not allowed");
    }
    let lines: Vec<&str> = py_splitlines(&text).into_iter().map(py_strip).collect();
    let first = lines
        .iter()
        .find(|l| !l.is_empty() && !l.starts_with("%%"))
        .and_then(|l| l.split(py_isspace).find(|w| !w.is_empty()))
        .unwrap_or("");
    if !MERMAID_KINDS.contains(&first) {
        bail!(
            "mermaid must start with a diagram type ({}, …), got {}",
            MERMAID_KINDS[..6].join(", "),
            py_repr(first)
        );
    }
    if text.contains("%%{") {
        bail!("mermaid: no %%{{init}}%% directives (theme and security are arb's)");
    }
    for l in &lines {
        let lower = l.to_lowercase();
        if starts_with_click(l) || lower.contains("javascript:") || lower.contains("href") {
            let head: String = l.chars().take(60).collect();
            bail!("mermaid: no click handlers or links ({})", py_repr(&head));
        }
    }
    if has_tag(&strip_br(&text)) {
        bail!("mermaid: no HTML in labels (only <br>); write plain text");
    }
    Ok(text)
}

/// `node` appears in `text` with no `[A-Za-z0-9_-]` right before or after it.
fn node_in(text: &str, node: &str) -> bool {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let bytes = text.as_bytes();
    text.match_indices(node).any(|(i, _)| {
        let before = i.checked_sub(1).map(|j| bytes[j]);
        let after = bytes.get(i + node.len()).copied();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// [`validate_result`] for a todo given by its id, kind and payload (the vectors' form).
pub fn validate_answer(
    repo: &Path,
    task_id: &str,
    kind: &str,
    payload: &Value,
    data: &Value,
) -> Result<Value> {
    if kind != "inspect" {
        bail!("todo {task_id} is not a 读代码 todo (kind {kind}); use `arb todo done`");
    }
    let field = |k: &str| {
        payload
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let view = field("view").parse::<InspectView>()?;
    let commit = field("code_ref");
    let Some(data) = data.as_object() else {
        bail!("the answer must be a JSON object");
    };
    let allowed = keys(view);
    let mut extra: Vec<&str> = data
        .keys()
        .map(String::as_str)
        .filter(|k| !allowed.contains(k))
        .collect();
    if !extra.is_empty() {
        extra.sort();
        bail!(
            "unknown keys for view={}: {} (allowed: {})",
            view.as_str(),
            extra.join(", "),
            allowed.join(", ")
        );
    }
    let mut out = Map::new();
    out.insert("view".into(), view.as_str().into());
    out.insert(
        "question".into(),
        payload.get("question").cloned().unwrap_or(Value::Null),
    );
    out.insert("code_ref".into(), commit.clone().into());
    out.insert(
        "tree".into(),
        payload.get("tree").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "target".into(),
        payload.get("target").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "title".into(),
        text_field(TextField::InspectTitle, data.get("title"), true)?.into(),
    );
    out.insert(
        "summary".into(),
        text_field(TextField::InspectSummary, data.get("summary"), false)?.into(),
    );
    let empty = Vec::new();
    let raw_spans: &Vec<Value> = if truthy(data.get("spans")) {
        match data.get("spans") {
            Some(Value::Array(a)) => a,
            _ => bail!("spans must be a list"),
        }
    } else {
        &empty
    };
    if view == InspectView::Code && raw_spans.is_empty() {
        bail!("view=code needs at least one span (the code to show)");
    }
    if raw_spans.len() > MAX_SPANS {
        bail!(
            "{} spans (max {MAX_SPANS}): keep to the core",
            raw_spans.len()
        );
    }
    let mut spans = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let (mut total, mut notes) = (0i64, 0usize);
    let mut cache: std::collections::HashMap<String, Vec<String>> = Default::default();
    for (i, s) in raw_spans.iter().enumerate() {
        let Some(s) = s.as_object() else {
            bail!("spans[{i}] must be an object");
        };
        let mut bad: Vec<&str> = s
            .keys()
            .map(String::as_str)
            .filter(|k| !SPAN_KEYS.contains(k))
            .collect();
        if !bad.is_empty() {
            bad.sort();
            bail!("spans[{i}]: unknown keys {}", bad.join(", "));
        }
        let sid = s
            .get("id")
            .cloned()
            .unwrap_or_else(|| Value::String(format!("s{}", i + 1)));
        match sid.as_str() {
            Some(x) if id_ok(x) && !ids.iter().any(|y| y == x) => ids.push(x.to_string()),
            _ => bail!(
                "spans[{i}].id {}: letters, digits, _ or -, unique",
                python_repr(&sid)
            ),
        }
        let sid = sid.as_str().expect("checked").to_string();
        let path = check_path(s.get("path").unwrap_or(&Value::Null))?;
        let start = int_field(s.get("start"), &format!("{path} start"))?;
        let end = int_field(s.get("end"), &format!("{path} end"))?;
        if !cache.contains_key(&path) {
            let lines = blob_lines(repo, &commit, &path)?;
            cache.insert(path.clone(), lines);
        }
        let lines = &cache[&path];
        if !(1 <= start && start <= end && end <= lines.len() as i64) {
            bail!(
                "{path}:{start}-{end} is outside the file ({} lines at {})",
                lines.len(),
                short(&commit)
            );
        }
        total += end - start + 1;
        if total > MAX_LINES as i64 {
            bail!(
                "the spans cover more than {MAX_LINES} lines: keep to the core (shorter ranges, fewer spans)"
            );
        }
        let mut line_notes: Vec<(i64, String)> = Vec::new();
        if truthy(s.get("line_notes")) {
            let Some(raw) = s.get("line_notes").and_then(Value::as_object) else {
                bail!("{path}: line_notes must map line numbers to notes");
            };
            for (k, v) in raw {
                let n = if !k.is_empty() && k.chars().all(|c| records::py_decimal(c).is_some()) {
                    k.chars().try_fold(0i64, |acc, c| {
                        acc.checked_mul(10)?
                            .checked_add(records::py_decimal(c)? as i64)
                    })
                } else {
                    None
                };
                let Some(n) = n.filter(|n| (start..=end).contains(n)) else {
                    bail!(
                        "{path}: line_notes key {} is not a line in {start}-{end}",
                        py_repr(k)
                    );
                };
                let note = text_field(TextField::LineNote, Some(v), true)?.expect("required");
                line_notes.retain(|(m, _)| *m != n);
                line_notes.push((n, note));
            }
        }
        notes += line_notes.len();
        if notes > MAX_LINE_NOTES {
            bail!("more than {MAX_LINE_NOTES} line notes: annotate only what matters");
        }
        line_notes.sort_by_key(|(n, _)| *n);
        let mut ln = Map::new();
        for (n, note) in line_notes {
            ln.insert(n.to_string(), note.into());
        }
        let mut span = Map::new();
        span.insert("id".into(), sid.into());
        span.insert("path".into(), path.clone().into());
        span.insert("start".into(), start.into());
        span.insert("end".into(), end.into());
        span.insert(
            "note".into(),
            text_field(TextField::SpanNote, s.get("note"), false)?.into(),
        );
        span.insert("line_notes".into(), Value::Object(ln));
        span.insert(
            "lines".into(),
            lines[(start - 1) as usize..end as usize].to_vec().into(),
        );
        spans.push(Value::Object(span));
    }
    out.insert("spans".into(), spans.into());
    if matches!(view, InspectView::Pseudo | InspectView::Simple) {
        out.insert(
            "text".into(),
            text_field(TextField::InspectText, data.get("text"), true)?.into(),
        );
        if view == InspectView::Simple {
            let lang = data.get("lang").cloned().unwrap_or_else(|| "python".into());
            match lang.as_str() {
                Some(l) if lang_ok(l) => {
                    out.insert("lang".into(), l.into());
                }
                _ => bail!(
                    "lang {}: a short language name like python",
                    python_repr(&lang)
                ),
            }
        }
    }
    if view == InspectView::Map {
        let mermaid = match data.get("mermaid") {
            None | Some(Value::Null) => {
                records::validate_text(TextField::Mermaid, None, true)?;
                unreachable!("required")
            }
            Some(Value::String(m)) => check_mermaid(m)?,
            Some(_) => bail!("mermaid must be a string"),
        };
        out.insert("mermaid".into(), mermaid.clone().into());
        let empty = Map::new();
        let nodes = if truthy(data.get("nodes")) {
            match data.get("nodes") {
                Some(Value::Object(m)) if m.len() <= MAX_NODES => m,
                _ => bail!("nodes must map node ids to span ids (at most {MAX_NODES})"),
            }
        } else {
            &empty
        };
        let mut clean = Map::new();
        for (node, refs) in nodes {
            if !id_ok(node) {
                bail!("nodes: bad node id {}", py_repr(node));
            }
            if !node_in(&mermaid, node) {
                bail!("nodes: {} does not appear in the diagram", py_repr(node));
            }
            let refs = match refs {
                Value::String(_) => vec![refs.clone()],
                Value::Array(a) => a.clone(),
                _ => Vec::new(),
            };
            let known = |r: &Value| r.as_str().is_some_and(|r| ids.iter().any(|i| i == r));
            if refs.is_empty() || !refs.iter().all(known) {
                let mut sorted = ids.clone();
                sorted.sort();
                let shown = if sorted.is_empty() {
                    "none given".to_string()
                } else {
                    sorted.join(", ")
                };
                bail!(
                    "nodes[{}] must list span ids from spans ({shown})",
                    py_repr(node)
                );
            }
            clean.insert(node.clone(), refs.into());
        }
        out.insert("nodes".into(), Value::Object(clean));
    }
    Ok(Value::Object(out))
}

/// Validate an answer against the todo and extract the source lines from git; returns
/// what will be stored as `result_json`.
pub fn validate_result(repo: &Path, task: &TaskView, data: &Value) -> Result<Value> {
    validate_answer(
        repo,
        task.task.id.as_str(),
        task.task.kind.as_str(),
        &tasks::payload(&task.task)?,
        data,
    )
}

/// Parse `--file` contents (JSON).
pub fn load_answer(text: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(|e| Error::Arb(format!("the answer is not valid JSON: {e}")))
}

/// The todo, refusing other kinds.
pub fn open_inspect_task(ledger: &Ledger, task_id: &TaskId) -> Result<TaskView> {
    let task = tasks::get_task(ledger, task_id)?;
    if task.task.kind != TaskKind::Inspect {
        bail!(
            "todo {task_id} is not a 读代码 todo (kind {})",
            task.task.kind
        );
    }
    Ok(task)
}

/// `arb inspect source TASK [PATH]`: the file list, or the file with line numbers.
pub fn source_text(
    ledger: &Ledger,
    repo: &Path,
    task_id: &TaskId,
    path: Option<&str>,
) -> Result<String> {
    let task = open_inspect_task(ledger, task_id)?;
    let p = tasks::payload(&task.task)?;
    let commit = p
        .get("code_ref")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(path) = path else {
        return Ok(list_files(repo, commit)?
            .into_iter()
            .map(|f| f + "\n")
            .collect());
    };
    let lines = blob_lines(repo, commit, path)?;
    let width = lines.len().to_string().len();
    let mut out = format!("# {path} @ {} ({} lines)\n", short(commit), lines.len());
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!("{:>width$}  {line}\n", i + 1));
    }
    Ok(out)
}

/// `arb inspect check`: validate and render as text; saves nothing.
pub fn check(ledger: &Ledger, repo: &Path, task_id: &TaskId, data: &Value) -> Result<String> {
    let task = open_inspect_task(ledger, task_id)?;
    let result = validate_result(repo, &task, data)?;
    Ok(format!(
        "{}\n\nok: valid for {task_id} (nothing saved; deliver with `arb inspect done {task_id} --file …`)",
        to_text(None, Some(&result))
    ))
}

/// `arb inspect done`: validate, store the `inspections` row, close the todo
/// (`inspect_done` + `todo_done` events). Returns the stored answer.
pub fn deliver(ledger: &mut Ledger, repo: &Path, task_id: &TaskId, data: &Value) -> Result<Value> {
    let task = open_inspect_task(ledger, task_id)?;
    if task.task.status != TaskStatus::Open {
        bail!("todo {task_id} is already {}", task.task.status);
    }
    let result = validate_result(repo, &task, data)?;
    let p = tasks::payload(&task.task)?;
    let title = result["title"].as_str().unwrap_or_default();
    let mut summary = title.to_string();
    if let Some(s) = result["summary"].as_str() {
        summary.push_str(&format!("：{s}"));
    }
    let limit = TextField::Result.limit();
    if summary.chars().count() > limit {
        summary = summary.chars().take(limit - 1).collect::<String>() + "…";
    }
    let owner = task.owner_name().to_string();
    let spans = result["spans"].as_array().map_or(0, Vec::len);
    let lines: usize = result["spans"].as_array().map_or(0, |a| {
        a.iter()
            .map(|s| s["lines"].as_array().map_or(0, Vec::len))
            .sum()
    });
    let s = |k: &str| {
        p.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let stored = python_dumps(&result);
    ledger.write(|tx| {
        let ts = now();
        tx.execute(
            "INSERT INTO inspections (task_id, idea_id, question, view, target, code_ref, tree, \
             result_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                task_id,
                task.task.idea_id.0,
                s("question"),
                s("view"),
                s("target"),
                s("code_ref"),
                s("tree"),
                stored,
                ts
            ],
        )?;
        tx.execute(
            "UPDATE tasks SET status = 'done', done_at = ?1, result = ?2 WHERE id = ?3",
            rusqlite::params![ts, summary, task_id],
        )?;
        tx.event(
            EventKind::InspectDone,
            task_id.as_str(),
            &json!({"idea": owner, "view": s("view"), "spans": spans, "lines": lines}),
        )?;
        tx.event(
            EventKind::TodoDone,
            task_id.as_str(),
            &json!({"idea": owner, "kind": "inspect", "result_chars": summary.chars().count()}),
        )
    })?;
    Ok(result)
}

// ---------------------------------------------------------------- text

/// The muted line about where an answer stands (empty when fresh).
pub fn status_line(state: &State) -> String {
    match state.status() {
        InspectStatus::Pending => {
            let t = state.todo.as_ref().expect("pending has a todo");
            let lead = if state.created {
                "已交给 agent"
            } else {
                "等 agent"
            };
            let old = if state.answer.is_some() {
                "，下面是上一版"
            } else {
                ""
            };
            format!(
                "{lead}（{}）：{} · {}{old}。",
                t.task.id,
                view_label(state.view),
                state.question
            )
        }
        InspectStatus::Dropped => {
            let t = state.todo.as_ref().expect("dropped has a todo");
            let why = t
                .task
                .result
                .as_deref()
                .filter(|r| !r.is_empty())
                .map(|r| format!("：{r}"))
                .unwrap_or_default();
            format!(
                "agent 没有做（{}{why}）。.refresh() 重新交给 agent",
                t.task.id
            )
        }
        InspectStatus::Stale => format!(
            "代码已变（答案基于 {}，之后工作区改过），.refresh() 重新生成",
            short(&state.answer.as_ref().expect("stale has an answer").code_ref)
        ),
        InspectStatus::Fresh => String::new(),
    }
}

fn label_of(view: &Value) -> &'static str {
    view.as_str()
        .and_then(|v| v.parse::<InspectView>().ok())
        .map_or("?", view_label)
}

/// Plain-text rendering (`arb inspect check`, the app's fallback).
pub fn to_text(state: Option<&State>, result: Option<&Value>) -> String {
    let mut lines: Vec<String> = Vec::new();
    let parsed;
    let mut result = result;
    if let Some(state) = state {
        parsed = state.result();
        result = parsed.as_ref();
        let title = result
            .and_then(|r| r["title"].as_str())
            .unwrap_or(&state.question);
        lines.push(format!("{title}  [读代码 · {}]", view_label(state.view)));
        let status = status_line(state);
        if !status.is_empty() {
            lines.push(status);
        }
    } else if let Some(r) = result {
        lines.push(format!(
            "{}  [读代码 · {}]",
            r["title"].as_str().unwrap_or_default(),
            label_of(&r["view"])
        ));
    }
    let Some(r) = result else {
        return lines.join("\n");
    };
    let commit = r["code_ref"].as_str().unwrap_or_default();
    if let Some(s) = r["summary"].as_str().filter(|s| !s.is_empty()) {
        lines.push(String::new());
        lines.push(s.to_string());
    }
    let view = r["view"].as_str().unwrap_or_default();
    if matches!(view, "pseudo" | "simple") {
        lines.push(String::new());
        lines.push(format!("--- agent 写的{} ---", label_of(&r["view"])));
        lines.push(r["text"].as_str().unwrap_or_default().to_string());
    }
    if view == "map" {
        lines.push(String::new());
        lines.push("--- Mermaid ---".into());
        lines.push(r["mermaid"].as_str().unwrap_or_default().to_string());
        if let Some(nodes) = r["nodes"].as_object() {
            for (node, refs) in nodes {
                let refs: Vec<&str> = refs
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                lines.push(format!("  {node} → {}", refs.join(", ")));
            }
        }
    }
    let spans: Vec<&Map<String, Value>> = r["spans"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    let width = spans
        .iter()
        .map(|s| s["end"].to_string().len())
        .max()
        .unwrap_or(1);
    for s in spans {
        let start = s["start"].as_i64().unwrap_or(1);
        lines.push(String::new());
        lines.push(format!(
            "[{}] {}:{}–{} @ {}",
            s["id"].as_str().unwrap_or_default(),
            s["path"].as_str().unwrap_or_default(),
            start,
            s["end"],
            short(commit)
        ));
        if let Some(n) = s
            .get("note")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
        {
            lines.push(format!("  agent：{n}"));
        }
        let notes = s.get("line_notes").and_then(Value::as_object);
        for (k, line) in s["lines"].as_array().into_iter().flatten().enumerate() {
            let i = start + k as i64;
            lines.push(format!(
                "{i:>width$} │ {}",
                line.as_str().unwrap_or_default()
            ));
            if let Some(note) = notes
                .and_then(|n| n.get(&i.to_string()))
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
            {
                lines.push(format!("{:>width$} ↳ agent：{note}", ""));
            }
        }
    }
    lines.join("\n")
}

/// The guide's template (Python `inspection.GUIDE`, verbatim).
const GUIDE: &str = include_str!("inspection_guide.md");

/// `arb inspect guide`: the answer conventions (for the agent).
pub fn guide() -> String {
    GUIDE
        .replace("{max_spans}", &MAX_SPANS.to_string())
        .replace("{max_lines}", &MAX_LINES.to_string())
        .replace("{max_notes}", &MAX_LINE_NOTES.to_string())
        .replace("{{", "{")
        .replace("}}", "}")
}
