//! The projects the app and the helper look at (ADR 0004): `<config dir>/projects.json`
//! (`$ARBORIA_CONFIG_DIR`, else `~/.config/arboria`). New in the Rust core; Python has no
//! counterpart (the marimo dashboard shows one repo).
//!
//! Invariants:
//! - Only main worktrees of initialized repos are added ([`add`] resolves the path with
//!   [`crate::paths::locate`]); the seed home is never listed: it is always shown.
//! - Writes take `<config dir>/projects.lock` and replace the file atomically (temp file +
//!   rename), so the app, the CLI and the helper can add at the same time.
//! - A listed repo that disappeared stays listed (the views show it as an error line);
//!   only [`remove`] takes it out.

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{self, RepoContext};
use crate::{Error, Result};

/// The file under the config dir.
pub const FILENAME: &str = "projects.json";

/// One listed project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The main worktree, absolute.
    pub path: PathBuf,
    pub added_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileData {
    #[serde(default)]
    projects: Vec<Entry>,
}

/// `<config dir>/projects.json`.
pub fn path() -> PathBuf {
    crate::clusters::config_dir().join(FILENAME)
}

fn read_at(file: &Path) -> Result<FileData> {
    match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| Error::Arb(format!("{}: not a projects list ({e})", file.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileData::default()),
        Err(e) => Err(e.into()),
    }
}

/// The listed projects, in the order they were added.
pub fn list() -> Result<Vec<Entry>> {
    Ok(read_at(&path())?.projects)
}

fn locked<T>(f: impl FnOnce(&mut FileData) -> Result<(bool, T)>) -> Result<T> {
    let file = path();
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir)?;
    let lock = File::create(dir.join("projects.lock"))?;
    lock.lock()?;
    let mut data = read_at(&file)?;
    let (changed, out) = f(&mut data)?;
    if changed {
        let tmp = dir.join(format!(".{FILENAME}.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(&data)? + "\n")?;
        std::fs::rename(&tmp, &file)?;
    }
    Ok(out)
}

/// Add the initialized repo at (or containing) `dir`, by its main worktree. Returns the
/// entry and whether it was new.
pub fn add(dir: &Path) -> Result<(Entry, bool)> {
    let ctx = paths::locate(Some(dir))?.require_initialized()?;
    add_context(&ctx)
}

/// [`add`] for a located repo (`arb init` calls it after initializing).
pub fn add_context(ctx: &RepoContext) -> Result<(Entry, bool)> {
    let main = ctx.main.clone();
    locked(|data| {
        if let Some(e) = data.projects.iter().find(|e| e.path == main) {
            return Ok((false, (e.clone(), false)));
        }
        let entry = Entry {
            path: main,
            added_at: crate::db::now(),
        };
        data.projects.push(entry.clone());
        Ok((true, (entry, true)))
    })
}

/// Take `path` (a listed main worktree, or a directory inside one) out of the list.
/// Returns whether it was listed.
pub fn remove(path: &Path) -> Result<bool> {
    let target = paths::locate(Some(path))
        .map(|c| c.main)
        .unwrap_or_else(|_| paths::resolve(path));
    locked(|data| {
        let before = data.projects.len();
        data.projects.retain(|e| e.path != target);
        let changed = data.projects.len() != before;
        Ok((changed, changed))
    })
}
