//! Arboria.app's Rust side (ADR 0004): the commands the main window calls, over
//! `arboria-core`. The core does the work; this crate only adapts it to the webview.
//!
//! Invariants:
//! - Only the main window's own pages may call these commands (`capabilities/main.json`).
//!   The embedded marimo notebook is a 127.0.0.1 iframe: another origin with no
//!   capability. `app/src/lib/rules.test.ts` keeps it so.
//! - Reads go through `arboria_core::view` (read-only). Writes: adding / removing a
//!   project in the list, installing the helper, making a missing notebook, the settings
//!   pages, approving, and the everyday actions in `actions.rs` (each a fixed `arb`
//!   command line, none of which approves).
//! - Approving: `card_shown` records which card (hash) the window showed and when;
//!   `approve` refuses another hash, then `submission::approve_on_device` checks the 2 s
//!   and asks for Touch ID. Nothing here can approve without the human's finger.

mod actions;
mod agentq;
mod clone;
mod nb;
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
    "notification_status",
    "notification_test",
    "act",
    "remove_project",
    "reveal",
    "nb_load",
    "nb_save",
    "nb_run",
    "nb_commands",
    "nb_interrupt",
    "nb_restart",
    "agent_rows",
    "agent_log",
    "agent_settings",
    "agent_save_settings",
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
    kernels: nb::Kernels,
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
    /// The seed home (种子园), made on the first seed.
    home: PathBuf,
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
        home: paths::home_root(),
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

/// A listed project (or the seed home) at `root`; anything else is refused.
fn listed(root: &str) -> Res<PathBuf> {
    let p = paths::resolve(Path::new(root));
    if p == paths::home_root() || registry::list().map_err(err)?.iter().any(|e| e.path == p) {
        Ok(p)
    } else {
        Err(format!("{root} 不在项目列表里"))
    }
}

/// One of the everyday actions (`actions::Action`) in the project at `root`.
#[tauri::command]
async fn act(root: String, action: actions::Action) -> Res<actions::Done> {
    tauri::async_runtime::spawn_blocking(move || -> Res<actions::Done> {
        let root = listed(&root)?;
        let board = view::board(&root).ok();
        let worktree = |idea: &str| {
            board.as_ref().and_then(|b| {
                b.ideas
                    .iter()
                    .find(|i| i.name == idea && i.stage == "open")
                    .and_then(|i| i.worktree.clone())
                    .map(PathBuf::from)
                    .filter(|p| p.is_dir())
            })
        };
        let home = paths::home_root();
        let (cwd, args) = actions::argv(&root, &action, worktree, &home)?;
        if cwd == home {
            // the first seed: arb makes the seed home in the folder it runs in
            std::fs::create_dir_all(&home).map_err(err)?;
        }
        actions::run(&arb_exe()?, &cwd, &args)
    })
    .await
    .map_err(err)?
}

/// Take a project off the list (`arb projects remove`); its files and ledger stay.
#[tauri::command]
fn remove_project(path: String) -> Res<bool> {
    registry::remove(Path::new(&path)).map_err(err)
}

/// Show a listed project's folder (or an idea's worktree in it) in Finder.
#[tauri::command]
fn reveal(root: String, path: Option<String>) -> Res<()> {
    let root = listed(&root)?;
    let target = match path {
        Some(p) => {
            let board = view::board(&root).map_err(err)?;
            if !board
                .ideas
                .iter()
                .any(|i| i.worktree.as_deref() == Some(p.as_str()))
            {
                return Err(format!("{p} 不是这个项目的 worktree"));
            }
            PathBuf::from(p)
        }
        None => root,
    };
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(&target).spawn().map(|_| ()).map_err(err)
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

/// Inside Arboria.app: notifications (the app's, and the helper's once installed from
/// here) are posted by this executable with `--notify`.
fn adopt_notifier() {
    if let Ok(exe) = std::env::current_exe() {
        if exe.to_string_lossy().contains(".app/Contents/MacOS/") {
            std::env::set_var(notifier::NOTIFIER_ENV, exe);
        }
    }
}

/// Whether this app may show notifications: `authorized`, `denied`, `not_determined`,
/// or why it cannot tell (not macOS, not the bundled app).
#[tauri::command]
async fn notification_status() -> String {
    tauri::async_runtime::spawn_blocking(|| match arboria_macos::notification_status() {
        Ok(arboria_macos::NotifyStatus::Authorized) => "authorized".to_string(),
        Ok(arboria_macos::NotifyStatus::Denied) => "denied".to_string(),
        Ok(arboria_macos::NotifyStatus::NotDetermined) => "not_determined".to_string(),
        Err(e) => e,
    })
    .await
    .unwrap_or_else(|e| e.to_string())
}

/// Send a test notification the way the helper does (`notifier::command`), and say what
/// happened: the program's error, or Ok.
#[tauri::command]
async fn notification_test() -> Res<String> {
    tauri::async_runtime::spawn_blocking(|| -> Res<String> {
        let (program, args) = notifier::command(
            "Arboria",
            "测试通知：通知能弹出来。run 跑完、失败或等你签字时会这样提醒你。",
        )
        .ok_or("这个系统上没有发通知的办法")?;
        let out = std::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("{program}: {e}"))?;
        if out.status.success() {
            Ok(program)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    })
    .await
    .map_err(err)?
}

// ---------------------------------------------------------------- the notebook (ADR 0005)

/// A notebook of a listed project (or the seed home): its text, blocks and last results.
#[tauri::command]
async fn nb_load(state: tauri::State<'_, AppState>, root: String, idea: String) -> Res<nb::NbView> {
    let root = listed(&root)?;
    nb::load(&state.kernels, &root, &idea)
}

#[derive(Debug, Serialize)]
struct Saved {
    text: String,
    doc: arboria_core::nbdoc::Doc,
}

/// Save the notebook's text (canonical form and ids come back).
#[tauri::command]
async fn nb_save(root: String, idea: String, text: String) -> Res<Saved> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = listed(&root)?;
        let (text, doc) = nb::save(&root, &idea, &text)?;
        Ok(Saved { text, doc })
    })
    .await
    .map_err(err)?
}

