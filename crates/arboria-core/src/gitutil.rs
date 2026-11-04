//! A thin wrapper over the system `git` executable. Ports `arboria.gitutil`.
//!
//! Invariants (ADR 0001: git is the system command, not git2 / gix):
//! - `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`, `GIT_PREFIX` are
//!   scrubbed from the environment so git acts on the `cwd` we pass; stdin is /dev/null.
//! - Output is decoded as UTF-8 lossily only for messages; paths and blobs are bytes
//!   (`inspect` keeps `\r`).
//! - A user without a git identity gets [`FALLBACK_IDENTITY`] for hidden refs and the
//!   backup repo only.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::{Error, Result};

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
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in SCRUBBED_ENV {
        cmd.env_remove(var);
    }
    cmd.envs(env.iter().copied());
    let out = cmd
        .output()
        .map_err(|e| Error::Arb(format!("cannot run git in {}: {e}", cwd.display())))?;
    let status = out.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if check && status != 0 {
        return Err(Error::Git {
            args: args.join(" "),
            cwd: cwd.to_path_buf(),
            code: status,
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(Output {
        status,
        stdout: out.stdout,
        stderr,
    })
}

/// Run git and return its trimmed stdout.
pub fn out(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Result<String> {
    Ok(run(args, cwd, env, true)?.text())
}

/// Run git; true when it exits 0 (never `Error::Git`).
pub fn ok(args: &[&str], cwd: &Path) -> Result<bool> {
    Ok(run(args, cwd, &[], false)?.status == 0)
}

/// Resolve `rev` to a commit hash (`rev-parse --verify --quiet <rev>^{commit}`).
pub fn rev_parse(rev: &str, cwd: &Path) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    let out = run(
        &["rev-parse", "--verify", "--quiet", &spec],
        cwd,
        &[],
        false,
    )?;
    Ok((out.status == 0).then(|| out.text()))
}

/// `show-ref --verify --quiet <ref>`.
pub fn ref_exists(git_ref: &str, cwd: &Path) -> Result<bool> {
    ok(&["show-ref", "--verify", "--quiet", git_ref], cwd)
}

/// [`FALLBACK_IDENTITY`] if the user has no git identity, else empty.
pub fn identity_env(cwd: &Path) -> Result<Vec<(&'static str, &'static str)>> {
    if ok(&["var", "GIT_COMMITTER_IDENT"], cwd)? && ok(&["var", "GIT_AUTHOR_IDENT"], cwd)? {
        Ok(Vec::new())
    } else {
        Ok(FALLBACK_IDENTITY.to_vec())
    }
}
