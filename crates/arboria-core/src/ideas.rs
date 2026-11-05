//! Ideas through their whole lifecycle: seed (no repo yet) → open (branch `idea/<name>`,
//! worktree folder, notebook) → merged / dropped. Ports `arboria.ideas`,
//! `arboria.seeds` (seeds are ideas at stage `seed` since schema v8) and the routing
//! logic of `cli._idea_route`.
//!
//! Invariants:
//! - Routing is fixed by the current directory ([`route`]): in the seed home or outside
//!   any git repo → seeds; in a repo without `arb init` → an error, never a seed by
//!   accident; in an initialized repo → its ideas.
//! - Seeds (gist ≤300, hypothesis, metric) are written only by the human: every
//!   seed-writing function calls [`require_human_for_seeds`], which refuses inside
//!   Claude Code. This is enforced in code, not only by deny rules (`arb idea new/drop`
//!   are allowed for Claude in a repo).
//! - Planting runs in the target repo, only in the human's terminal (it creates a branch
//!   and a worktree), goes through [`new_idea`], and copies only the seed notebook's text
//!   blocks (parsed, never executed).
//! - Closing never loses work: a dropped idea's final state is snapshotted and tagged
//!   before its worktree and branch are removed; a dirty worktree is refused.
//! - Notebooks are marimo files the Python package writes ([`crate::notebooks`], until the
//!   app's markdown notebooks): an interpreter that cannot import `arboria` only costs the
//!   notebook (a warning, `notebook` null in the `idea_new` event; `arb idea notebook`
//!   writes it later).
//! - v8: a seed is an ideas row without a branch (stage `seed`, or `dropped` before
//!   planting); it is planted iff it has `plantings` rows.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::db::{self, now, Ledger};
use crate::model::{Confidence, EventKind, Idea, IdeaName, IdeaStage, Planting, Run};
use crate::paths::{Home, RepoContext};
use crate::records::{py_repr, require_text, validate_text, TextField};
use crate::{bail, gitutil, notebooks, snapshot, Error, Result, Warn};

/// `seeds.HUMAN_ONLY`.
pub const HUMAN_ONLY: &str = "{command} 只由人在自己的终端里做（{what}）；Claude 不能运行它。种子是人的：Claude 可以读（仓库外的 arb idea list / show、seeds/<名字>.py），不写";
/// `seeds.PLANT_NEEDS`.
pub const PLANT_NEEDS: &str = "种下需要假设和指标：种子 {name} 还没有{missing}。用 --hypothesis / --metric 给出，或先在仓库外 arb idea set {name} --hypothesis …";
/// `idea/<name>`.
pub fn branch_for(name: &IdeaName) -> String {
    format!("idea/{name}")
}

/// `<main parent>/<main name>-<name>`: the idea's worktree folder.
pub fn worktree_for(ctx: &RepoContext, name: &IdeaName) -> PathBuf {
    let main_name = ctx
        .main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = ctx.main.parent().unwrap_or(Path::new("/"));
    parent.join(format!("{main_name}-{name}"))
}

/// `.arboria/notebooks/<name>.py`: where the Python side keeps an idea's notebook.
pub fn notebook_path(state_dir: &Path, name: &str) -> PathBuf {
    state_dir.join("notebooks").join(format!("{name}.py"))
}

// ---------------------------------------------------------------- routing

/// Where `arb idea …` acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The seeds of this home (cwd in the home, or outside any git repo).
    Seeds(Home),
    /// The ideas of this initialized repo.
    Repo(RepoContext),
}

/// `cli._idea_route`: the home for seeds, or the initialized repo; a repo without
/// `arb init` is `Error::NotInitialized`.
pub fn route(cwd: &Path) -> Result<Route> {
    if let Some(h) = home_of(Some(cwd)) {
        return Ok(Route::Seeds(h));
    }
    match crate::paths::locate(Some(cwd)) {
        Err(Error::NotARepo(_)) => Ok(Route::Seeds(crate::paths::home())),
        Err(e) => Err(e),
        Ok(ctx) => Ok(Route::Repo(ctx.require_initialized()?)),
    }
}

/// The home containing `cwd`, if any (`seeds.home_of`; it need not be initialized).
pub fn home_of(cwd: Option<&Path>) -> Option<Home> {
    let h = crate::paths::home();
    let here;
    let cwd = match cwd {
        Some(c) => c,
        None => {
            here = std::env::current_dir().ok()?;
            &here
        }
    };
    h.contains(cwd).then_some(h)
}

/// Create the home's dirs, backup repo and ledger (`seeds.init_home`); the actions taken.
pub fn init_home(h: &Home) -> Result<Vec<String>> {
    let mut actions = Vec::new();
    if !h.root.exists() {
        actions.push(format!("created {}", h.root.display()));
    }
    std::fs::create_dir_all(h.seeds_dir())?;
    // also marks the state dir as a home's
    std::fs::create_dir_all(h.mirror_dir())?;
    let had_db = h.db_path().exists();
    db::init_backup_repo(&h.state_dir())?;
    let ledger = Ledger::open(&h.state_dir())?;
    if !had_db {
        actions.push(format!("created ledger {}", h.db_path().display()));
    }
    ledger.backup("init home")?;
    Ok(actions)
}

