//! Arboria.app's Rust side (ADR 0004): the commands the main window calls, over
//! `arboria-core`. The core does the work; this crate only adapts it to the webview.
//!
//! Invariants:
//! - Only the main window's own pages may call these commands (`capabilities/main.json`).
//!   The embedded marimo notebook is a 127.0.0.1 iframe: another origin with no
//!   capability. `app/src/lib/rules.test.ts` keeps it so.
//! - Reads go through `arboria_core::view` (read-only). The only writes: adding a project,
//!   installing the helper, making a missing notebook, and approving.
//! - Approving: `card_shown` records which card (hash) the window showed and when;
//!   `approve` refuses another hash, then `submission::approve_on_device` checks the 2 s
//!   and asks for Touch ID. Nothing here can approve without the human's finger.

mod clone;
mod notebook;
mod pty;
mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use arboria_core::db::Ledger;
use arboria_core::model::RunId;
use arboria_core::{
    clusters, helper, human, notifier, paths, registry, settings, submission, view,
};
use serde::Serialize;

/// Every command the main window may call (`build.rs` makes an `allow-…` permission for
/// each; `capabilities/main.json` grants them to the main window only).
pub const COMMANDS: &[&str] = &[
    "platform",
    "projects",
    "board",
    "add_project",
    "card_shown",
    "approve",
    "helper_status",
    "helper_install",
    "open_notebook",
    "close_notebook",
    "pty_open",
    "pty_write",
    "pty_resize",
    "pty_close",
    "pick_project",
    "clusters_read",
    "clusters_check",
    "clusters_save",
    "open_in_editor",
    "terminal_launchers",
    "pick_folder",
    "clone_project",
    "clone_dir",
    "settings_machines",
    "settings_save_machine",
    "settings_remove_machine",
    "settings_blank_machine",
    "settings_project",
    "settings_save_project",
    "app_settings",
    "set_notifications",
    "helper_uninstall",
];

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// What the app keeps between calls.
#[derive(Default)]
pub struct AppState {
    /// (root, run) → (card hash, when the window showed it).
    shown: Mutex<HashMap<(String, String), (String, Instant)>>,
    ptys: pty::Ptys,
    notebooks: notebook::Notebooks,
}

#[derive(Debug, Serialize)]
struct Platform {
    os: &'static str,
    /// The approval panel can ask for Touch ID here.
    touch_id: bool,
    /// Why not, when it cannot.
    touch_id_why: Option<String>,
    /// The app was started from Claude Code (approving here is refused).
    in_claude_code: Option<&'static str>,
    min_shown_ms: u64,
}

#[tauri::command]
fn platform() -> Platform {
    let device = human::device_available();
    Platform {
        os: std::env::consts::OS,
        touch_id: device.is_ok(),
        touch_id_why: device.err(),
        in_claude_code: human::in_claude_code(),
        min_shown_ms: human::MIN_SHOWN.as_millis() as u64,
    }
}

