//! 从仓库链接添加项目: `git clone` a repo into a folder the human picked, then `arb init`
//! (core `state::init_state`) and list it (`registry`). The system git does the network
//! part with the human's own credentials (ssh keys, the Keychain helper); it never prompts
//! (`GIT_TERMINAL_PROMPT=0`, ssh BatchMode): a repo that needs a password fails with
//! git's message instead of hanging.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use arboria_core::{paths, registry, state};
use serde::Serialize;

/// How long a clone may take.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// What [`clone_and_init`] did.
#[derive(Debug, Clone, Serialize)]
pub struct Cloned {
    pub path: PathBuf,
    /// What `arb init` did (its output lines).
    pub actions: Vec<String>,
    pub warnings: Vec<String>,
}

/// The folder name a repo link clones into (`…/lm.git` → `lm`), refusing what is not a
/// git link: https://, ssh://, git://, or scp-like `user@host:path`.
pub fn repo_dir_name(url: &str) -> Result<String, String> {
    let url = url.trim();
    let scp_like = !url.contains("://")
        && url
            .split_once(':')
            .is_some_and(|(h, p)| h.contains('@') && !h.contains('/') && !p.is_empty());
    let schemed = ["https://", "http://", "ssh://", "git://"]
        .iter()
        .any(|s| url.starts_with(s));
    if url.starts_with('-')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
        || !(schemed || scp_like)
    {
        return Err(format!(
            "不是仓库链接：{url}（要 https://…、ssh://… 或 git@host:owner/repo 这样的地址）"
        ));
    }
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default();
    let name = last.strip_suffix(".git").unwrap_or(last);
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !ok {
        return Err(format!("从 {url} 看不出仓库名"));
    }
    Ok(name.to_string())
}

/// Clone `url` into `<parent>/<repo name>`, `arb init` it and list it.
pub fn clone_and_init(url: &str, parent: &Path) -> Result<Cloned, String> {
    let name = repo_dir_name(url)?;
    if !parent.is_dir() {
        return Err(format!("{} 不是一个文件夹", parent.display()));
    }
    let dest = parent.join(&name);
    if dest.exists() {
        return Err(format!(
            "{} 已经存在：换一个文件夹，或者直接用「选择文件夹…」添加它",
            dest.display()
        ));
    }
    clone_to(url.trim(), &dest)
}

/// [`clone_and_init`] once the link and the destination are checked.
fn clone_to(url: &str, dest: &Path) -> Result<Cloned, String> {
    let mut child = Command::new("git")
        .args(["clone", "--"])
        .arg(url)
        .arg(dest)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git ({e})"))?;
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = std::fs::remove_dir_all(dest);
            return Err("git clone 超过 10 分钟没有完成，已停止".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read as _;
            let _ = e.read_to_string(&mut err);
        }
        let last = err.trim().lines().last().unwrap_or("").to_string();
        return Err(format!("git clone 失败：{last}"));
    }
    let ctx = paths::locate(Some(dest)).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    let actions = state::init_state(&ctx, &mut |w| warnings.push(w.to_string()))
        .map_err(|e| e.to_string())?;
    registry::add_context(&ctx).map_err(|e| e.to_string())?;
    Ok(Cloned {
        path: ctx.main,
        actions,
        warnings,
    })
}

/// The app's own settings: `<config dir>/app.json` (`{"clone_dir": "…"}`).
fn settings_path() -> PathBuf {
    arboria_core::clusters::config_dir().join("app.json")
}

/// Where clones go unless the human picks another folder: the folder picked last time
/// (saved by [`set_default_parent`]), else `~/code` (or `~/src`, `~/projects`,
/// `~/Developer`) when it exists, else the home folder.
pub fn default_parent() -> PathBuf {
    let saved = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["clone_dir"].as_str().map(PathBuf::from))
        .filter(|p| p.is_dir());
    if let Some(p) = saved {
        return p;
    }
    let home = std::env::home_dir().unwrap_or_default();
    ["code", "src", "projects", "Developer"]
        .iter()
        .map(|d| home.join(d))
        .find(|p| p.is_dir())
        .unwrap_or(home)
}

/// Remember `dir` as where clones go from now on.
pub fn set_default_parent(dir: &Path) -> Result<PathBuf, String> {
    if !dir.is_dir() {
        return Err(format!("{} 不是一个文件夹", dir.display()));
    }
    let path = settings_path();
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v["clone_dir"] = dir.display().to_string().into();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tests below set the process-wide ARBORIA_CONFIG_DIR.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn repo_links() {
        assert_eq!(repo_dir_name("https://github.com/me/lm.git").unwrap(), "lm");
        assert_eq!(
            repo_dir_name("https://github.com/me/WaveFront/").unwrap(),
            "WaveFront"
        );
        assert_eq!(repo_dir_name("git@github.com:me/lm.git").unwrap(), "lm");
        assert_eq!(repo_dir_name("ssh://git@host:22/x/y-z.git").unwrap(), "y-z");
        for bad in [
            "--upload-pack=x",
            "lm",
            "/Users/me/lm",
            "https://x/a b",
            "https://x/..",
            "file:///etc",
        ] {
            assert!(repo_dir_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn clones_inits_and_lists() {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("arboria-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("ARBORIA_CONFIG_DIR", tmp.join("cfg"));
        let g = |cwd: &Path, args: &[&str]| {
            let o = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(o.status.success(), "{o:?}");
        };
        let origin = tmp.join("origin").join("lm");
        std::fs::create_dir_all(&origin).unwrap();
        g(&origin, &["init", "-q", "-b", "main"]);
        std::fs::write(origin.join("train.py"), "print('hi')\n").unwrap();
        g(&origin, &["add", "-A"]);
        g(&origin, &["commit", "-q", "-m", "init"]);
        let parent = tmp.join("code");
        std::fs::create_dir_all(&parent).unwrap();
        // a local path is not a link
        assert!(clone_and_init(origin.to_str().unwrap(), &parent).is_err());
        // nothing listens there: git's own message, nothing left behind
        let e = clone_and_init("http://127.0.0.1:9/nope.git", &parent).unwrap_err();
        assert!(e.starts_with("git clone 失败："), "{e}");
        assert!(!parent.join("nope").exists());
        // the clone itself (from the local repo), arb init, listed
        let got = clone_to(origin.to_str().unwrap(), &parent.join("lm")).unwrap();
        assert_eq!(got.path, parent.join("lm").canonicalize().unwrap());
        assert!(got.path.join(".arboria").join("ledger.db").is_file());
        assert!(got.path.join("arboria.toml").is_file());
        assert!(registry::list().unwrap().iter().any(|e| e.path == got.path));
        let again = clone_and_init("https://example.com/me/lm.git", &parent).unwrap_err();
        assert!(again.contains("已经存在"), "{again}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_clone_folder_is_remembered() {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("arboria-clonedir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("elsewhere")).unwrap();
        std::env::set_var("ARBORIA_CONFIG_DIR", tmp.join("cfg"));
        assert!(set_default_parent(&tmp.join("missing")).is_err());
        set_default_parent(&tmp.join("elsewhere")).unwrap();
        assert_eq!(default_parent(), tmp.join("elsewhere"));
        std::fs::remove_dir_all(tmp.join("elsewhere")).unwrap();
        assert_ne!(
            default_parent(),
            tmp.join("elsewhere"),
            "a folder that is gone is ignored"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
