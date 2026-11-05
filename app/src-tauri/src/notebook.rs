//! The idea's marimo notebook in its own window (ADR 0004; the app's own notebook is
//! phase 3). `marimo edit` runs in the idea's worktree on 127.0.0.1 with a token from a
//! 0600 file; the window loads it and gets no IPC capability (only `main` has one). The
//! process stops when its window closes.

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
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Window label → the marimo process behind it.
#[derive(Default)]
pub struct Notebooks {
    procs: Arc<Mutex<HashMap<String, Child>>>,
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

fn token_file(label: &str, token: &str) -> Result<PathBuf, String> {
    use std::io::Write as _;
    let dir = paths::cache_dir().join("notebook-tokens");
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(label);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    opts.open(&path)
        .and_then(|mut f| f.write_all(token.as_bytes()))
        .map_err(err)?;
    Ok(path)
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

pub async fn open(app: &AppHandle, nbs: &Notebooks, root: &str, idea: &str) -> Result<(), String> {
    let label = label_for(root, idea);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    let (r, i) = (root.to_string(), idea.to_string());
    let (cwd, file) =
        tauri::async_runtime::spawn_blocking(move || -> Result<(PathBuf, PathBuf), String> {
            let ctx = paths::locate(Some(Path::new(&r)))
                .and_then(|c| c.require_initialized())
                .map_err(err)?;
            let mut ledger = Ledger::open(&ctx.state_dir()).map_err(err)?;
            let name = IdeaName::parse(&i).map_err(err)?;
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
    let tok = token()?;
    let tok_path = token_file(&label, &tok)?;
    let mut child = Command::new(commands::python_exe())
        .args(["-m", "marimo", "edit"])
        .arg(&file)
        .args([
            "--headless",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--token",
        ])
        .arg("--token-password-file")
        .arg(&tok_path)
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
    let base = format!("http://127.0.0.1:{port}");
    let url: tauri::Url = format!("{base}/?access_token={tok}").parse().map_err(err)?;
    let allowed = base.clone();
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title(format!("{idea} · notebook"))
        .inner_size(1200.0, 860.0)
        .on_navigation(move |u| u.as_str().starts_with(&allowed))
        .build()
        .map_err(err)?;
    nbs.procs.lock().map_err(err)?.insert(label.clone(), child);
    let procs = nbs.procs.clone();
    window.on_window_event(move |e| {
        if let tauri::WindowEvent::Destroyed = e {
            if let Some(mut c) = procs.lock().ok().and_then(|mut p| p.remove(&label)) {
                let _ = c.kill();
                let _ = c.wait();
            }
            let _ = std::fs::remove_file(&tok_path);
        }
    });
    Ok(())
}

impl Drop for Notebooks {
    fn drop(&mut self) {
        if let Ok(mut p) = self.procs.lock() {
            for (_, mut c) in p.drain() {
                let _ = c.kill();
            }
        }
    }
}
