//! The background helper (ADR 0004): `arb helper run`, started at login by a LaunchAgent
//! (macOS) or a systemd user service (Linux). New in the Rust core.
//!
//! Each pass, for every listed project ([`crate::registry`]) and the seed home:
//! 1. poll and observe the active runs, as `arb watch` does (`monitor::poll_run` +
//!    `monitor::observe`); a machine that needs a login is skipped for the rest of the
//!    pass and reported once until it works again;
//! 2. run the probes that are due (enabled, the bytes on disk hash to the enabled hash,
//!    the last result older than `every`), and the built-in `balance` every
//!    [`BALANCE_EVERY_S`] for machines with a `balance_cmd` (stored in the seed home);
//! 3. turn what happened into system notifications: new lines of `notifications.log`
//!    (poll and watch write them: a run ended, a failure pattern, a machine needs a login)
//!    and new runs waiting for a signature (`run_submit` events).
//!
//! Invariants:
//! - It writes only what poll, observe and probes write already (through their own
//!   functions). It never approves: no `Confirmed`, no decisions, no `approve` (a source
//!   scan in `tests/guards.rs`).
//! - A project seen for the first time is not replayed: the notification cursor starts
//!   at its current end.
//! - One helper at a time (`<cache>/helper.lock`); its state is `<cache>/helper.json`.
//! - It never stops on a project's error: the error is kept in the state (the app shows
//!   it) and the next project goes on.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::backend::{Runner, SshSession, SystemRunner};
use crate::db::{self, Ledger};
use crate::model::{Run, RunStatus};
use crate::monitor::{self, BackendFactory};
use crate::paths::{self, RepoContext};
use crate::probe::{self, ProbeKey, ProbeScope, ProbeState};
use crate::{clusters, config, Error, Result};

/// Seconds between passes (`arb helper run --every`).
pub const DEFAULT_EVERY_S: u64 = 120;
/// How often the built-in balance probe runs per machine.
pub const BALANCE_EVERY_S: i64 = 30 * 60;
/// The LaunchAgent label / plist name.
pub const LABEL: &str = "world.cwx.arboria.helper";
/// The systemd user unit.
pub const UNIT: &str = "arboria-helper.service";
/// Environment the installed helper keeps from the shell that installed it.
pub const KEPT_ENV: &[&str] = &[
    "ARBORIA_HOME",
    "ARBORIA_CONFIG_DIR",
    "ARBORIA_CACHE_DIR",
    "ARBORIA_PYTHON",
    "SSH_AUTH_SOCK",
];
/// The notification title.
pub const TITLE: &str = "Arboria";

/// Where one project stands for the helper.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectState {
    /// The last `events.id` turned into notifications.
    pub last_event: i64,
    /// Bytes of `notifications.log` already turned into notifications.
    pub log_offset: u64,
    /// Why the last pass could not read it.
    pub error: Option<String>,
}

/// `<cache>/helper.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HelperState {
    pub last_tick: Option<String>,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectState>,
    /// Machines that needed a login (what the ssh gate said), reported once.
    #[serde(default)]
    pub needs_login: BTreeMap<String, String>,
    /// Last built-in balance run per machine.
    #[serde(default)]
    pub balance_at: BTreeMap<String, String>,
}

/// `<cache>/helper.json`.
pub fn state_path() -> PathBuf {
    paths::cache_dir().join("helper.json")
}

/// The saved state (empty when missing or unreadable).
pub fn load_state() -> HelperState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Save the state atomically.
pub fn save_state(state: &HelperState) -> Result<()> {
    let path = state_path();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".helper.json.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(state)? + "\n")?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// What one pass did (`arb helper run --once` prints it).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Pass {
    /// What poll / observe / probes said.
    pub messages: Vec<String>,
    /// The notifications sent (title, body).
    pub notified: Vec<(String, String)>,
    /// Per-project problems (also kept in the state).
    pub errors: Vec<String>,
}

/// The side effects of a pass, injectable for tests.
pub struct Deps<'a> {
    /// Backend for a run (default: its profile's).
    pub make_backend: &'a mut BackendFactory<'a>,
    /// Starts ssh and the probe host.
    pub runner: Arc<dyn Runner>,
    /// Shows a notification.
    pub notify: &'a mut dyn FnMut(&str, &str),
    /// Unix seconds now.
    pub now: i64,
    /// Run probes and the built-in balance.
    pub probes: bool,
}

