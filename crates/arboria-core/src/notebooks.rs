//! Writing the notebooks of ideas and seeds. New notebooks are the app's markdown
//! notebooks (ADR 0005), written here; the rest of this module is the bridge to the
//! Python package for the marimo files of ideas from before (ADR 0002).
//!
//! For the marimo files the work is on the Python kernel side: its template, the stylesheet with the embedded fonts (`arboria.style`, the font
//! files ship in the Python package) and the `ast` reading of seed notebooks are
//! `arboria.notebooks`. The core runs them in the interpreter [`crate::commands::python_exe`]
//! (`$ARBORIA_PYTHON`, else the interpreter of `arb-py`) with `-P` (the current directory is not importable), one JSON request on stdin, one
//! JSON reply on stdout, the same way `arb ui` runs the dashboard.
//!
//! Invariants:
//! - The notebook is the human's: an existing file is never overwritten (`write_new`),
//!   restyling changes only the `marimo.App(...)` line (`notebooks.restyle`).
//! - Notebook code is never executed: seed notebooks are read with `ast`.
//! - An interpreter that cannot import `arboria` costs the notebook only: the caller gets
//!   [`Bridge::Unavailable`] and warns (the idea or seed is still created); `arb idea
//!   notebook` writes it later. Errors the Python side raises as `ArbError` (a link that
//!   cannot be made) are errors here too.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::{json, Value};

use crate::ideas::Restyled;
use crate::model::{Idea, Run};
use crate::paths::Home;
use crate::{nbdoc, Error, Result, Warn};

/// The request handler, run as `python -P -c BRIDGE`.
const BRIDGE: &str = r#"
import json, sys
try:
    from pathlib import Path
    from arboria import notebooks, paths
    from arboria.errors import ArbError
except ImportError as exc:
    print(json.dumps({"unavailable": f"{sys.executable} cannot import arboria ({exc})"}))
    sys.exit(0)
req = json.loads(sys.stdin.read())
warnings = []
op = req["op"]
try:
    if op == "create":
        ctx = paths.locate(Path(req["main"]))
        out = str(notebooks.create(ctx, req["idea"], req["parent"], req["from_run"],
                                   warn=warnings.append, link=req["link"], seed=req["seed"]))
    elif op == "seed_new":
        path = Path(req["path"])
        out = False
        if not path.exists():
            out = notebooks.write_new(path, notebooks.render_seed(req["seed"]))
        notebooks.ensure_style(path.parent)
    elif op == "seed_markdown":
        try:
            out = notebooks.seed_markdown(Path(req["path"]).read_text(encoding="utf-8"),
                                          req["name"])
        except (OSError, UnicodeDecodeError):
            out = []
    elif op == "restyle":
        d = Path(req["dir"])
        out = []
        if d.is_dir():
            notebooks.ensure_style(d, warnings.append)
            if req["main"] is not None:
                ctx = paths.locate(Path(req["main"]))
                for wt in req["worktrees"]:
                    notebooks.link_style(ctx, Path(wt), warnings.append)
            out = [[str(p), notebooks.restyle(p)] for p in sorted(d.glob("*.py")) if p.is_file()]
    elif op == "migrate":
        from arboria import migrate
        out = migrate.convert(Path(req["path"]).read_text(encoding="utf-8"))
    else:
        raise ArbError(f"unknown notebook request {op!r}")
except ArbError as exc:
    print(json.dumps({"error": str(exc), "warnings": warnings}))
    sys.exit(0)
print(json.dumps({"result": out, "warnings": warnings}))
"#;

/// What the Python side answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Bridge {
    Done(Value),
    /// No interpreter, or it cannot import `arboria`: why.
    Unavailable(String),
}

/// Run one request; the Python side's warnings go to `warn`.
fn call(req: &Value, warn: Warn<'_>) -> Result<Bridge> {
    let exe = crate::commands::python_exe();
    let child = Command::new(&exe)
        .args(["-P", "-c", BRIDGE])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Ok(Bridge::Unavailable(format!("cannot run {exe} ({e})"))),
    };
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(req.to_string().as_bytes())?;
    }
    let out = child.wait_with_output()?;
    let reply: Option<Value> = std::str::from_utf8(&out.stdout)
        .ok()
        .and_then(|s| s.lines().last().map(str::to_string))
        .and_then(|l| serde_json::from_str(&l).ok());
    let Some(reply) = reply.filter(|_| out.status.success()) else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr.trim_end().lines().last().unwrap_or("no output");
        return Err(Error::Arb(format!(
            "writing the notebook failed in {exe}: {last}"
        )));
    };
    if let Some(why) = reply["unavailable"].as_str() {
        return Ok(Bridge::Unavailable(why.to_string()));
    }
    for w in reply["warnings"].as_array().into_iter().flatten() {
        warn(w.as_str().unwrap_or_default());
    }
    if let Some(e) = reply["error"].as_str() {
        return Err(Error::Arb(e.to_string()));
    }
    Ok(Bridge::Done(reply["result"].clone()))
}