#[tauri::command]
async fn projects() -> Res<Vec<view::ProjectSummary>> {
    tauri::async_runtime::spawn_blocking(view::projects)
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
async fn board(root: String) -> Res<view::Board> {
    tauri::async_runtime::spawn_blocking(move || view::board(Path::new(&root)))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
fn add_project(path: String) -> Res<registry::Entry> {
    registry::add(Path::new(&path)).map(|(e, _)| e).map_err(err)
}

/// The card of `run` as the window is about to show it; remembers its hash and the time.
#[tauri::command]
async fn card_shown(
    state: tauri::State<'_, AppState>,
    root: String,
    run: String,
) -> Res<view::CardView> {
    let r = root.clone();
    let b = tauri::async_runtime::spawn_blocking(move || view::board(Path::new(&r)))
        .await
        .map_err(err)?
        .map_err(err)?;
    let card = b
        .cards
        .into_iter()
        .find(|c| c.run == run)
        .ok_or_else(|| format!("{run} 不在等你签字（可能已经批准或撤回了）"))?;
    if let (Some(hash), Some(true)) = (&card.card_hash, card.matches) {
        state
            .shown
            .lock()
            .map_err(err)?
            .insert((root, run), (hash.clone(), Instant::now()));
    }
    Ok(card)
}

#[derive(Debug, Serialize)]
struct Approved {
    run: String,
    status: String,
    decision: Option<String>,
    jobs: Vec<String>,
}

/// Approve `run` after the human read the card with `card_hash` (Touch ID on macOS).
#[tauri::command]
async fn approve(
    state: tauri::State<'_, AppState>,
    root: String,
    run: String,
    card_hash: String,
) -> Res<Approved> {
    let key = (root.clone(), run.clone());
    let (hash, at) = state
        .shown
        .lock()
        .map_err(err)?
        .get(&key)
        .cloned()
        .ok_or("先打开审核单读完再签字")?;
    if hash != card_hash {
        return Err(submission::CARD_CHANGED_SINCE_SHOWN.into());
    }
    let shown_for = at.elapsed();
    let out = tauri::async_runtime::spawn_blocking(move || -> arboria_core::Result<Approved> {
        let ctx = paths::locate(Some(Path::new(&root)))?.require_initialized()?;
        let mut ledger = Ledger::open(&ctx.state_dir())?;
        let run_id = RunId::parse(&run)?;
        let r = submission::approve_on_device(&ctx, &mut ledger, &run_id, &hash, shown_for)?;
        Ok(Approved {
            run: r.run.id.to_string(),
            status: r.run.status.to_string(),
            decision: r.decision_id.map(|d| d.to_string()),
            jobs: r.submitted.iter().map(|s| s.job_id.clone()).collect(),
        })
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    state.shown.lock().map_err(err)?.remove(&key);
    Ok(out)
}

fn user_home() -> Res<PathBuf> {
    std::env::home_dir().ok_or_else(|| "no home directory".to_string())
}

#[tauri::command]
fn helper_status() -> Res<helper::Status> {
    helper::status(&user_home()?).map_err(err)
}

/// The `arb` the helper service runs: `$ARBORIA_ARB`, else `arb` on PATH.
fn arb_exe() -> Res<PathBuf> {
    if let Some(p) = std::env::var_os("ARBORIA_ARB").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join("arb"))
        .find(|p| p.is_file())
        .ok_or_else(|| {
            "找不到 arb：先在 arboria 仓库里运行 cargo install --locked --path crates/arb".into()
        })
}

#[tauri::command]
async fn helper_install() -> Res<String> {
    tauri::async_runtime::spawn_blocking(|| -> Res<String> {
        let exe = arb_exe()?;
        let (path, _) =
            helper::write_service(&user_home()?, &exe, &helper::install_env()).map_err(err)?;
        helper::run_commands(&helper::load_commands(&path, true)).map_err(err)?;
        Ok(path.display().to_string())
    })
    .await
    .map_err(err)?
}

/// The URL of the idea's marimo notebook (started on first use), for the page's iframe.
#[tauri::command]
async fn open_notebook(
    state: tauri::State<'_, AppState>,
    root: String,
    idea: String,
) -> Res<String> {
    notebook::open(&state.notebooks, &root, &idea).await
}

#[tauri::command]
fn close_notebook(state: tauri::State<'_, AppState>, root: String, idea: String) -> Res<()> {
    notebook::close(&state.notebooks, &root, &idea)
}

/// The native “choose a folder” dialog, then `arb init` (when the repo has none) and
/// [`registry::add`]. None: cancelled.
#[tauri::command]
async fn pick_project(app: tauri::AppHandle) -> Res<Option<registry::Entry>> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || -> Res<Option<registry::Entry>> {
        let Some(picked) = app
            .dialog()
            .file()
            .set_title("选择一个 arb init 过的仓库")
            .blocking_pick_folder()
        else {
            return Ok(None);
        };
        let dir = picked.into_path().map_err(err)?;
        // a git repo that was never `arb init`-ed: the human chose it here, so init it
        let ctx = paths::locate(Some(&dir)).map_err(err)?;
        if !ctx.initialized() {
            arboria_core::state::init_state(&ctx, &mut |_| {}).map_err(err)?;
        }
        registry::add_context(&ctx)
            .map(|(e, _)| Some(e))
            .map_err(err)
    })
    .await
    .map_err(err)?
}

/// clusters.toml as it is now (the example when there is none), checked.
#[tauri::command]
fn clusters_read() -> Res<clusters::FileCheck> {
    clusters::read_checked().map_err(err)
}

/// What `arb` would say about this text (nothing is written).
#[tauri::command]
fn clusters_check(text: String) -> clusters::FileCheck {
    clusters::check_text(&text)
}

/// Save the text when it is valid (the old file becomes clusters.toml.bak).
#[tauri::command]
fn clusters_save(text: String) -> Res<clusters::FileCheck> {
    clusters::save_text(&text).map_err(err)
}

/// Open clusters.toml, or a listed project's arboria.toml, in the default text editor.
#[tauri::command]
fn open_in_editor(path: String) -> Res<()> {
    let p = PathBuf::from(&path);
    let allowed = p == clusters::config_path()
        || registry::list()
            .map_err(err)?
            .iter()
            .any(|e| e.path.join(paths::CONFIG_NAME) == p);
    if !allowed {
        return Err(format!(
            "{path} is not clusters.toml or a project's arboria.toml"
        ));
    }
    if !p.exists() {
        if p == clusters::config_path() {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(err)?;
            }
            std::fs::write(&p, clusters::TEMPLATE).map_err(err)?;
        } else {
            return Err(format!("{path} does not exist"));
        }
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-t");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(&p).spawn().map(|_| ()).map_err(err)
}

/// The native “choose a folder” dialog (no registering): the folder a clone goes into.
#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Res<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || -> Res<Option<String>> {
        let picked = app
            .dialog()
            .file()
            .set_title("把仓库放在哪个文件夹里")
            .set_directory(clone::default_parent())
            .blocking_pick_folder();
        match picked {
            None => Ok(None),
            Some(p) => Ok(Some(p.into_path().map_err(err)?.display().to_string())),
        }
    })
    .await
    .map_err(err)?
}