fn fail(pass: &mut Pass, st: &mut ProjectState, root: &Path, e: &Error) {
    let msg = format!("{}: {e}", root.display());
    if st.error.as_deref() != Some(msg.as_str()) {
        pass.errors.push(msg.clone());
    }
    st.error = Some(msg);
}

/// One pass over every listed project and the seed home.
pub fn tick(state: &mut HelperState, deps: &mut Deps<'_>) -> Pass {
    let mut errors = Vec::new();
    let roots: Vec<PathBuf> = match crate::registry::list() {
        Ok(l) => l.into_iter().map(|e| e.path).collect(),
        Err(e) => {
            errors.push(e.to_string());
            Vec::new()
        }
    };
    let home = paths::home();
    let mut pass = tick_at(state, &roots, home.initialized().then_some(&home), deps);
    errors.append(&mut pass.errors);
    pass.errors = errors;
    pass
}

/// [`tick`] over the given projects and seed home.
pub fn tick_at(
    state: &mut HelperState,
    projects: &[PathBuf],
    home: Option<&paths::Home>,
    deps: &mut Deps<'_>,
) -> Pass {
    let mut pass = Pass::default();
    let mut roots: Vec<(PathBuf, bool)> = projects.iter().map(|p| (p.clone(), false)).collect();
    if let Some(h) = home {
        roots.push((h.root.clone(), true));
    }
    let mut paused: BTreeMap<String, String> = BTreeMap::new();
    let mut worked: Vec<String> = Vec::new();
    for (root, is_home) in &roots {
        let key = root.display().to_string();
        let mut st = state.projects.remove(&key).unwrap_or_else(|| ProjectState {
            last_event: -1,
            ..Default::default()
        });
        match project_pass(
            root,
            *is_home,
            &mut st,
            deps,
            &mut paused,
            &mut worked,
            &mut pass,
        ) {
            Ok(()) => st.error = None,
            Err(e) => fail(&mut pass, &mut st, root, &e),
        }
        state.projects.insert(key, st);
    }
    if let (true, Some(home)) = (deps.probes, home) {
        if let Err(e) = balance_pass(state, deps, home, &mut paused, &mut worked, &mut pass) {
            pass.errors.push(format!("balance: {e}"));
        }
    }
    for (machine, msg) in &paused {
        if state.needs_login.get(machine) != Some(msg) {
            let body = format!("{machine} 需要登录：{msg}");
            (deps.notify)(TITLE, &body);
            pass.notified.push((TITLE.into(), body));
        }
    }
    state.needs_login.retain(|m, _| !worked.contains(m));
    state.needs_login.extend(paused);
    state.last_tick = Some(db::format_utc(deps.now));
    pass
}

fn project_pass(
    root: &Path,
    is_home: bool,
    st: &mut ProjectState,
    deps: &mut Deps<'_>,
    paused: &mut BTreeMap<String, String>,
    worked: &mut Vec<String>,
    pass: &mut Pass,
) -> Result<()> {
    let (ctx, mut ledger) = if is_home {
        let ctx = RepoContext {
            cwd: root.to_path_buf(),
            common_dir: root.join(".git"),
            main: root.to_path_buf(),
            toplevel: None,
        };
        let ledger = Ledger::open(&ctx.state_dir())?;
        (ctx, ledger)
    } else {
        let ctx = paths::locate(Some(root))?.require_initialized()?;
        let ledger = Ledger::open(&ctx.state_dir())?;
        (ctx, ledger)
    };
    let first = st.last_event < 0;
    if first {
        st.last_event =
            ledger
                .conn()
                .query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))?;
        st.log_offset = std::fs::metadata(ctx.state_dir().join(monitor::NOTIFICATIONS))
            .map(|m| m.len())
            .unwrap_or(0);
    }
    if !is_home && config::project_config(&ctx)?.project_id.is_some() {
        poll_pass(&ctx, &mut ledger, deps, paused, worked, pass)?;
    }
    if deps.probes {
        probe_pass(&ctx, is_home, &mut ledger, deps, paused, worked, pass)?;
    }
    let name = if is_home {
        "种子园".to_string()
    } else {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    for line in new_log_lines(
        &ctx.state_dir().join(monitor::NOTIFICATIONS),
        &mut st.log_offset,
    ) {
        let body = format!("{name} · {line}");
        (deps.notify)(TITLE, &body);
        pass.notified.push((TITLE.into(), body));
    }
    let mut stmt = ledger.conn().prepare(
        "SELECT e.id, e.subject FROM events e JOIN runs r ON r.id = e.subject \
         WHERE e.id > ?1 AND e.kind = 'run_submit' AND r.status = 'awaiting_approval' ORDER BY e.id",
    )?;
    let fresh: Vec<(i64, String)> = stmt
        .query_map([st.last_event], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    for (_, run) in &fresh {
        let body = format!("{name} · {run} 等你签字");
        (deps.notify)(TITLE, &body);
        pass.notified.push((TITLE.into(), body));
    }
    st.last_event =
        ledger
            .conn()
            .query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))?;
    Ok(())
}