/// Run one block in the notebook's kernel; streams come as `nb-event`.
#[tauri::command]
async fn nb_run(
    app: tauri::AppHandle,
    root: String,
    idea: String,
    block: String,
    code: String,
) -> Res<arboria_core::nbout::Cached> {
    use tauri::Manager as _;
    tauri::async_runtime::spawn_blocking(move || {
        let root = listed(&root)?;
        let state = app.state::<AppState>();
        nb::run(&app, &state.kernels, &root, &idea, &block, &code)
    })
    .await
    .map_err(err)?
}

/// The commands a block can call, for completion.
#[tauri::command]
async fn nb_commands(root: String) -> Res<Vec<nb::CommandEntry>> {
    tauri::async_runtime::spawn_blocking(move || nb::commands(&listed(&root)?))
        .await
        .map_err(err)?
}

#[tauri::command]
fn nb_interrupt(state: tauri::State<'_, AppState>, root: String, idea: String) -> Res<()> {
    let root = listed(&root)?;
    nb::interrupt(&state.kernels, &root, &idea)
}

#[tauri::command]
fn nb_restart(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    root: String,
    idea: String,
) -> Res<()> {
    let root = listed(&root)?;
    nb::restart(&app, &state.kernels, &root, &idea)
}

// ---------------------------------------------------------------- the agent queue

#[tauri::command]
async fn agent_rows(root: String) -> Res<Vec<agentq::Row>> {
    tauri::async_runtime::spawn_blocking(move || agentq::rows(&listed(&root)?))
        .await
        .map_err(err)?
}

#[tauri::command]
async fn agent_log(root: String, task: String) -> Res<Vec<arboria_core::agent::LogLine>> {
    tauri::async_runtime::spawn_blocking(move || agentq::log(&listed(&root)?, &task))
        .await
        .map_err(err)?
}

#[tauri::command]
fn agent_settings() -> Res<arboria_core::agent::Settings> {
    arboria_core::agent::load_settings().map_err(err)
}

#[tauri::command]
fn agent_save_settings(
    settings: arboria_core::agent::Settings,
) -> Res<arboria_core::agent::Settings> {
    arboria_core::agent::save_settings(&settings).map_err(err)?;
    arboria_core::agent::load_settings().map_err(err)
}

/// Start the app.
pub fn run() {
    adopt_login_path();
    adopt_notifier();
    // ask for notification permission while the human is here (the system prompt, once)
    if cfg!(target_os = "macos") && notifier::enabled() {
        std::thread::spawn(|| {
            // whichever Arboria process the system asks, the answer is "show the banner"
            let _ = arboria_macos::present_while_in_front();
            if arboria_macos::notification_status()
                == Ok(arboria_macos::NotifyStatus::NotDetermined)
            {
                let _ = arboria_macos::request_notifications();
            }
        });
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            watch::start(app.handle().clone());
            agentq::start(app.handle().clone());
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
            notification_status,
            notification_test,
            act,
            remove_project,
            reveal,
            nb_load,
            nb_save,
            nb_run,
            nb_commands,
            nb_interrupt,
            nb_restart,
            agent_rows,
            agent_log,
            agent_settings,
            agent_save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Arboria");
}
