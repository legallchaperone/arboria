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

use std::path::{Component, Path, PathBuf};

use crate::gitutil;
use crate::records::py_strip;
use crate::{Error, Result};

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
    let cwd = match cwd {
        Some(c) => c.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let cwd = resolve(&cwd);
    let common = gitutil::run(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        &cwd,
        &[],
        false,
    )?;
    if common.status != 0 {
        return Err(Error::NotARepo(format!(
            "{} is not inside a git repository",
            cwd.display()
        )));
    }
    let common_dir = resolve(Path::new(&common.text()));
    let top = gitutil::run(&["rev-parse", "--show-toplevel"], &cwd, &[], false)?;
    let toplevel =
        (top.status == 0 && !top.text().is_empty()).then(|| resolve(Path::new(&top.text())));
    let main = common_dir
        .parent()
        .map_or_else(|| common_dir.clone(), Path::to_path_buf);
    Ok(RepoContext {
        cwd,
        common_dir,
        main,
        toplevel,
    })
}

/// Python's `Path.resolve()` (non-strict): symlinks resolved as far as the path exists,
/// the rest appended as is.
pub fn resolve(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    if let Ok(p) = abs.canonicalize() {
        return p;
    }
    // the longest existing prefix, canonicalized, plus the missing tail (normalized)
    let mut tail = Vec::new();
    let mut head = abs.as_path();
    loop {
        match head.canonicalize() {
            Ok(base) => {
                let mut out = base;
                for part in tail.iter().rev() {
                    match part {
                        Component::ParentDir => {
                            out.pop();
                        }
                        Component::CurDir => {}
                        c => out.push(c),
                    }
                }
                return out;
            }
            Err(_) => match (head.parent(), head.components().next_back()) {
                (Some(parent), Some(last)) => {
                    tail.push(last);
                    head = parent;
                }
                _ => return abs,
            },
        }
    }
}

/// Add `pattern` to `<common dir>/info/exclude` once; returns whether it was added.
pub fn ensure_excluded(common_dir: &Path, pattern: &str, comment: &str) -> Result<bool> {
    let exclude = common_dir.join("info").join("exclude");
    std::fs::create_dir_all(exclude.parent().expect("has a parent"))?;
    let mut text = match std::fs::read_to_string(&exclude) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    // Python str.splitlines() / strip(); the file is plain ASCII in practice
    if text.lines().any(|l| py_strip(l) == pattern) {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("# {comment}\n{pattern}\n"));
    std::fs::write(&exclude, text)?;
    Ok(true)
}

/// [`ensure_excluded`] with arb's own pattern: `/.arboria/`.
pub fn ensure_state_excluded(common_dir: &Path) -> Result<bool> {
    ensure_excluded(
        common_dir,
        &format!("/{STATE_DIR_NAME}/"),
        "arboria state (local only)",
    )
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
        let p = resolve(path);
        p.starts_with(&self.root)
    }
}

/// `$ARBORIA_HOME` (expanded, resolved) or `~/arboria`.
pub fn home_root() -> PathBuf {
    let root = match std::env::var_os(HOME_ENV) {
        Some(env) if !env.is_empty() => expand_user(Path::new(&env)),
        _ => user_home().join("arboria"),
    };
    resolve(&root)
}

fn user_home() -> PathBuf {
    std::env::home_dir().unwrap_or_default()
}

/// Python `Path.expanduser()` for `~` and `~/…` (not `~user`).
fn expand_user(p: &Path) -> PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) => user_home().join(rest),
        Err(_) => p.to_path_buf(),
    }
}

/// The home at [`home_root`].
pub fn home() -> Home {
    Home { root: home_root() }
}

/// `$ARBORIA_CACHE_DIR`, else `$XDG_CACHE_HOME/arboria`, else `~/.cache/arboria`.
pub fn cache_dir() -> PathBuf {
    if let Some(env) = std::env::var_os(CACHE_ENV).filter(|e| !e.is_empty()) {
        return expand_user(Path::new(&env));
    }
    let base = match std::env::var_os("XDG_CACHE_HOME").filter(|e| !e.is_empty()) {
        Some(xdg) => expand_user(Path::new(&xdg)),
        None => user_home().join(".cache"),
    };
    base.join("arboria")
}

/// `(home, seed name)` if `path` is `<home>/seeds/<name>.py` of an initialized home.
pub fn seed_of_notebook(path: &Path) -> Option<(Home, String)> {
    let p = std::path::absolute(path).ok()?;
    if !p.exists() {
        return None;
    }
    // the notebook may be a link: resolve its directory, keep its own name
    let p = p.parent()?.canonicalize().ok()?.join(p.file_name()?);
    if p.extension()? != "py" || p.parent()?.file_name()? != SEEDS_DIR {
        return None;
    }
    let home = Home {
        root: p.parent()?.parent()?.to_path_buf(),
    };
    let name = p.file_stem()?.to_str()?.to_string();
    home.initialized().then_some((home, name))
}