/// Create the home's state dir, ledger and backup repo if missing (`seeds.ensure_home`);
/// `echo` reports what was created.
pub fn ensure_home(echo: Warn<'_>) -> Result<Home> {
    let h = crate::paths::home();
    if !h.initialized() {
        init_home(&h)?;
        echo(&format!(
            "种子园建在 {}（ARBORIA_HOME 可以改位置）",
            h.root.display()
        ));
    }
    Ok(h)
}

/// Refuse inside Claude Code: seeds are the human's (`seeds.require_human`); `what`
/// says what the command would do.
pub fn require_human_for_seeds(command: &str, what: &str) -> Result<()> {
    if crate::human::in_claude_code().is_some() {
        return Err(Error::Arb(
            HUMAN_ONLY
                .replace("{command}", command)
                .replace("{what}", what),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- ideas in a repo

/// An `arb idea list` row.
#[derive(Debug, Clone, Serialize)]
pub struct IdeaSummary {
    #[serde(flatten)]
    pub idea: Idea,
    pub parent_name: Option<String>,
    pub run_count: u32,
}

/// `arb idea show` of an idea.
#[derive(Debug, Clone, Serialize)]
pub struct IdeaDetail {
    #[serde(flatten)]
    pub idea: Idea,
    pub parent_name: Option<String>,
    pub notebook: String,
    pub runs: Vec<Run>,
}

/// How an idea was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseMode {
    /// Merge `idea/<name>` into the main worktree's branch.
    Merge,
    /// Archive a final snapshot (tagged) and delete the branch.
    Drop,
}

/// What closing did (`idea["close"]` in Python; also the `idea_close` event).
#[derive(Debug, Clone, Serialize)]
pub struct CloseInfo {
    pub name: String,
    pub mode: CloseMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// `arb idea close` / `arb idea drop` of an idea.
#[derive(Debug, Clone, Serialize)]
pub struct Closed {
    #[serde(flatten)]
    pub idea: Idea,
    pub close: CloseInfo,
}

pub(crate) fn idea_by_name(conn: &rusqlite::Connection, name: &str) -> Result<Option<Idea>> {
    db::one(conn, "SELECT * FROM ideas WHERE name = ?1", [name])
}

fn idea_by_id(conn: &rusqlite::Connection, id: crate::model::IdeaId) -> Result<Option<Idea>> {
    db::one(conn, "SELECT * FROM ideas WHERE id = ?1", [id.0])
}

/// An idea by name (`no idea named 'x' (see `arb idea list`)`).
pub fn get_idea(ledger: &Ledger, name: &IdeaName) -> Result<Idea> {
    idea_by_name(ledger.conn(), name.as_str())?.ok_or_else(|| {
        Error::Arb(format!(
            "no idea named {} (see `arb idea list`)",
            py_repr(name.as_str())
        ))
    })
}

/// The idea whose worktree contains the current directory.
pub fn current_idea(ctx: &RepoContext, ledger: &Ledger) -> Result<Option<Idea>> {
    let Some(top) = &ctx.toplevel else {
        return Ok(None);
    };
    db::one(
        ledger.conn(),
        "SELECT * FROM ideas WHERE worktree_path = ?1 ORDER BY id DESC LIMIT 1",
        [top.display().to_string()],
    )
}

/// [`current_idea`], refusing when there is none or it is not open.
pub fn require_current_idea(ctx: &RepoContext, ledger: &Ledger) -> Result<Idea> {
    let Some(idea) = current_idea(ctx, ledger)? else {
        bail!(
            "the current directory is not an idea worktree; cd into one (see `arb idea list`) or create one with `arb idea new`"
        );
    };
    if idea.stage != IdeaStage::Open {
        bail!("idea {} is {}", py_repr(idea.name.as_str()), idea.stage);
    }
    Ok(idea)
}

/// The seed an idea grows from (`arb idea plant`).
#[derive(Debug, Clone, Copy)]
pub struct SeedOrigin<'a> {
    /// The seed's row in the home ledger.
    pub seed: &'a Idea,
    /// Its home.
    pub home: &'a Home,
    /// The seed notebook's markdown cells (copied, folded, into the idea's notebook).
    pub markdown: &'a [String],
}

/// What a new idea starts from.
#[derive(Debug, Clone, Default)]
pub struct NewIdea<'a> {
    pub hypothesis: &'a str,
    pub metric: &'a str,
    /// Branch off this open idea.
    pub parent: Option<&'a IdeaName>,
    /// Fork from this run's snapshot (`lab.fork`).
    pub from_run: Option<&'a Run>,
    /// Planting: the seed it grows from.
    pub seed: Option<SeedOrigin<'a>>,
}

/// `arb idea new` in a repo: branch `idea/NAME`, worktree, ledger row.
pub fn new_idea(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    name: &IdeaName,
    new: NewIdea<'_>,
    warn: Warn<'_>,
) -> Result<Idea> {
    let hypothesis = require_text(TextField::Hypothesis, new.hypothesis)?;
    let metric = require_text(TextField::Metric, new.metric)?;
    if idea_by_name(ledger.conn(), name.as_str())?.is_some() {
        bail!(
            "an idea named {} already exists (names are never reused)",
            py_repr(name.as_str())
        );
    }
    let branch = branch_for(name);
    let (parent_row, base) = if let Some(run) = new.from_run {
        let parent_row = idea_by_id(ledger.conn(), run.idea_id)?
            .ok_or_else(|| Error::Arb(format!("run {} has no idea", run.id)))?;
        if let Some(p) = new.parent {
            if p != &parent_row.name {
                bail!("run {} belongs to {}, not {p}", run.id, parent_row.name);
            }
        }
        let base = run.snapshot_commit.clone();
        if gitutil::rev_parse(&base, &ctx.main)?.is_none() {
            let short: String = base.chars().take(12).collect();
            bail!("snapshot {short} of {} is missing from the repo", run.id);
        }
        (Some(parent_row), base)
    } else if let Some(p) = new.parent {
        let parent_row = get_idea(ledger, p)?;
        if parent_row.stage != IdeaStage::Open {
            bail!(
                "parent idea {} is {}; its branch is gone",
                py_repr(p.as_str()),
                parent_row.stage
            );
        }
        let pbranch = parent_row.branch.clone().unwrap_or_default();
        let Some(base) = gitutil::rev_parse(&pbranch, &ctx.main)? else {
            bail!("parent branch {pbranch} does not exist");
        };
        (Some(parent_row), base)
    } else {
        let Some(base) = gitutil::rev_parse("HEAD", &ctx.main)? else {
            bail!(
                "the main worktree {} has no commits yet; commit something first",
                ctx.main.display()
            );
        };
        (None, base)
    };

    if gitutil::ref_exists(&format!("refs/heads/{branch}"), &ctx.main)? {
        bail!("branch {branch} already exists; pick another name");
    }
    let path = worktree_for(ctx, name);
    if path.exists() || path.symlink_metadata().is_ok() {
        bail!(
            "{} already exists; pick another name or move it away",
            path.display()
        );
    }
    let path_s = path.display().to_string();
    gitutil::run(
        &["worktree", "add", "-q", "-b", &branch, &path_s, &base],
        &ctx.main,
        &[],
        true,
    )?;
    let path = path.canonicalize().unwrap_or(path);
    let written = ledger.write(|tx| {
        let ts = now();
        tx.execute(
            "INSERT INTO ideas (name, hypothesis, metric, parent_id, branch, worktree_path, \
             created_at, updated_at, stage, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, \
             'open', 'open')",
            rusqlite::params![
                name,
                hypothesis,
                metric,
                parent_row.as_ref().map(|p| p.id.0),
                branch,
                path.display().to_string(),
                ts
            ],
        )?;
        let row = idea_by_name(tx, name.as_str())?
            .ok_or_else(|| Error::Arb(format!("idea {name} vanished")))?;
        let seed_note = new.seed.map(|o| notebooks::SeedNote {
            name: o.seed.name.as_str().to_string(),
            notebook: seed_notebook_file(o.home, o.seed.name.as_str()),
            markdown: o.markdown.to_vec(),
            home: o.home.root.clone(),
        });
        let nb = notebooks::create(
            &ctx.main,
            &row,
            parent_row.as_ref().map(|p| p.name.as_str()),
            new.from_run,
            seed_note.as_ref(),
            true,
            &mut *warn,
        )?;
        let nb = match nb {
            Ok(p) => Value::String(
                p.strip_prefix(ctx.state_dir())
                    .unwrap_or(&p)
                    .display()
                    .to_string(),
            ),
            Err(why) => {
                warn(&notebooks::unavailable_warning(
                    &why,
                    &format!("run `arb idea notebook {name}`"),
                ));
                Value::Null
            }
        };
        let mut event = Map::new();
        event.insert("branch".into(), branch.clone().into());
        event.insert("base".into(), base.clone().into());
        event.insert(
            "parent".into(),
            json!(parent_row.as_ref().map(|p| p.name.as_str())),
        );
        event.insert(
            "from_run".into(),
            json!(new.from_run.map(|r| r.id.as_str())),
        );
        event.insert("notebook".into(), nb);
        if let Some(origin) = new.seed {
            event.insert("seed".into(), origin.seed.name.as_str().into());
            event.insert(
                "seed_home".into(),
                origin.home.root.display().to_string().into(),
            );
        }
        tx.event(EventKind::IdeaNew, name.as_str(), &Value::Object(event))
    });
    if let Err(e) = written {
        let _ = gitutil::run(
            &["worktree", "remove", "--force", &path.display().to_string()],
            &ctx.main,
            &[],
            false,
        );
        let _ = gitutil::run(&["branch", "-D", &branch], &ctx.main, &[], false);
        let nb = crate::nbdoc::idea_path(&ctx.state_dir(), name.as_str());
        if nb.exists() && idea_by_name(ledger.conn(), name.as_str())?.is_none() {
            let _ = std::fs::remove_file(nb);
        }
        return Err(e);
    }
    get_idea(ledger, name)
}

/// `arb idea notebook`: create the notebook (and, for an open idea, the worktree link)
/// of an idea that has none; (idea, notebook path, created). An existing notebook is
/// never touched (only a missing link is repaired).
pub fn ensure_notebook(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    name: Option<&IdeaName>,
    warn: Warn<'_>,
) -> Result<(Idea, PathBuf, bool)> {
    let idea = named_or_current(
        ctx,
        ledger,
        name,
        "not inside an idea worktree; pass the idea NAME",
    )?;
    let md = crate::nbdoc::idea_path(&ctx.state_dir(), idea.name.as_str());
    let marimo = notebook_path(&ctx.state_dir(), idea.name.as_str());
    let link = idea.stage == IdeaStage::Open
        && idea
            .worktree_path
            .as_deref()
            .is_some_and(|w| Path::new(w).is_dir());
    let unavailable = |why: String| {
        Error::Arb(notebooks::unavailable_warning(
            &why,
            "run `arb idea notebook` again",
        ))
    };
    if md.exists() {
        if link {
            let wt = idea.worktree_path.as_deref().unwrap_or_default();
            notebooks::link_md(&ctx.common_dir, Path::new(wt), &md, warn)?;
        }
        return Ok((idea, md, false));
    }
    if marimo.exists() {
        // an idea from before the app's notebooks: its marimo file (`arb idea migrate`)
        if link {
            notebooks::link_marimo(&ctx.main, &idea, warn)?.map_err(unavailable)?;
        }
        return Ok((idea, marimo, false));
    }
    let path = md;
    let parent = parent_name(ledger, &idea)?;
    let rel = path
        .strip_prefix(ctx.state_dir())
        .unwrap_or(&path)
        .display()
        .to_string();
    ledger.write(|tx| {
        notebooks::create(&ctx.main, &idea, parent.as_deref(), None, None, link, warn)?
            .map_err(unavailable)?;
        tx.event(
            EventKind::NotebookNew,
            idea.name.as_str(),
            &json!({"notebook": rel}),
        )
    })?;
    Ok((idea, path, true))
}

/// What `arb idea style` did to one notebook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Restyled {
    Added,
    Already,
    Own,
    Unknown,
}

/// `arb idea style` in a repo: refresh `.arboria/notebooks/arboria.css`, link it into
/// each open idea's worktree, add `css_file="arboria.css"` where it is missing (only that
/// line changes). Idempotent.
pub fn restyle(
    ctx: &RepoContext,
    ledger: &Ledger,
    warn: Warn<'_>,
) -> Result<Vec<(PathBuf, Restyled)>> {
    let dir = ctx.state_dir().join("notebooks");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let worktrees: Vec<String> = ledger
        .conn()
        .prepare("SELECT worktree_path FROM ideas WHERE stage = 'open' ORDER BY id")?
        .query_map([], |r| r.get::<_, Option<String>>(0))?
        .filter_map(|r| r.transpose())
        .collect::<rusqlite::Result<_>>()?;
    notebooks::restyle(&dir, Some(&ctx.main), &worktrees, warn)
}

/// `arb idea list`.
pub fn list_ideas(ledger: &Ledger) -> Result<Vec<IdeaSummary>> {
    let mut stmt = ledger.conn().prepare(
        "SELECT i.*, p.name AS parent_name, \
         (SELECT COUNT(*) FROM runs r WHERE r.idea_id = i.id) AS run_count \
         FROM ideas i LEFT JOIN ideas p ON p.id = i.parent_id ORDER BY i.id",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut map = db::row_map(row)?;
        let parent_name = match map.remove("parent_name") {
            Some(Value::String(s)) => Some(s),
            _ => None,
        };
        let run_count = map
            .remove("run_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        out.push(IdeaSummary {
            idea: db::from_map(map)?,
            parent_name,
            run_count,
        });
    }
    Ok(out)
}