/// `arb watch --once` over this project's active runs, sharing `paused` across projects.
fn poll_pass(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    deps: &mut Deps<'_>,
    paused: &mut BTreeMap<String, String>,
    worked: &mut Vec<String>,
    pass: &mut Pass,
) -> Result<()> {
    let window = config::project_config(ctx)?.watch.observation_minutes;
    let runs: Vec<Run> = monitor::active_runs(ledger)?;
    for run in runs {
        let machine = run.machine.clone().unwrap_or_default();
        if paused.contains_key(&machine) {
            continue;
        }
        let one = (|| -> Result<Vec<String>> {
            let backend = (deps.make_backend)(ctx, &run)?;
            let outcome = monitor::poll_run(ctx, ledger, &run.id, Some(backend.as_ref()))?;
            let mut msgs = outcome.messages.clone();
            if !RunStatus::FINAL.contains(&outcome.status) {
                msgs.extend(monitor::observe(
                    ctx,
                    ledger,
                    &run.id,
                    backend.as_ref(),
                    window,
                )?);
            }
            Ok(msgs)
        })();
        match one {
            Ok(msgs) => {
                worked.push(machine);
                pass.messages.extend(msgs);
            }
            Err(Error::NeedLogin(msg)) => {
                paused.insert(machine.clone(), msg.clone());
                if !msg.is_empty() {
                    monitor::notify(
                        ctx,
                        &format!(
                            "{machine}: {msg}；小助手暂停对 {machine} 的远程调用，登录后自动继续"
                        ),
                    )?;
                }
            }
            Err(e @ (Error::Sqlite(_) | Error::Io(_) | Error::Json(_))) => return Err(e),
            Err(e) => pass.messages.push(format!("warning: {}: {e}", run.id)),
        }
    }
    Ok(())
}

fn latest_at(ledger: &Ledger, machine: &str, key: &ProbeKey) -> Result<Option<i64>> {
    let at: Option<String> = ledger.conn().query_row(
        "SELECT MAX(at) FROM probe_results WHERE machine = ?1 AND probe = ?2",
        [machine, &key.to_string()],
        |r| r.get(0),
    )?;
    Ok(at.as_deref().and_then(crate::view::parse_utc))
}

fn probe_pass(
    ctx: &RepoContext,
    is_home: bool,
    ledger: &mut Ledger,
    deps: &mut Deps<'_>,
    paused: &mut BTreeMap<String, String>,
    worked: &mut Vec<String>,
    pass: &mut Pass,
) -> Result<()> {
    let state_dir = ctx.state_dir();
    let files = probe::discover((!is_home).then_some(state_dir.as_path()))?;
    for file in files {
        if probe::state(ledger.conn(), &file)? != ProbeState::Enabled {
            continue;
        }
        let Ok(header) = &file.header else { continue };
        if paused.contains_key(&header.machine) {
            continue;
        }
        let last = latest_at(ledger, &header.machine, &file.key)?;
        if last.is_some_and(|t| t + header.every_s as i64 > deps.now) {
            continue;
        }
        match probe::run_once_with(ledger, &file, deps.runner.clone()) {
            Ok(r) => {
                worked.push(header.machine.clone());
                pass.messages
                    .push(format!("probe {}: {}", file.key, probe::result_text(&r)));
            }
            Err(Error::NeedLogin(msg)) => {
                paused.insert(header.machine.clone(), msg);
            }
            Err(e @ (Error::Sqlite(_) | Error::Io(_))) => return Err(e),
            Err(e) => pass
                .messages
                .push(format!("warning: probe {}: {e}", file.key)),
        }
    }
    Ok(())
}

