//! Errors. Port of `arboria.errors` (plus the error classes scattered over the Python
//! modules: `GitError`, `BackendError`, `NeedLogin`, `NotHumanError`,
//! `LargeUntrackedError`).
//!
//! Invariant: every variant's `Display` is the user-facing message, printed by the CLI as
//! `error: <message>` with exit status 1 (Python `cli.handle_errors`). Messages are kept
//! identical to Python's where Python has one, so docs and muscle memory carry over.

use std::path::PathBuf;

/// Result alias used across the core.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything the core can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `ArbError`: a refusal or a user mistake, with a message saying what to do.
    #[error("{0}")]
    Arb(String),
    /// `NotARepoError`: the directory is not inside a git repository.
    #[error("{0}")]
    NotARepo(String),
    /// `NotInitializedError`: the repository has no `.arboria/ledger.db` (`arb init`).
    #[error("{0}")]
    NotInitialized(String),
    /// `human.NotHumanError`: an approval outside the human's own terminal.
    #[error("{0}")]
    NotHuman(String),
    /// `snapshot.LargeUntrackedError`: untracked files over the limits, no `--allow-large`.
    #[error("{0}")]
    LargeUntracked(String),
    /// `backends.BackendError`: a remote operation failed; the ledger stays resumable.
    #[error("{0}")]
    Backend(String),
    /// `backends.NeedLogin`: no live ssh master. Never retried (each try is a Duo push).
    #[error("{0}")]
    NeedLogin(String),
    /// `gitutil.GitError`: `git <args>` exited non-zero.
    #[error("`git {args}` failed in {cwd} (exit {code}){detail}", detail = if stderr.is_empty() { String::new() } else { format!(": {stderr}") })]
    Git {
        /// The git arguments, space-joined.
        args: String,
        /// Where git ran.
        cwd: PathBuf,
        /// Exit status (-1 when killed by a signal).
        code: i32,
        /// Trimmed stderr.
        stderr: String,
    },
    /// The SQLite ledger.
    #[error("ledger: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// File system.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization of ledger columns or answers.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// Shorthand for [`Error::Arb`].
    pub fn arb(msg: impl Into<String>) -> Self {
        Error::Arb(msg.into())
    }

    /// The Python exception class this corresponds to (test vectors write errors as
    /// `<class>: <message>`).
    pub fn python_class(&self) -> &'static str {
        match self {
            Error::Arb(_) => "ArbError",
            Error::NotARepo(_) => "NotARepoError",
            Error::NotInitialized(_) => "NotInitializedError",
            Error::NotHuman(_) => "NotHumanError",
            Error::LargeUntracked(_) => "LargeUntrackedError",
            Error::Backend(_) => "BackendError",
            Error::NeedLogin(_) => "NeedLogin",
            Error::Git { .. } => "GitError",
            Error::Sqlite(_) => "sqlite3.Error",
            Error::Io(_) => "OSError",
            Error::Json(_) => "ValueError",
        }
    }
}

/// `return Err(Error::Arb(format!(...)))`.
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::Error::Arb(format!($($arg)*)))
    };
}