fn parent_name(ledger: &Ledger, idea: &Idea) -> Result<Option<String>> {
    Ok(match idea.parent_id {
        Some(id) => idea_by_id(ledger.conn(), id)?.map(|p| p.name.to_string()),
        None => None,
    })
}

fn named_or_current(
    ctx: &RepoContext,
    ledger: &Ledger,
    name: Option<&IdeaName>,
    missing: &str,
) -> Result<Idea> {
    match name {
        Some(n) => get_idea(ledger, n),
        None => current_idea(ctx, ledger)?.ok_or_else(|| Error::arb(missing)),
    }
}

/// `arb idea show` (default: the current directory's idea).
pub fn show_idea(
    ctx: &RepoContext,
    ledger: &Ledger,
    name: Option<&IdeaName>,
) -> Result<IdeaDetail> {
    let idea = named_or_current(
        ctx,
        ledger,
        name,
        "not inside an idea worktree; pass the idea NAME (see `arb idea list`)",
    )?;
    let runs = db::all(
        ledger.conn(),
        "SELECT * FROM runs WHERE idea_id = ?1 ORDER BY id",
        [idea.id.0],
    )?;
    Ok(IdeaDetail {
        parent_name: parent_name(ledger, &idea)?,
        notebook: idea_notebook_file(&ctx.state_dir(), idea.name.as_str())
            .display()
            .to_string(),
        runs,
        idea,
    })
}