/// The warning when there is no notebook (`then` says how to get it later).
pub fn unavailable_warning(why: &str, then: &str) -> String {
    format!(
        "no notebook written: {why}; install the Python package (`uv tool install '.[ui]'` in the arboria checkout puts arb-py on PATH) or set ARBORIA_PYTHON, then {then}"
    )
}

/// The seed an idea grows from, as its notebook header shows it.
#[derive(Debug, Clone)]
pub struct SeedNote {
    pub name: String,
    pub notebook: PathBuf,
    /// The seed notebook's markdown cells ([`seed_markdown`]).
    pub markdown: Vec<String>,
    pub home: PathBuf,
}

/// The idea's notebook: the app's markdown notebook (ADR 0005), never overwriting one,
/// linked into the worktree as `notebook.md` when `link`. The written (or existing) path.
/// The `Result` inside is kept for the callers' "no notebook" path; writing markdown needs
/// no Python, so it is always `Ok`.
pub fn create(
    main: &Path,
    idea: &Idea,
    parent: Option<&str>,
    from_run: Option<&Run>,
    seed: Option<&SeedNote>,
    link: bool,
    warn: Warn<'_>,
) -> Result<std::result::Result<PathBuf, String>> {
    let ctx = crate::paths::locate(Some(main))?;
    let path = nbdoc::idea_path(&ctx.state_dir(), idea.name.as_str());
    let seed_text = seed.map(|s| s.markdown.join("\n\n"));
    let text = nbdoc::idea_notebook(&nbdoc::IdeaStart {
        idea: idea.name.as_str(),
        hypothesis: idea.hypothesis.as_deref().unwrap_or(""),
        metric: idea.metric.as_deref().unwrap_or(""),
        parent,
        from_run: from_run.map(|r| r.id.as_str()),
        seed: seed.map(|s| (s.name.as_str(), seed_text.as_deref().unwrap_or(""))),
    });
    nbdoc::write_new(&path, &text)?;
    if link {
        if let Some(wt) = idea.worktree_path.as_deref() {
            link_md(&ctx.common_dir, Path::new(wt), &path, warn)?;
        }
    }
    Ok(Ok(path))
}

/// Repair the worktree links of an existing marimo notebook (`notebook.py`, the
/// stylesheet) through the Python package, as before ADR 0005.
pub fn link_marimo(
    main: &Path,
    idea: &Idea,
    warn: Warn<'_>,
) -> Result<std::result::Result<PathBuf, String>> {
    let req = json!({
        "op": "create",
        "main": main,
        "idea": idea,
        "parent": null,
        "from_run": null,
        "seed": null,
        "link": true,
    });
    Ok(match call(&req, warn)? {
        Bridge::Done(v) => Ok(PathBuf::from(v.as_str().unwrap_or_default())),
        Bridge::Unavailable(why) => Err(why),
    })
}

