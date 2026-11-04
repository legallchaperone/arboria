//! A thin wrapper over the system `git` executable. Ports `arboria.gitutil`.
//!
//! Invariants (ADR 0001: git is the system command, not git2 / gix):
//! - `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`, `GIT_PREFIX` are
//!   scrubbed from the environment so git acts on the `cwd` we pass; stdin is /dev/null.
//! - Output is decoded as UTF-8 lossily only for messages; paths and blobs are bytes
//!   (`inspect` keeps `\r`).
//! - A user without a git identity gets [`FALLBACK_IDENTITY`] for hidden refs and the
//!   backup repo only.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::Path;

use crate::Result;

/// Variables that would redirect git away from the `cwd` we pass.
pub const SCRUBBED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
];

/// Identity used when the user has none configured.
pub const FALLBACK_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "arboria"),
    ("GIT_AUTHOR_EMAIL", "arboria@localhost"),
    ("GIT_COMMITTER_NAME", "arboria"),
    ("GIT_COMMITTER_EMAIL", "arboria@localhost"),
];

/// What a git run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    /// stdout as UTF-8 (lossy), trimmed like Python's `strip=True`.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }
}

/// Run `git <args>` in `cwd` with extra env; `Error::Git` on failure when `check`.
pub fn run(args: &[&str], cwd: &Path, env: &[(&str, &str)], check: bool) -> Result<Output> {
    todo!("port of arboria.gitutil.run")
}

/// Run git and return its trimmed stdout.
pub fn out(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Result<String> {
    todo!("port of arboria.gitutil.out")
}

/// Run git; true when it exits 0 (never `Error::Git`).
pub fn ok(args: &[&str], cwd: &Path) -> Result<bool> {
    todo!("port of arboria.gitutil.ok")
}

/// Resolve `rev` to a commit hash (`rev-parse --verify --quiet <rev>^{commit}`).
pub fn rev_parse(rev: &str, cwd: &Path) -> Result<Option<String>> {
    todo!("port of arboria.gitutil.rev_parse")
}

/// `show-ref --verify --quiet <ref>`.
pub fn ref_exists(git_ref: &str, cwd: &Path) -> Result<bool> {
    todo!("port of arboria.gitutil.ref_exists")
}

/// [`FALLBACK_IDENTITY`] if the user has no git identity, else empty.
pub fn identity_env(cwd: &Path) -> Result<Vec<(&'static str, &'static str)>> {
    todo!("port of arboria.gitutil.identity_env")
}
