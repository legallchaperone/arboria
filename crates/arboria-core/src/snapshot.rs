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

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::Path;

use serde::Serialize;

use crate::{Result, Warn};

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

/// Warnings for untracked files that are individually or collectively too large.
pub fn size_warnings(
    worktree: &Path,
    paths: &[String],
    file_limit: u64,
    total_limit: u64,
) -> Vec<String> {
    todo!("port of arboria.snapshot.size_warnings")
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
    todo!("port of arboria.snapshot.write_tree")
}

/// A recorded snapshot commit with this tree that still exists.
pub fn find_existing_commit(
    conn: &rusqlite::Connection,
    tree: &str,
    worktree: &Path,
) -> Result<Option<String>> {
    todo!("port of arboria.snapshot.find_existing_commit")
}

/// `git commit-tree --no-gpg-sign [-p parent] -m message tree`.
pub fn commit_tree(
    worktree: &Path,
    tree: &str,
    parent: Option<&str>,
    message: &str,
) -> Result<String> {
    todo!("port of arboria.snapshot.commit_tree")
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
    todo!("port of arboria.snapshot.snapshot")
}

/// `git update-ref -m reason ref commit`.
pub fn update_ref(worktree: &Path, git_ref: &str, commit: &str, reason: &str) -> Result<()> {
    todo!("port of arboria.snapshot.update_ref")
}