fn balance_pass(
    state: &mut HelperState,
    deps: &mut Deps<'_>,
    home: &paths::Home,
    paused: &mut BTreeMap<String, String>,
    worked: &mut Vec<String>,
    pass: &mut Pass,
) -> Result<()> {
    let raw = match clusters::load_all_raw(None) {
        Ok(r) => r,
        Err(_) => return Ok(()), // no clusters.toml: nothing to ask
    };
    let mut ledger: Option<Ledger> = None;
    for (name, table) in &raw {
        if !table.is_table() || paused.contains_key(name) {
            continue;
        }
        let Ok(profile) = clusters::parse_profile(name, table, "clusters.toml") else {
            continue;
        };
        if profile.balance_cmd.trim().is_empty() || profile.host.is_empty() {
            continue;
        }
        let due = state
            .balance_at
            .get(name)
            .and_then(|t| crate::view::parse_utc(t))
            .is_none_or(|t| t + BALANCE_EVERY_S <= deps.now);
        if !due {
            continue;
        }
        let session = SshSession::new(&profile.host, deps.runner.clone());
        match session.remote(&profile.balance_cmd, None, &[0]) {
            Ok(out) => {
                worked.push(name.clone());
                let input = out.stdout.as_bytes();
                let input = &input[..input.len().min(probe::MAX_INPUT_BYTES)];
                let result = probe::builtin_balance(&profile, &out.stdout);
                if ledger.is_none() {
                    ledger = Some(Ledger::open(&home.state_dir())?);
                }
                let l = ledger.as_mut().expect("opened above");
                probe::set_probe_result(
                    l,
                    name,
                    &ProbeKey::new(ProbeScope::Builtin, "balance")?,
                    &crate::cards::sha256_hex(input),
                    None,
                    &result,
                )?;
                state
                    .balance_at
                    .insert(name.clone(), db::format_utc(deps.now));
                pass.messages
                    .push(format!("balance {name}: {}", probe::result_text(&result)));
            }
            Err(Error::NeedLogin(msg)) => {
                paused.insert(name.clone(), msg);
            }
            Err(e) => pass.messages.push(format!("warning: balance {name}: {e}")),
        }
    }
    Ok(())
}

/// New complete lines of `path` after `offset` (which moves past them); a file that
/// shrank (rotated) starts over. Lines are `<stamp> <text>`: the text is returned.
fn new_log_lines(path: &Path, offset: &mut u64) -> Vec<String> {
    let Ok(mut f) = File::open(path) else {
        return Vec::new();
    };
    let size = f.seek(SeekFrom::End(0)).unwrap_or(0);
    if size < *offset {
        *offset = 0;
    }
    if f.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if f.take(crate::view::NOTIFICATIONS_TAIL_BYTES)
        .read_to_end(&mut buf)
        .is_err()
    {
        return Vec::new();
    }
    let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
        return Vec::new(); // no complete line yet
    };
    *offset += end as u64 + 1;
    String::from_utf8_lossy(&buf[..end])
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match l.split_once(' ') {
            Some((stamp, rest)) if crate::view::parse_utc(stamp).is_some() => rest.to_string(),
            _ => l.to_string(),
        })
        .collect()
}

