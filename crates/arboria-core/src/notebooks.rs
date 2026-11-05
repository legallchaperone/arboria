//! The marimo notebooks of ideas and seeds, written by the Python package (ADR 0002).
//!
//! Until the app's markdown notebooks (ADR 0001) the notebook lives on the Python kernel
//! side: its template, the stylesheet with the embedded fonts (`arboria.style`, the font
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

use serde_json::{json, Value};

use crate::ideas::Restyled;
use crate::model::{Idea, Run};
use crate::paths::Home;
use crate::{Error, Result, Warn};

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

/// `notebooks.create`: the idea's notebook (never overwriting one) and stylesheet, linked
/// into the worktree when `link`. The written path, or why there is none.
pub fn create(
    main: &Path,
    idea: &Idea,
    parent: Option<&str>,
    from_run: Option<&Run>,
    seed: Option<&SeedNote>,
    link: bool,
    warn: Warn<'_>,
) -> Result<std::result::Result<PathBuf, String>> {
    let req = json!({
        "op": "create",
        "main": main,
        "idea": idea,
        "parent": parent,
        "from_run": from_run.map(|r| json!({"id": r.id, "snapshot_commit": r.snapshot_commit})),
        "seed": seed.map(|s| json!({"name": s.name, "notebook": s.notebook,
                                    "markdown": s.markdown, "home": s.home})),
        "link": link,
    });
    Ok(match call(&req, warn)? {
        Bridge::Done(v) => Ok(PathBuf::from(v.as_str().unwrap_or_default())),
        Bridge::Unavailable(why) => Err(why),
    })
}

/// A seed's notebook `<home>/seeds/<name>.py` (unless it exists) and the stylesheet next
/// to it: whether it was written, or why there is none.
pub fn seed_new(
    home: &Home,
    seed: &Idea,
    warn: Warn<'_>,
) -> Result<std::result::Result<bool, String>> {
    let req = json!({"op": "seed_new", "path": home.notebook(seed.name.as_str()), "seed": seed});
    Ok(match call(&req, warn)? {
        Bridge::Done(v) => Ok(v.as_bool().unwrap_or(false)),
        Bridge::Unavailable(why) => Err(why),
    })
}

/// The markdown cells of a seed notebook (without its generated header), read with `ast`;
/// empty when the file is missing or unreadable, or why the Python side is unavailable.
pub fn seed_markdown(path: &Path, name: &str) -> Result<std::result::Result<Vec<String>, String>> {
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
