//! Where things are: the repo context (main worktree, shared `.arboria/` state dir), the
//! seed home (`~/arboria` or `$ARBORIA_HOME`), the per-user cache dir. Ports
//! `arboria.paths`.
//!
//! Invariants:
//! - The state dir is `<main worktree>/.arboria`, shared by every worktree of the repo;
//!   it is excluded from git via `<common dir>/info/exclude`, never `.gitignore`.
//! - A home is not a git repo; its state dir is told from a project's by
//!   `.arboria/seeds/` (the backup mirror of the seed notebooks).
//! - Tests never touch the real `~/arboria`: they set `ARBORIA_HOME`.
//!
//! The pure accessors are real; everything that runs git or touches files is stubbed.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::{Path, PathBuf};

use crate::Result;

/// `.arboria`.
pub const STATE_DIR_NAME: &str = ".arboria";
/// `arboria.toml` in the main worktree.
pub const CONFIG_NAME: &str = "arboria.toml";
/// Overrides the home root.
pub const HOME_ENV: &str = "ARBORIA_HOME";
/// `seeds/` in a home.
pub const SEEDS_DIR: &str = "seeds";
/// Overrides the cache dir.
pub const CACHE_ENV: &str = "ARBORIA_CACHE_DIR";

/// Where we are: the current worktree and the repo-wide state location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoContext {
    /// The directory the command runs in (resolved).
    pub cwd: PathBuf,
    /// The shared git dir (`.git` of the main worktree).
    pub common_dir: PathBuf,
    /// The main worktree's root.
    pub main: PathBuf,
    /// Root of the worktree containing `cwd` (None inside `.git` etc.).
    pub toplevel: Option<PathBuf>,
}

impl RepoContext {
    /// `<main>/.arboria`.
    pub fn state_dir(&self) -> PathBuf {
        self.main.join(STATE_DIR_NAME)
    }

    /// `<main>/.arboria/ledger.db`.
    pub fn db_path(&self) -> PathBuf {
        self.state_dir().join(crate::db::DB_NAME)
    }

    /// `<main>/arboria.toml`.
    pub fn config_path(&self) -> PathBuf {
        self.main.join(CONFIG_NAME)
    }

    /// Whether `arb init` has run (the ledger exists).
    pub fn initialized(&self) -> bool {
        self.db_path().is_file()
    }

    /// Whether `cwd` is in the main worktree.
    pub fn in_main(&self) -> bool {
        self.toplevel.as_deref() == Some(self.main.as_path())
    }

    /// Refuse an uninitialized repo (`arboria is not initialized for …; run `arb init` there first`).
    pub fn require_initialized(self) -> Result<Self> {
        if self.initialized() {
            Ok(self)
        } else {
            Err(crate::Error::NotInitialized(format!(
                "arboria is not initialized for {}; run `arb init` there first",
                self.main.display()
            )))
        }
    }
}

/// Resolve the repository context for `cwd` (default: the process cwd) with
/// `git rev-parse`. `Error::NotARepo` outside any repo.
pub fn locate(cwd: Option<&Path>) -> Result<RepoContext> {
    todo!("port of arboria.paths.locate")
}

/// Add `pattern` to `<common dir>/info/exclude` once; returns whether it was added.
pub fn ensure_excluded(common_dir: &Path, pattern: &str, comment: &str) -> Result<bool> {
    todo!("port of arboria.paths.ensure_excluded")
}

/// The seed home: `~/arboria` or `$ARBORIA_HOME`. Not a git repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// The home root.
    pub root: PathBuf,
}

impl Home {
    /// `<root>/.arboria`.
    pub fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR_NAME)
    }

    /// `<root>/.arboria/ledger.db`.
    pub fn db_path(&self) -> PathBuf {
        self.state_dir().join(crate::db::DB_NAME)
    }

    /// `<root>/seeds`.
    pub fn seeds_dir(&self) -> PathBuf {
        self.root.join(SEEDS_DIR)
    }

    /// `<root>/.arboria/seeds`: the backup copies; also the home marker.
    pub fn mirror_dir(&self) -> PathBuf {
        self.state_dir().join(SEEDS_DIR)
    }

    /// Ledger and mirror dir exist.
    pub fn initialized(&self) -> bool {
        self.db_path().is_file() && self.mirror_dir().is_dir()
    }

    /// The seed notebook `<root>/seeds/<name>.py` (`.md` once notebooks move to the
    /// markdown format of ADR 0001).
    pub fn notebook(&self, name: &str) -> PathBuf {
        self.seeds_dir().join(format!("{name}.py"))
    }

    /// Whether `path` is inside the home.
    pub fn contains(&self, path: &Path) -> bool {
        todo!("port of arboria.paths.Home.contains")
    }
}

/// `$ARBORIA_HOME` (expanded, resolved) or `~/arboria`.
pub fn home_root() -> PathBuf {
    todo!("port of arboria.paths.home_root")
}

/// The home at [`home_root`].
pub fn home() -> Home {
    Home { root: home_root() }
}

/// `$ARBORIA_CACHE_DIR`, else `$XDG_CACHE_HOME/arboria`, else `~/.cache/arboria`.
pub fn cache_dir() -> PathBuf {
    todo!("port of arboria.paths.cache_dir")
}

/// `(home, seed name)` if `path` is `<home>/seeds/<name>.py` of an initialized home.
pub fn seed_of_notebook(path: &Path) -> Option<(Home, String)> {
    todo!("port of arboria.paths.seed_of_notebook")
}