/// `<cache>/helper.lock`, held while a helper runs. None: another one holds it.
pub fn try_lock() -> Result<Option<File>> {
    let dir = paths::cache_dir();
    std::fs::create_dir_all(&dir)?;
    let f = File::create(dir.join("helper.lock"))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// `arb helper run`: a pass every `every_s` seconds (or one with `once`), saving the
/// state after each. `echo` gets what each pass did.
pub fn run(every_s: u64, once: bool, echo: &mut dyn FnMut(&Pass)) -> Result<()> {
    let Some(_lock) = try_lock()? else {
        crate::bail!(
            "another arb helper is already running (it holds {})",
            paths::cache_dir().join("helper.lock").display()
        );
    };
    loop {
        let mut state = load_state();
        let mut make = |ctx: &RepoContext, run: &Run| monitor::backend_for(ctx, run);
        let mut notify = |t: &str, b: &str| crate::notifier::send(t, b);
        let mut deps = Deps {
            make_backend: &mut make,
            runner: Arc::new(SystemRunner),
            notify: &mut notify,
            now: now_s(),
            probes: true,
        };
        let pass = tick(&mut state, &mut deps);
        save_state(&state)?;
        echo(&pass);
        if once {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(every_s.max(10)));
    }
}

/// Unix seconds now.
pub fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

// ---------------------------------------------------------------- installing

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The LaunchAgent that keeps `exe helper run` going (macOS).
pub fn launch_agent_plist(exe: &Path, env: &[(String, String)], log_dir: &Path) -> String {
    let mut vars = String::new();
    for (k, v) in env {
        vars.push_str(&format!(
            "      <key>{}</key>\n      <string>{}</string>\n",
            xml(k),
            xml(v)
        ));
    }
    let log = |n: &str| xml(&log_dir.join(n).display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
      <string>{}</string>
      <string>helper</string>
      <string>run</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
{vars}    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
  </dict>
</plist>
"#,
        xml(&exe.display().to_string()),
        log("helper.log"),
        log("helper.err.log"),
    )
}

fn systemd_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

/// The systemd user unit that keeps `exe helper run` going (Linux).
pub fn systemd_unit(exe: &Path, env: &[(String, String)]) -> String {
    let mut lines = vec![
        "[Unit]".to_string(),
        "Description=Arboria helper: polls runs, runs probes, sends notifications".into(),
        String::new(),
        "[Service]".into(),
        format!(
            "ExecStart={} helper run",
            systemd_quote(&exe.display().to_string())
        ),
        "Restart=always".into(),
        "RestartSec=30".into(),
    ];
    for (k, v) in env {
        lines.push(format!(
            "Environment={}",
            systemd_quote(&format!("{k}={v}"))
        ));
    }
    lines.extend([
        String::new(),
        "[Install]".into(),
        "WantedBy=default.target".into(),
        String::new(),
    ]);
    lines.join("\n")
}

/// The environment the installed helper gets: `PATH` (ssh, git, python) and
/// [`KEPT_ENV`], as they are now.
pub fn install_env() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for k in std::iter::once("PATH").chain(KEPT_ENV.iter().copied()) {
        if let Some(v) = std::env::var_os(k).filter(|v| !v.is_empty()) {
            out.push((k.to_string(), v.to_string_lossy().into_owned()));
        }
    }
    out
}

/// Where the service file goes under the user's home `home`.
pub fn service_path(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    } else {
        home.join(".config/systemd/user").join(UNIT)
    }
}

/// The service manager commands that (un)load the service.
pub fn load_commands(path: &Path, load: bool) -> Vec<Vec<String>> {
    let p = path.display().to_string();
    if cfg!(target_os = "macos") {
        let uid = unix_uid();
        let domain = format!("gui/{uid}");
        if load {
            vec![
                vec![
                    "launchctl".into(),
                    "bootout".into(),
                    domain.clone(),
                    p.clone(),
                ],
                vec!["launchctl".into(), "bootstrap".into(), domain, p],
            ]
        } else {
            vec![vec!["launchctl".into(), "bootout".into(), domain, p]]
        }
    } else if load {
        vec![
            vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
            vec![
                "systemctl".into(),
                "--user".into(),
                "enable".into(),
                "--now".into(),
                UNIT.into(),
            ],
        ]
    } else {
        vec![vec![
            "systemctl".into(),
            "--user".into(),
            "disable".into(),
            "--now".into(),
            UNIT.into(),
        ]]
    }
}

fn unix_uid() -> String {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "501".into())
}