/// Where clones go by default; with `set`, remember that folder from now on.
#[tauri::command]
fn clone_dir(set: Option<String>) -> Res<String> {
    match set {
        Some(dir) => clone::set_default_parent(Path::new(&dir)).map(|p| p.display().to_string()),
        None => Ok(clone::default_parent().display().to_string()),
    }
}

/// Clone a repo link into `parent` (default [`clone::default_parent`]), `arb init` it and
/// list it.
#[tauri::command]
async fn clone_project(url: String, parent: Option<String>) -> Res<clone::Cloned> {
    tauri::async_runtime::spawn_blocking(move || {
        let parent = parent
            .map(PathBuf::from)
            .unwrap_or_else(clone::default_parent);
        clone::clone_and_init(&url, &parent)
    })
    .await
    .map_err(err)?
}

// ---------------------------------------------------------------- settings pages

#[tauri::command]
fn settings_machines() -> Res<settings::Machines> {
    settings::machines().map_err(err)
}

#[tauri::command]
fn settings_blank_machine(name: String) -> settings::MachineForm {
    settings::blank_machine(&name)
}

#[tauri::command]
fn settings_save_machine(
    original: Option<String>,
    form: settings::MachineForm,
) -> Res<settings::Machines> {
    settings::save_machine(original.as_deref(), &form).map_err(err)
}

#[tauri::command]
fn settings_remove_machine(name: String) -> Res<settings::Machines> {
    settings::remove_machine(&name).map_err(err)
}

#[tauri::command]
fn settings_project(root: String) -> Res<settings::ProjectSettings> {
    settings::project(Path::new(&root)).map_err(err)
}

#[tauri::command]
fn settings_save_project(
    root: String,
    form: settings::ProjectForm,
) -> Res<settings::ProjectSettings> {
    settings::save_project(Path::new(&root), &form).map_err(err)
}

#[derive(Debug, Serialize)]
struct AppSettings {
    notifications: bool,
    clone_dir: String,
    helper: helper::Status,
}

#[tauri::command]
fn app_settings() -> Res<AppSettings> {
    Ok(AppSettings {
        notifications: notifier::setting_on(),
        clone_dir: clone::default_parent().display().to_string(),
        helper: helper::status(&user_home()?).map_err(err)?,
    })
}

/// Turn system notifications on or off (`<config>/app.json`; the helper reads it).
#[tauri::command]
fn set_notifications(on: bool) -> Res<bool> {
    clone::set_setting("notifications", serde_json::Value::Bool(on))?;
    Ok(notifier::setting_on())
}

#[tauri::command]
async fn helper_uninstall() -> Res<()> {
    tauri::async_runtime::spawn_blocking(|| -> Res<()> {
        let path = helper::service_path(&user_home()?);
        if path.is_file() {
            helper::run_commands(&helper::load_commands(&path, false)).map_err(err)?;
            std::fs::remove_file(&path).map_err(err)?;
        }
        Ok(())
    })
    .await
    .map_err(err)?
}

/// What the terminal's "+" menu offers.
#[tauri::command]
fn terminal_launchers() -> Vec<pty::Launcher> {
    pty::launchers()
}

#[tauri::command]
fn pty_open(
    state: tauri::State<'_, AppState>,
    cwd: String,
    program: Option<String>,
    cols: u16,
    rows: u16,
    on_data: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
    on_exit: tauri::ipc::Channel<()>,
) -> Res<u32> {
    state.ptys.open(
        Path::new(&cwd),
        program.as_deref(),
        cols,
        rows,
        on_data,
        on_exit,
    )
}

#[tauri::command]
fn pty_write(state: tauri::State<'_, AppState>, id: u32, data: String) -> Res<()> {
    state.ptys.write(id, data.as_bytes())
}

#[tauri::command]
fn pty_resize(state: tauri::State<'_, AppState>, id: u32, cols: u16, rows: u16) -> Res<()> {
    state.ptys.resize(id, cols, rows)
}

#[tauri::command]
fn pty_close(state: tauri::State<'_, AppState>, id: u32) -> Res<()> {
    state.ptys.close(id)
}

/// A Finder-started app gets launchd's short PATH; take the login shell's, so `arb`,
/// `ssh`, `claude` and the Python of `arb-py` are found as in the human's terminal.
fn adopt_login_path() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = std::process::Command::new(shell)
        .args(["-l", "-c", "printf %s \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(o) = out {
        let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if o.status.success() && path.contains('/') {
            std::env::set_var("PATH", path);
        }
    }
}

/// Start the app.
pub fn run() {
    adopt_login_path();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            watch::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            platform,
            projects,
            board,
            add_project,
            card_shown,
            approve,
            helper_status,
            helper_install,
            open_notebook,
            close_notebook,
            pty_open,
            pty_write,
            pty_resize,
            pty_close,
            pick_project,
            clusters_read,
            clusters_check,
            clusters_save,
            open_in_editor,
            terminal_launchers,
            pick_folder,
            clone_project,
            clone_dir,
            settings_machines,
            settings_save_machine,
            settings_remove_machine,
            settings_blank_machine,
            settings_project,
            settings_save_project,
            app_settings,
            set_notifications,
            helper_uninstall,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Arboria");
}