fn is_dirty(worktree: &Path) -> Result<bool> {
    Ok(!gitutil::out(
        &["status", "--porcelain", "--untracked-files=all"],
        worktree,
        &[],
    )?
    .is_empty())
}

fn remove_worktree_and_branch(ctx: &RepoContext, idea: &Idea) -> Result<()> {
    if let Some(path) = idea.worktree_path.as_deref() {
        if Path::new(path).exists() {
            gitutil::run(
                &["worktree", "remove", "--force", path],
                &ctx.main,
                &[],
                true,
            )?;
        }
    }
    gitutil::run(&["worktree", "prune"], &ctx.main, &[], true)?;
    if let Some(branch) = idea.branch.as_deref() {
        if gitutil::ref_exists(&format!("refs/heads/{branch}"), &ctx.main)? {
            gitutil::run(&["branch", "-D", branch], &ctx.main, &[], true)?;
        }
    }
    Ok(())
}

/// `arb idea close --merge|--drop` / `arb idea drop`.
pub fn close_idea(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    name: Option<&IdeaName>,
    mode: CloseMode,
    confidence: Option<Confidence>,
    allow_large: bool,
    warn: Warn<'_>,
) -> Result<Closed> {
    let idea = named_or_current(
        ctx,
        ledger,
        name,
        "not inside an idea worktree; pass the idea NAME",
    )?;
    let name = idea.name.clone();
    if idea.stage != IdeaStage::Open {
        bail!("idea {} is already {}", py_repr(name.as_str()), idea.stage);
    }
    let path = PathBuf::from(idea.worktree_path.clone().unwrap_or_default());
    let branch = idea.branch.clone().unwrap_or_default();
    let mut info = CloseInfo {
        name: name.to_string(),
        mode,
        merged_into: None,
        merge_commit: None,
        final_commit: None,
        final_ref: None,
        tag: None,
    };
    let stage = match mode {
        CloseMode::Drop => {
            let final_ref = format!("refs/arboria/ideas/{name}/final");
            let tag = format!("arboria/archive/{name}");
            if gitutil::ref_exists(&format!("refs/tags/{tag}"), &ctx.main)? {
                bail!("tag {tag} already exists; remove it first");
            }
            let final_commit = if path.exists() {
                snapshot::snapshot(
                    &path,
                    ledger.conn(),
                    &format!("arboria snapshot final {name}"),
                    allow_large,
                    None,
                    None,
                    warn,
                )?
                .commit
            } else {
                // worktree folder deleted by hand: archive the branch tip instead
                gitutil::rev_parse(&branch, &ctx.main)?.ok_or_else(|| {
                    Error::Arb(format!(
                        "worktree {} and branch {branch} are both gone",
                        path.display()
                    ))
                })?
            };
            snapshot::update_ref(
                &ctx.main,
                &final_ref,
                &final_commit,
                &format!("arboria: drop {name}"),
            )?;
            gitutil::run(&["tag", &tag, &final_commit], &ctx.main, &[], true)?;
            remove_worktree_and_branch(ctx, &idea)?;
            info.final_commit = Some(final_commit);
            info.final_ref = Some(final_ref);
            info.tag = Some(tag);
            IdeaStage::Dropped
        }
        CloseMode::Merge => {
            if path.exists() && is_dirty(&path)? {
                bail!(
                    "idea worktree {} has uncommitted changes; commit them on {branch} (or discard them) before merging",
                    path.display()
                );
            }
            let target = gitutil::run(
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                &ctx.main,
                &[],
                false,
            )?;
            if target.status != 0 {
                bail!(
                    "the main worktree {} is on a detached HEAD; check out a branch",
                    ctx.main.display()
                );
            }
            let target = target.text();
            let merge = gitutil::run(
                &[
                    "merge",
                    "--no-ff",
                    "--no-edit",
                    "-m",
                    &format!("arboria: merge idea {name}"),
                    &branch,
                ],
                &ctx.main,
                &[],
                false,
            )?;
            if merge.status != 0 {
                if gitutil::ok(
                    &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"],
                    &ctx.main,
                )? {
                    let _ = gitutil::run(&["merge", "--abort"], &ctx.main, &[], false);
                    bail!(
                        "merging {branch} into {target} conflicts; merge aborted, nothing changed. Resolve by merging {target} into {branch} in the idea worktree, then retry."
                    );
                }
                let detail = if merge.stderr.trim().is_empty() {
                    merge.text()
                } else {
                    merge.stderr.trim().to_string()
                };
                bail!("git merge of {branch} into {target} failed: {detail}");
            }
            info.merged_into = Some(target);
            info.merge_commit = gitutil::rev_parse("HEAD", &ctx.main)?;
            remove_worktree_and_branch(ctx, &idea)?;
            IdeaStage::Merged
        }
    };
    ledger.write(|tx| {
        let ts = now();
        tx.execute(
            "UPDATE ideas SET stage = ?1, status = ?1, closed_at = ?2, updated_at = ?2, \
             confidence = COALESCE(?3, confidence) WHERE id = ?4",
            rusqlite::params![
                stage.as_str(),
                ts,
                confidence.map(Confidence::as_str),
                idea.id.0
            ],
        )?;
        let mut event = serde_json::to_value(&info)?;
        event["confidence"] = json!(confidence.map(Confidence::as_str));
        tx.event(EventKind::IdeaClose, name.as_str(), &event)
    })?;
    Ok(Closed {
        idea: get_idea(ledger, &name)?,
        close: info,
    })
}