/// `<worktree>/notebook.md` → the notebook (excluded from git). An existing file there
/// is left alone (warned).
pub fn link_md(common_dir: &Path, worktree: &Path, path: &Path, warn: Warn<'_>) -> Result<()> {
    if !worktree.is_dir() {
        return Ok(());
    }
    crate::paths::ensure_excluded(
        common_dir,
        &format!("/{}", nbdoc::LINK_NAME),
        "arboria notebook link (local only)",
    )?;
    let link = worktree.join(nbdoc::LINK_NAME);
    if link.is_symlink() && link.canonicalize().ok() == path.canonicalize().ok() {
        return Ok(());
    }
    if link.exists() || link.is_symlink() {
        warn(&format!(
            "{} already exists (tracked in the repo?); not linking the notebook. Open {} instead",
            link.display(),
            path.display()
        ));
        return Ok(());
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(path, &link).map_err(|e| {
        Error::Arb(format!(
            "cannot link {} -> {}: {e}",
            link.display(),
            path.display()
        ))
    })?;
    Ok(())
}

/// A seed's notebook `<home>/seeds/<name>.md` (unless it, or a marimo `.py` of the same
/// seed, exists): whether it was written.
pub fn seed_new(
    home: &Home,
    seed: &Idea,
    _warn: Warn<'_>,
) -> Result<std::result::Result<bool, String>> {
    let name = seed.name.as_str();
    if home.notebook(name).exists() {
        return Ok(Ok(false));
    }
    let path = nbdoc::seed_path(&home.seeds_dir(), name);
    let text = nbdoc::seed_notebook(
        name,
        seed.gist.as_deref().unwrap_or(""),
        seed.hypothesis.as_deref(),
        seed.metric.as_deref(),
    );
    Ok(Ok(nbdoc::write_new(&path, &text)?))
}

/// A seed notebook's text without what [`nbdoc::seed_notebook`] generated (the title,
/// the hypothesis and metric lines, which the idea's own header has, and the
/// placeholder sentence); one string per text part that is left.
pub fn seed_thoughts(doc: &nbdoc::Doc, name: &str) -> Vec<String> {
    let title = format!("# {name}");
    let mut out = Vec::new();
    for p in &doc.parts {
        let nbdoc::Part::Text { text } = p else {
            continue;
        };
        let kept: Vec<&str> = text
            .lines()
            .filter(|l| {
                let t = l.trim();
                t != title
                    && !t.starts_with("假设：")
                    && !t.starts_with("指标：")
                    && t != nbdoc::SEED_PLACEHOLDER
                    && t != "## 思路"
            })
            .collect();
        let mut joined = kept.join("\n");
        while joined.contains("\n\n\n") {
            joined = joined.replace("\n\n\n", "\n\n");
        }
        let joined = joined.trim().to_string();
        if !joined.is_empty() {
            out.push(joined);
        }
    }
    out
}

/// The text a seed passes on when planted: the `.md` notebook's text parts (without its
/// first, generated part), else the markdown cells of a marimo `.py` (read with `ast` by
/// the Python package); empty when there is no notebook.
pub fn seed_markdown(path: &Path, name: &str) -> Result<std::result::Result<Vec<String>, String>> {
    let md = path.with_extension(nbdoc::EXT);
    if md.is_file() {
        let doc = nbdoc::parse(&std::fs::read_to_string(&md)?);
        return Ok(Ok(seed_thoughts(&doc, name)));
    }
    if !path.is_file() {
        return Ok(Ok(Vec::new()));
    }
    let req = json!({"op": "seed_markdown", "path": path, "name": name});
    Ok(match call(&req, &mut |_| {})? {
        Bridge::Done(v) => Ok(serde_json::from_value(v)?),
        Bridge::Unavailable(why) => Err(why),
    })
}

/// `notebooks.restyle` of every `*.py` in `dir` after refreshing its stylesheet (and, for
/// a repo, linking it into each open idea's worktree).
pub fn restyle(
    dir: &Path,
    main: Option<&Path>,
    worktrees: &[String],
    warn: Warn<'_>,
) -> Result<Vec<(PathBuf, Restyled)>> {
    let req = json!({"op": "restyle", "dir": dir, "main": main, "worktrees": worktrees});
    match call(&req, warn)? {
        Bridge::Done(v) => {
            let rows: Vec<(PathBuf, String)> = serde_json::from_value(v)?;
            Ok(rows
                .into_iter()
                .map(|(p, how)| {
                    let how = match how.as_str() {
                        "added" => Restyled::Added,
                        "already" => Restyled::Already,
                        "own" => Restyled::Own,
                        _ => Restyled::Unknown,
                    };
                    (p, how)
                })
                .collect())
        }
        Bridge::Unavailable(why) => Err(Error::Arb(format!(
            "arb idea style needs the Python package (the notebooks are marimo files): {why}; install it (`uv tool install '.[ui]'` in the arboria checkout puts arb-py on PATH) or set ARBORIA_PYTHON"
        ))),
    }
}

/// What `arb idea migrate` did with one notebook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Migrated {
    /// Written from the marimo file.
    Written,
    /// The `.md` exists already: left alone.
    Already,
    /// No marimo file to migrate.
    NoMarimo,
}

/// The marimo notebook `py` as the app's notebook `md` (never overwriting one); the
/// marimo file stays. `front` is the frontmatter key and name (`idea`/`seed`).
pub fn migrate(
    py: &Path,
    md: &Path,
    front: (&str, &str),
) -> Result<std::result::Result<Migrated, String>> {
    if md.exists() {
        return Ok(Ok(Migrated::Already));
    }
    if !py.is_file() {
        return Ok(Ok(Migrated::NoMarimo));
    }
    let req = json!({"op": "migrate", "path": py});
    let parts = match call(&req, &mut |_| {})? {
        Bridge::Done(v) => v,
        Bridge::Unavailable(why) => return Ok(Err(why)),
    };
    let mut doc = nbdoc::Doc::default();
    doc.set("arboria", &nbdoc::FORMAT.to_string());
    doc.set(front.0, front.1);
    for p in parts.as_array().into_iter().flatten() {
        let text = p["text"].as_str().or(p["code"].as_str()).unwrap_or("");
        doc.parts.push(if p["kind"] == "text" {
            nbdoc::Part::Text {
                text: text.to_string(),
            }
        } else {
            nbdoc::Part::Block(nbdoc::Block {
                id: String::new(),
                code: text.to_string(),
            })
        });
    }
    nbdoc::ensure_ids(&mut doc);
    nbdoc::write_new(md, &nbdoc::render(&doc))?;
    Ok(Ok(Migrated::Written))
}
