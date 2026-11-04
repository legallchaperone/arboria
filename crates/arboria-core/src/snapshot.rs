//! Snapshot a worktree's exact state (uncommitted and untracked files included) as a
//! hidden commit. Ports `arboria.snapshot`.
//!
//! Invariants (the Python contract is `tests/test_snapshot.py`):
//! - The user's branch, index and working tree are never touched: the tree is built in a
//!   throw-away index (`GIT_INDEX_FILE` in a temp dir: read-tree HEAD → add -A →
//!   write-tree) and the commit is made with `commit-tree`; callers anchor it under
//!   `refs/arboria/...`.
//! - Ignored files are not included; untracked files over 50 MB each or 200 MB in total
//!   are refused unless `allow_large`.
//! - An identical tree reuses an earlier run's commit (if it still exists).

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{gitutil, Error, Result, Warn};

/// 1 MiB.
pub const MB: u64 = 1024 * 1024;
/// Per-file limit for untracked files.
pub const DEFAULT_FILE_LIMIT: u64 = 50 * MB;
/// Total limit for untracked files.
pub const DEFAULT_TOTAL_LIMIT: u64 = 200 * MB;

/// A snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotResult {
    pub tree: String,
    pub commit: String,
    /// An earlier run's commit with the same tree was reused.
    pub reused: bool,
    pub warnings: Vec<String>,
}

fn fmt_mb(n: u64) -> String {
    format!("{:.1} MB", n as f64 / MB as f64)
}

/// Warnings for untracked files that are individually or collectively too large.
pub fn size_warnings(
    worktree: &Path,
    paths: &[String],
    file_limit: u64,
    total_limit: u64,
) -> Vec<String> {
    let paths: Vec<&std::ffi::OsStr> = paths.iter().map(|p| p.as_ref()).collect();
    size_warnings_os(worktree, &paths, file_limit, total_limit)
}

/// [`size_warnings`] on raw paths (a non-UTF-8 name is still measured).
fn size_warnings_os(
    worktree: &Path,
    paths: &[&std::ffi::OsStr],
    file_limit: u64,
    total_limit: u64,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut total = 0u64;
    for rel in paths {
        let Ok(meta) = std::fs::symlink_metadata(worktree.join(rel)) else {
            continue;
        };
        let rel = rel.to_string_lossy();
        let size = meta.len();
        total += size;
        if size > file_limit {
            warnings.push(format!(
                "large untracked file {rel} ({} > {})",
                fmt_mb(size),
                fmt_mb(file_limit)
            ));
        }
    }
    if total > total_limit {
        warnings.push(format!(
            "untracked files total {} (> {})",
            fmt_mb(total),
            fmt_mb(total_limit)
        ));
    }
    warnings
}

/// A private (0700) temp dir for the throw-away index, removed on drop.
struct IndexDir(PathBuf);

impl IndexDir {
    fn new() -> Result<IndexDir> {
        use std::os::unix::fs::DirBuilderExt;
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        for _ in 0..100 {
            let dir = base.join(format!(
                "arboria-index-{}-{}-{nanos:x}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => return Ok(IndexDir(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::arb("cannot create a temporary index directory"))
    }
}

impl Drop for IndexDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `(tree, HEAD commit or None, warnings)` of the worktree's current state, built in a
/// temporary index. `Error::LargeUntracked` over the limits without `allow_large`.
pub fn write_tree(
    worktree: &Path,
    allow_large: bool,
    file_limit: Option<u64>,
    total_limit: Option<u64>,
    warn: Warn<'_>,
) -> Result<(String, Option<String>, Vec<String>)> {
    let file_limit = file_limit.unwrap_or(DEFAULT_FILE_LIMIT);
    let total_limit = total_limit.unwrap_or(DEFAULT_TOTAL_LIMIT);
    let head = gitutil::rev_parse("HEAD", worktree)?;
    let tmp = IndexDir::new()?;
    let index = tmp.0.join("index");
    let index = index.to_str().ok_or_else(|| {
        Error::arb(format!(
            "temporary directory {} is not valid UTF-8",
            tmp.0.display()
        ))
    })?;
    let env = [("GIT_INDEX_FILE", index)];
    match &head {
        Some(h) => gitutil::run(&["read-tree", h], worktree, &env, true)?,
        None => gitutil::run(&["read-tree", "--empty"], worktree, &env, true)?,
    };
    // files `add -A` would newly add: not in HEAD and not ignored
    let listing = gitutil::run(
        &["ls-files", "-z", "--others", "--exclude-standard"],
        worktree,
        &env,
        true,
    )?
    .stdout;
    let untracked: Vec<&std::ffi::OsStr> = listing
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(std::ffi::OsStr::from_bytes)
        .collect();
    let warnings = size_warnings_os(worktree, &untracked, file_limit, total_limit);
    for w in &warnings {
        warn(w);
    }
    if !warnings.is_empty() && !allow_large {
        return Err(Error::LargeUntracked(format!(
            "refusing to snapshot: {}. Add them to .gitignore, or pass --allow-large.",
            warnings.join("; ")
        )));
    }
    gitutil::run(&["add", "-A", "--", "."], worktree, &env, true)?;
    let tree = gitutil::out(&["write-tree"], worktree, &env)?;
    Ok((tree, head, warnings))
}

/// A recorded snapshot commit with this tree that still exists.
pub fn find_existing_commit(
    conn: &rusqlite::Connection,
    tree: &str,
    worktree: &Path,
) -> Result<Option<String>> {
    let mut stmt = conn
        .prepare("SELECT snapshot_commit FROM runs WHERE tree_hash = ? ORDER BY created_at, id")?;
    let commits = stmt
        .query_map([tree], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for commit in commits {
        if gitutil::ok(
            &["cat-file", "-e", &format!("{commit}^{{commit}}")],
            worktree,
        )? {
            return Ok(Some(commit));
        }
    }
    Ok(None)
}

/// `git commit-tree --no-gpg-sign [-p parent] -m message tree`.
pub fn commit_tree(
    worktree: &Path,
    tree: &str,
    parent: Option<&str>,
    message: &str,
) -> Result<String> {
    let mut args = vec!["commit-tree", "--no-gpg-sign"];
    if let Some(p) = parent.filter(|p| !p.is_empty()) {
        args.extend(["-p", p]);
    }
    args.extend(["-m", message, tree]);
    gitutil::out(&args, worktree, &gitutil::identity_env(worktree)?)
}

/// Snapshot `worktree`, reusing an earlier run's commit when the tree is identical.
pub fn snapshot(
    worktree: &Path,
    conn: &rusqlite::Connection,
    message: &str,
    allow_large: bool,
    file_limit: Option<u64>,
    total_limit: Option<u64>,
    warn: Warn<'_>,
) -> Result<SnapshotResult> {
    let (tree, head, warnings) = write_tree(worktree, allow_large, file_limit, total_limit, warn)?;
    if let Some(commit) = find_existing_commit(conn, &tree, worktree)? {
        return Ok(SnapshotResult {
            tree,
            commit,
            reused: true,
            warnings,
        });
    }
    let commit = commit_tree(worktree, &tree, head.as_deref(), message)?;
    Ok(SnapshotResult {
        tree,
        commit,
        reused: false,
        warnings,
    })
}

/// `git update-ref -m reason ref commit`.
pub fn update_ref(worktree: &Path, git_ref: &str, commit: &str, reason: &str) -> Result<()> {
    gitutil::run(
        &["update-ref", "-m", reason, git_ref, commit],
        worktree,
        &[],
        true,
    )?;
    Ok(())
}