// ---------------------------------------------------------------- seeds (stage = seed)

/// A seed's status as Python names it (`seeds.status`): derived in v8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SeedState {
    Thinking,
    Planted,
    Dropped,
}

impl SeedState {
    /// `thinking` / `planted` / `dropped`.
    pub fn as_str(self) -> &'static str {
        match self {
            SeedState::Thinking => "thinking",
            SeedState::Planted => "planted",
            SeedState::Dropped => "dropped",
        }
    }

    /// `seeds.STATUS_LABELS`.
    pub fn label(self) -> &'static str {
        match self {
            SeedState::Thinking => "思考中",
            SeedState::Planted => "已种下",
            SeedState::Dropped => "已放弃",
        }
    }
}

/// A seed with its notebook, last edit and plantings (`seeds._decorate`).
#[derive(Debug, Clone, Serialize)]
pub struct SeedView {
    #[serde(flatten)]
    pub idea: Idea,
    /// Python's `seeds.status`.
    pub seed_status: SeedState,
    pub notebook: String,
    /// Later of the ledger's `updated_at` and the notebook's mtime.
    pub last_edited: String,
    pub planted_to: Vec<Planting>,
    /// The notebook file existed and was left as is (new only).
    pub kept_existing_notebook: bool,
}

/// The editable seed fields; `Some("")` clears one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeedFields<'a> {
    pub gist: Option<&'a str>,
    pub hypothesis: Option<&'a str>,
    pub metric: Option<&'a str>,
}