/// Write the service file for `exe` under `home`; returns its path and the file text.
pub fn write_service(
    home: &Path,
    exe: &Path,
    env: &[(String, String)],
) -> Result<(PathBuf, String)> {
    let path = service_path(home);
    let text = if cfg!(target_os = "macos") {
        let logs = home.join("Library/Logs/Arboria");
        std::fs::create_dir_all(&logs)?;
        launch_agent_plist(exe, env, &logs)
    } else {
        systemd_unit(exe, env)
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, &text)?;
    Ok((path, text))
}

/// Run the service manager commands; the first failing one is the error (a `bootout`
/// of a service that was not loaded is fine).
pub fn run_commands(cmds: &[Vec<String>]) -> Result<()> {
    for argv in cmds {
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .map_err(|e| Error::Arb(format!("cannot run {}: {e}", argv[0])))?;
        let tolerated = argv.get(1).is_some_and(|a| a == "bootout");
        if !out.status.success() && !tolerated {
            return Err(Error::Arb(format!(
                "{} failed: {}",
                argv.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    Ok(())
}

/// Where the helper stands (`arb helper status`, the app's overview).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub service: PathBuf,
    pub installed: bool,
    /// A helper holds the lock right now.
    pub running: bool,
    pub last_tick: Option<String>,
    pub needs_login: BTreeMap<String, String>,
    pub errors: Vec<String>,
}

/// [`Status`] for the user's home `home`.
pub fn status(home: &Path) -> Result<Status> {
    let service = service_path(home);
    let running = try_lock()?.is_none();
    let state = load_state();
    Ok(Status {
        installed: service.is_file(),
        service,
        running,
        last_tick: state.last_tick,
        needs_login: state.needs_login,
        errors: state
            .projects
            .values()
            .filter_map(|p| p.error.clone())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_files() {
        let env = vec![
            ("PATH".to_string(), "/usr/bin:/opt/homebrew/bin".to_string()),
            ("ARBORIA_HOME".to_string(), "/Users/me/a&b".to_string()),
        ];
        let plist = launch_agent_plist(
            Path::new("/Users/me/.cargo/bin/arb"),
            &env,
            Path::new("/Users/me/Library/Logs/Arboria"),
        );
        assert!(plist.contains("<string>world.cwx.arboria.helper</string>"));
        assert!(plist.contains(
            "<string>/Users/me/.cargo/bin/arb</string>\n      <string>helper</string>\n      <string>run</string>"
        ));
        assert!(plist.contains("<key>ARBORIA_HOME</key>\n      <string>/Users/me/a&amp;b</string>"));
        assert!(plist.contains("<key>KeepAlive</key>\n    <true/>"));
        assert!(plist.contains("/Users/me/Library/Logs/Arboria/helper.err.log"));
        let unit = systemd_unit(Path::new("/home/me/.cargo/bin/arb"), &env);
        assert!(unit.contains("ExecStart=\"/home/me/.cargo/bin/arb\" helper run\n"));
        assert!(unit.contains("Environment=\"ARBORIA_HOME=/Users/me/a&b\"\n"));
        assert!(unit.contains("WantedBy=default.target"));
        let p = service_path(Path::new("/h"));
        let cmds = load_commands(&p, true);
        if cfg!(target_os = "macos") {
            assert_eq!(
                p,
                Path::new("/h/Library/LaunchAgents/world.cwx.arboria.helper.plist")
            );
            assert_eq!(cmds[1][1], "bootstrap");
        } else {
            assert_eq!(
                p,
                Path::new("/h/.config/systemd/user/arboria-helper.service")
            );
            assert_eq!(cmds[1], ["systemctl", "--user", "enable", "--now", UNIT]);
        }
    }

    #[test]
    fn log_lines_are_read_once() {
        let dir = std::env::temp_dir().join(format!("arboria-helper-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("notifications.log");
        std::fs::write(&log, "2026-11-04T08:00:00Z r001 已成功（1.00 SU）\npartial").unwrap();
        let mut off = 0;
        assert_eq!(new_log_lines(&log, &mut off), ["r001 已成功（1.00 SU）"]);
        assert!(
            new_log_lines(&log, &mut off).is_empty(),
            "the partial line waits"
        );
        std::fs::write(&log, "x\n").unwrap();
        assert_eq!(
            new_log_lines(&log, &mut off),
            ["x"],
            "a rotated log starts over"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
