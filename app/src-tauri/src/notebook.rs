//! The idea's (or a seed's) marimo notebook, embedded in the main window (ADR 0004; the app's own
//! notebook is phase 3). `marimo edit` runs in the idea's worktree on 127.0.0.1 under a
//! secret base path (`--base-url /nb-<48 random hex>`; everything else answers 404), not
//! marimo's token: the token becomes a `SameSite=Lax` cookie, which WKWebView does not
//! send from a cross-site iframe (the page is `tauri://localhost`), so on macOS the
//! notebook stayed blank. The page shows it in a sandboxed iframe (CSP `frame-src` allows
//! only 127.0.0.1). The iframe is another origin: it cannot reach the page, and Tauri gives
//! that origin no capability, so notebook code cannot call `approve`. One server per idea,
//! reused while the app runs, stopped with it.

use std::collections::HashMap;
use std::io::Read as _;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arboria_core::db::Ledger;
use arboria_core::model::IdeaName;
use arboria_core::{commands, ideas, paths};

/// A running notebook server.
struct Server {
    child: Child,
    url: String,
}

/// (root, idea) key → the marimo server behind it.
#[derive(Default)]
pub struct Notebooks {
    procs: Arc<Mutex<HashMap<String, Server>>>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn label_for(root: &str, idea: &str) -> String {
    let h = arboria_core::cards::sha256_hex(root.as_bytes());
    format!("notebook-{}-{idea}", &h[..8])
}

fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind(("127.0.0.1", 0)).map_err(err)?;
    Ok(l.local_addr().map_err(err)?.port())
}

fn token() -> Result<String, String> {
    let mut b = [0u8; 24];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(err)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Where marimo runs and which file: the worktree's `notebook.py` link when there is
/// one (the stylesheet link sits next to it), else the file under `.arboria/notebooks`.
fn target(worktree: Option<&str>, file: &Path) -> (PathBuf, PathBuf) {
    if let Some(wt) = worktree.map(Path::new) {
        if wt.join("notebook.py").exists() {
            return (wt.to_path_buf(), PathBuf::from("notebook.py"));
        }
    }
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = file.file_name().map(PathBuf::from).unwrap_or_default();
    (dir, name)
}

/// The URL of the idea's notebook server, starting it when it is not running.
pub async fn open(nbs: &Notebooks, root: &str, idea: &str) -> Result<String, String> {
    let label = label_for(root, idea);
    {
        let mut procs = nbs.procs.lock().map_err(err)?;
        if let Some(s) = procs.get_mut(&label) {
            if s.child.try_wait().ok().flatten().is_none() {
                return Ok(s.url.clone());
            }
            procs.remove(&label);
        }
    }
    let (r, i) = (root.to_string(), idea.to_string());
    let (cwd, file) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(PathBuf, PathBuf), String> {
            let name = IdeaName::parse(&i).map_err(err)?;
            // a seed: its notebook is a plain file in the seed home (`seeds/<name>.py`)
            let home = paths::home_root();
            if paths::resolve(Path::new(&r)) == home {
                let dir = home.join(paths::SEEDS_DIR);
                let file = format!("{}.py", name.as_str());
                if !dir.join(&file).is_file() {
                    return Err(format!(
                        "种子 {i} 没有 notebook（{}）",
                        dir.join(&file).display()
                    ));
                }
                return Ok((dir, PathBuf::from(file)));
            }
            let ctx = paths::locate(Some(Path::new(&r)))
                .and_then(|c| c.require_initialized())
                .map_err(err)?;
            let mut ledger = Ledger::open(&ctx.state_dir()).map_err(err)?;
            let mut warnings = Vec::new();
            let (row, path, _) = ideas::ensure_notebook(&ctx, &mut ledger, Some(&name), &mut |w| {
                warnings.push(w.to_string())
            })
            .map_err(err)?;
            if !path.is_file() {
                return Err(warnings.join("; "));
            }
            Ok(target(row.worktree_path.as_deref(), &path))
        })
        .await
        .map_err(err)??;
    let port = free_port()?;
    let base = format!("/nb-{}", token()?);
    let mut child = Command::new(commands::python_exe())
        .args(["-m", "marimo", "edit"])
        .arg(&file)
        .args([
            "--headless",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--no-token",
            "--base-url",
            &base,
        ])
        .current_dir(&cwd)
        .env("MARIMO_SKIP_UPDATE_CHECK", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "cannot start marimo with {} ({e}); set ARBORIA_PYTHON",
                commands::python_exe()
            )
        })?;
    let started = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if let Ok(Some(status)) = child.try_wait() {
            let mut msg = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut msg);
            }
            let last = msg.trim().lines().last().unwrap_or("").to_string();
            return Err(format!("marimo exited ({status}): {last}"));
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            return Err("marimo did not start within 30 s".into());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    let url = format!("http://127.0.0.1:{port}{base}/");
    nbs.procs.lock().map_err(err)?.insert(
        label,
        Server {
            child,
            url: url.clone(),
        },
    );
    Ok(url)
}

/// Stop the idea's notebook server (if it runs).
pub fn close(nbs: &Notebooks, root: &str, idea: &str) -> Result<(), String> {
    if let Some(mut s) = nbs
        .procs
        .lock()
        .map_err(err)?
        .remove(&label_for(root, idea))
    {
        let _ = s.child.kill();
        let _ = s.child.wait();
    }
    Ok(())
}

impl Drop for Notebooks {
    fn drop(&mut self) {
        if let Ok(mut p) = self.procs.lock() {
            for (_, mut s) in p.drain() {
                let _ = s.child.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str) -> String {
        use std::io::Write as _;
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out.lines().next().unwrap_or("").to_string()
    }

    /// A seed's notebook through the real marimo (`ARBORIA_TEST_MARIMO=1`, with
    /// `ARBORIA_PYTHON` pointing at a Python that has marimo): served under the secret
    /// path with no cookie needed (what a cross-site iframe on macOS gets), 404 elsewhere.
    #[test]
    fn served_under_a_secret_path_without_cookies() {
        if std::env::var("ARBORIA_TEST_MARIMO").as_deref() != Ok("1") {
            eprintln!("skipped: set ARBORIA_TEST_MARIMO=1");
            return;
        }
        let _env = crate::clone::tests::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!("arboria-nb-{}", std::process::id()));
        std::fs::create_dir_all(home.join("seeds")).unwrap();
        std::fs::write(
            home.join("seeds/idea-a.py"),
            "import marimo\napp = marimo.App()\n\n@app.cell\ndef _():\n    return\n",
        )
        .unwrap();
        std::env::set_var("ARBORIA_HOME", &home);
        let nbs = Notebooks::default();
        let root = paths::home_root().display().to_string();
        let url = tauri::async_runtime::block_on(open(&nbs, &root, "idea-a")).unwrap();
        let rest = url.strip_prefix("http://127.0.0.1:").unwrap();
        let (port, path) = rest.split_at(rest.find('/').unwrap());
        let port: u16 = port.parse().unwrap();
        assert!(path.starts_with("/nb-") && path.len() > 40, "{url}");
        assert!(get(port, path).contains(" 200"), "{}", get(port, path));
        for other in ["/", "/api/status", "/nb-wrong/"] {
            assert!(
                get(port, other).contains(" 404"),
                "{other}: {}",
                get(port, other)
            );
        }
        // the same server is reused
        let again = tauri::async_runtime::block_on(open(&nbs, &root, "idea-a")).unwrap();
        assert_eq!(again, url);
        close(&nbs, &root, "idea-a").unwrap();
        std::env::remove_var("ARBORIA_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }
}