/// A seed by name (`no seed named 'x' …`).
pub fn get_seed(ledger: &Ledger, name: &IdeaName) -> Result<Idea> {
    db::one(
        ledger.conn(),
        "SELECT * FROM ideas WHERE name = ?1 AND branch IS NULL",
        [name],
    )?
    .ok_or_else(|| {
        Error::Arb(format!(
            "no seed named {} (see `arb idea list` outside any repo)",
            py_repr(name.as_str())
        ))
    })
}

fn plantings(ledger: &Ledger, seed: &Idea) -> Result<Vec<Planting>> {
    db::all(
        ledger.conn(),
        "SELECT * FROM plantings WHERE seed_idea_id = ?1 ORDER BY id",
        [seed.id.0],
    )
}

fn seed_state(seed: &Idea, planted: bool) -> SeedState {
    if seed.stage == IdeaStage::Dropped {
        SeedState::Dropped
    } else if planted {
        SeedState::Planted
    } else {
        SeedState::Thinking
    }
}

/// An idea's notebook file: the app's `.md`, else a marimo `.py` from before, else where
/// the `.md` goes.
pub fn idea_notebook_file(state_dir: &Path, name: &str) -> PathBuf {
    let md = crate::nbdoc::idea_path(state_dir, name);
    let py = notebook_path(state_dir, name);
    if !md.exists() && py.exists() {
        py
    } else {
        md
    }
}

/// A seed's notebook file: the app's `.md`, else a marimo `.py` from before, else where
/// the `.md` goes.
pub fn seed_notebook_file(h: &Home, name: &str) -> PathBuf {
    let md = crate::nbdoc::seed_path(&h.seeds_dir(), name);
    let py = h.notebook(name);
    if !md.exists() && py.exists() {
        py
    } else {
        md
    }
}

fn notebook_mtime(h: &Home, name: &str) -> Option<String> {
    let modified = std::fs::metadata(seed_notebook_file(h, name))
        .ok()?
        .modified()
        .ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(db::format_utc(secs as i64))
}

fn decorate(h: &Home, ledger: &Ledger, seed: Idea) -> Result<SeedView> {
    let planted_to = plantings(ledger, &seed)?;
    let updated = seed.updated_at.clone().unwrap_or_default();
    let last_edited = match notebook_mtime(h, seed.name.as_str()) {
        Some(m) if m > updated => m,
        _ => updated,
    };
    Ok(SeedView {
        seed_status: seed_state(&seed, !planted_to.is_empty()),
        notebook: seed_notebook_file(h, seed.name.as_str())
            .display()
            .to_string(),
        last_edited,
        planted_to,
        kept_existing_notebook: false,
        idea: seed,
    })
}

struct Fields {
    gist: Option<String>,
    hypothesis: Option<String>,
    metric: Option<String>,
}

fn check_fields(f: SeedFields<'_>) -> Result<Fields> {
    Ok(Fields {
        gist: validate_text(TextField::Gist, f.gist, false)?,
        hypothesis: validate_text(TextField::Hypothesis, f.hypothesis, false)?,
        metric: validate_text(TextField::Metric, f.metric, false)?,
    })
}

/// `arb idea new NAME "gist"` outside a repo (human only): the seed and its notebook
/// (an existing one is reported, never touched).
pub fn new_seed(
    home: &Home,
    ledger: &mut Ledger,
    name: &IdeaName,
    fields: SeedFields<'_>,
    warn: Warn<'_>,
) -> Result<SeedView> {
    let values = check_fields(fields)?;
    if idea_by_name(ledger.conn(), name.as_str())?.is_some() {
        bail!(
            "a seed named {} already exists (see `arb idea show {name}` outside any repo)",
            py_repr(name.as_str())
        );
    }
    // the app's notebook; a marimo `.py` of the same name (from before) is kept as is
    let path = seed_notebook_file(home, name.as_str());
    let existed = path.exists();
    let rel = path
        .strip_prefix(&home.root)
        .unwrap_or(&path)
        .display()
        .to_string();
    ledger.write(|tx| {
        let ts = now();
        tx.execute(
            "INSERT INTO ideas (name, gist, hypothesis, metric, stage, status, created_at, \
             updated_at) VALUES (?1, ?2, ?3, ?4, 'seed', 'open', ?5, ?5)",
            rusqlite::params![name, values.gist, values.hypothesis, values.metric, ts],
        )?;
        let row = idea_by_name(tx, name.as_str())?
            .ok_or_else(|| Error::Arb(format!("seed {name} vanished")))?;
        if let Err(why) = notebooks::seed_new(home, &row, &mut *warn)? {
            warn(&why);
        }
        tx.event(
            EventKind::SeedNew,
            name.as_str(),
            &json!({"notebook": rel, "kept_existing_notebook": existed}),
        )
    })?;
    let mut view = show_seed(home, ledger, name)?;
    view.kept_existing_notebook = existed;
    Ok(view)
}

/// `arb idea list` outside a repo: most recently edited first.
pub fn list_seeds(home: &Home, ledger: &Ledger, include_dropped: bool) -> Result<Vec<SeedView>> {
    let rows: Vec<Idea> = db::all(
        ledger.conn(),
        "SELECT * FROM ideas WHERE branch IS NULL ORDER BY id",
        [],
    )?;
    let mut out = Vec::new();
    for r in rows {
        let v = decorate(home, ledger, r)?;
        if include_dropped || v.seed_status != SeedState::Dropped {
            out.push(v);
        }
    }
    out.sort_by(|a, b| (&b.last_edited, b.idea.id).cmp(&(&a.last_edited, a.idea.id)));
    Ok(out)
}

/// `arb idea show NAME` outside a repo.
pub fn show_seed(home: &Home, ledger: &Ledger, name: &IdeaName) -> Result<SeedView> {
    decorate(home, ledger, get_seed(ledger, name)?)
}

/// `arb idea set` (human only). A no-op when nothing changes.
pub fn set_seed_fields(
    ledger: &mut Ledger,
    name: &IdeaName,
    fields: SeedFields<'_>,
) -> Result<Idea> {
    let seed = get_seed(ledger, name)?;
    let values = check_fields(fields)?;
    let mut changed: Vec<(&str, Option<String>)> = Vec::new();
    for (field, given, new, old) in [
        ("gist", fields.gist, values.gist, &seed.gist),
        (
            "hypothesis",
            fields.hypothesis,
            values.hypothesis,
            &seed.hypothesis,
        ),
        ("metric", fields.metric, values.metric, &seed.metric),
    ] {
        if given.is_some() && &new != old {
            changed.push((field, new));
        }
    }
    if changed.is_empty() {
        return Ok(seed);
    }
    ledger.write(|tx| {
        for (field, value) in &changed {
            tx.execute(
                &format!("UPDATE ideas SET {field} = ?1 WHERE id = ?2"),
                rusqlite::params![value, seed.id.0],
            )?;
        }
        tx.execute(
            "UPDATE ideas SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), seed.id.0],
        )?;
        let mut names: Vec<&str> = changed.iter().map(|(f, _)| *f).collect();
        names.sort_unstable();
        tx.event(EventKind::SeedSet, name.as_str(), &json!({"fields": names}))
    })?;
    get_seed(ledger, name)
}

/// `arb idea drop NAME` outside a repo (human only; the notebook is kept).
pub fn drop_seed(ledger: &mut Ledger, name: &IdeaName) -> Result<Idea> {
    let seed = get_seed(ledger, name)?;
    let state = seed_state(&seed, !plantings(ledger, &seed)?.is_empty());
    if state == SeedState::Dropped {
        bail!("seed {} is already dropped", py_repr(name.as_str()));
    }
    ledger.write(|tx| {
        let ts = now();
        tx.execute(
            "UPDATE ideas SET stage = 'dropped', status = 'dropped', closed_at = ?1, \
             updated_at = ?1 WHERE id = ?2",
            rusqlite::params![ts, seed.id.0],
        )?;
        tx.event(
            EventKind::SeedDrop,
            name.as_str(),
            &json!({"was": state.as_str()}),
        )
    })?;
    get_seed(ledger, name)
}

/// `arb idea revive` (human only): dropped seed → seed (planted again if it had been).
pub fn revive_seed(ledger: &mut Ledger, name: &IdeaName) -> Result<Idea> {
    let seed = get_seed(ledger, name)?;
    let state = seed_state(&seed, !plantings(ledger, &seed)?.is_empty());
    if state != SeedState::Dropped {
        bail!(
            "seed {} is not dropped (it is {} · {}); only a dropped seed can be revived",
            py_repr(name.as_str()),
            state.as_str(),
            state.label()
        );
    }
    ledger.write(|tx| {
        tx.execute(
            "UPDATE ideas SET stage = 'seed', status = 'open', closed_at = NULL, \
             updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), seed.id.0],
        )?;
        tx.event(EventKind::SeedRevive, name.as_str(), &json!({}))
    })?;
    get_seed(ledger, name)
}

/// Options of `arb idea plant`.
#[derive(Debug, Clone, Default)]
pub struct PlantOptions<'a> {
    /// Idea name in the repo (default: the seed's).
    pub idea_name: Option<&'a IdeaName>,
    pub hypothesis: Option<&'a str>,
    pub metric: Option<&'a str>,
    /// Plant a seed that is already planted.
    pub again: bool,
}

/// `arb idea plant` (human only, in the target repo): the seed becomes an idea there.
/// A failure to record the planting in the home only warns (the idea exists).
pub fn plant(
    home: &Home,
    home_ledger: &mut Ledger,
    name: &IdeaName,
    target: &RepoContext,
    opts: PlantOptions<'_>,
    warn: Warn<'_>,
) -> Result<Idea> {
    let seed = get_seed(home_ledger, name)?;
    let done = plantings(home_ledger, &seed)?;
    if seed.stage == IdeaStage::Dropped {
        bail!(
            "seed {} is dropped; revive it first (`arb idea revive {name}` outside any repo)",
            py_repr(name.as_str())
        );
    }
    if !done.is_empty() && !opts.again {
        let place = done
            .iter()
            .map(|p| format!("{} · {}", p.repo, p.idea))
            .collect::<Vec<_>>()
            .join("、");
        bail!(
            "seed {} is already planted ({place}); add --again to plant it into another repo too",
            py_repr(name.as_str())
        );
    }
    if !target.initialized() {
        bail!(
            "the repo {} is not set up for arboria yet; run `arb init` there first",
            target.main.display()
        );
    }
    let hyp = opts
        .hypothesis
        .map(str::to_string)
        .or_else(|| seed.hypothesis.clone())
        .unwrap_or_default();
    let met = opts
        .metric
        .map(str::to_string)
        .or_else(|| seed.metric.clone())
        .unwrap_or_default();
    let missing: Vec<&str> = [("假设", &hyp), ("指标", &met)]
        .into_iter()
        .filter(|(_, v)| crate::records::py_strip(v).is_empty())
        .map(|(label, _)| label)
        .collect();
    if !missing.is_empty() {
        return Err(Error::Arb(
            PLANT_NEEDS
                .replace("{name}", name.as_str())
                .replace("{missing}", &missing.join("和")),
        ));
    }
    let idea_name = opts.idea_name.unwrap_or(name).clone();
    let markdown = match notebooks::seed_markdown(&home.notebook(name.as_str()), name.as_str())? {
        Ok(m) => m,
        Err(why) => {
            warn(&format!("the seed notebook's text is not copied: {why}"));
            Vec::new()
        }
    };
    let project_id = crate::config::project_config(target)?.project_id;
    let mut ledger = Ledger::open(&target.state_dir())?;
    let idea = new_idea(
        target,
        &mut ledger,
        &idea_name,
        NewIdea {
            hypothesis: &hyp,
            metric: &met,
            parent: None,
            from_run: None,
            seed: Some(SeedOrigin {
                seed: &seed,
                home,
                markdown: &markdown,
            }),
        },
        &mut *warn,
    )?;
    let repo = target.main.display().to_string();
    let recorded = home_ledger.write(|tx| {
        let ts = now();
        tx.execute(
            "INSERT INTO plantings (seed_idea_id, repo, project_id, idea, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![seed.id.0, repo, project_id, idea_name, ts],
        )?;
        tx.execute(
            "UPDATE ideas SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![ts, seed.id.0],
        )?;
        tx.event(
            EventKind::SeedPlant,
            name.as_str(),
            &json!({"repo": repo, "project": project_id, "idea": idea_name.as_str(),
                    "markdown_cells": markdown.len()}),
        )
    });
    if let Err(e) = recorded {
        warn(&format!(
            "idea {idea_name} was created, but recording the planting in {} failed: {e}",
            home.state_dir().display()
        ));
    }
    Ok(idea)
}

/// `arb idea style` outside a repo: every seed notebook (see [`restyle`]).
pub fn restyle_seeds(home: &Home) -> Result<Vec<(PathBuf, Restyled)>> {
    let dir = home.seeds_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    notebooks::restyle(&dir, None, &[], &mut |_| {})
}

/// `arb idea migrate` in a repo: each named idea (or, with none, every idea) that has a
/// marimo notebook gets the app's `.md` next to it (linked as `notebook.md` while open).
pub fn migrate(
    ctx: &RepoContext,
    ledger: &Ledger,
    names: &[IdeaName],
    warn: Warn<'_>,
) -> Result<Vec<(String, notebooks::Migrated)>> {
    let ideas: Vec<Idea> = if names.is_empty() {
        list_ideas(ledger)?.into_iter().map(|s| s.idea).collect()
    } else {
        names
            .iter()
            .map(|n| get_idea(ledger, n))
            .collect::<Result<_>>()?
    };
    let state = ctx.state_dir();
    let mut out = Vec::new();
    for idea in ideas {
        let name = idea.name.as_str();
        let md = crate::nbdoc::idea_path(&state, name);
        let done = notebooks::migrate(&notebook_path(&state, name), &md, ("idea", name))?
            .map_err(|why| Error::Arb(format!("arb idea migrate needs the Python package (it reads the marimo file with ast): {why}")))?;
        if md.exists() && idea.stage == IdeaStage::Open {
            if let Some(wt) = idea.worktree_path.as_deref() {
                notebooks::link_md(&ctx.common_dir, Path::new(wt), &md, warn)?;
            }
        }
        out.push((name.to_string(), done));
    }
    Ok(out)
}

/// `arb idea migrate` in the seed home: the seeds' marimo notebooks.
pub fn migrate_seeds(
    home: &Home,
    ledger: &Ledger,
    names: &[IdeaName],
) -> Result<Vec<(String, notebooks::Migrated)>> {
    let seeds: Vec<IdeaName> = if names.is_empty() {
        list_seeds(home, ledger, true)?
            .into_iter()
            .map(|s| s.idea.name)
            .collect()
    } else {
        names.to_vec()
    };
    let mut out = Vec::new();
    for name in seeds {
        get_seed(ledger, &name)?;
        let n = name.as_str();
        let md = crate::nbdoc::seed_path(&home.seeds_dir(), n);
        let done = notebooks::migrate(&home.notebook(n), &md, ("seed", n))?
            .map_err(|why| Error::Arb(format!("arb idea migrate needs the Python package (it reads the marimo file with ast): {why}")))?;
        out.push((n.to_string(), done));
    }
    Ok(out)
}
