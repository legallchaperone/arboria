//! The embedded terminal's processes (ADR 0004): one pseudo-terminal per tab, read on a
//! thread, bytes sent to the window over a channel. A tab runs the login shell, or one
//! program through it (an agent CLI, or `ssh` to a machine of clusters.toml), so PATH
//! is the human's. What a tab may run is decided here ([`resolve`]), never by the page.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tauri::ipc::{Channel, InvokeResponseBody};

struct Tab {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// The open terminal tabs.
#[derive(Default)]
pub struct Ptys {
    tabs: Mutex<HashMap<u32, Tab>>,
    next: AtomicU32,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The agent CLIs a tab can run: (id, label, program on PATH).
pub const AGENTS: &[(&str, &str, &str)] = &[
    ("claude", "Claude Code", "claude"),
    ("codex", "Codex", "codex"),
];

/// What a tab can open (the "+" menu).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Launcher {
    /// `shell`, an agent id, or `ssh:<machine>`.
    pub id: String,
    pub label: String,
    /// The agent CLI is on PATH (always true for the shell and ssh).
    pub installed: bool,
    /// `agent` / `ssh`; the shell has none.
    pub group: Option<&'static str>,
}

fn on_path(program: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|d| d.join(program).is_file())
}

/// The "+" menu: the shell, the agent CLIs (installed or not), `ssh` to each machine of
/// clusters.toml that has a host.
pub fn launchers() -> Vec<Launcher> {
    let mut out = vec![Launcher {
        id: "shell".into(),
        label: "新终端".into(),
        installed: true,
        group: None,
    }];
    for (id, label, program) in AGENTS {
        out.push(Launcher {
            id: (*id).into(),
            label: (*label).into(),
            installed: on_path(program),
            group: Some("agent"),
        });
    }
    if let Ok(raw) = arboria_core::clusters::load_all_raw(None) {
        for (name, table) in &raw {
            let host = table
                .get("host")
                .and_then(|h| h.as_str())
                .unwrap_or_default();
            if table.is_table() && !host.is_empty() {
                out.push(Launcher {
                    id: format!("ssh:{name}"),
                    label: format!("登录 {name}"),
                    installed: true,
                    group: Some("ssh"),
                });
            }
        }
    }
    out
}

/// The shell command a launcher id runs (None: the plain login shell). Refuses anything
/// that is not one of [`launchers`]' kinds.
pub fn resolve(id: &str) -> Result<Option<String>, String> {
    if id == "shell" {
        return Ok(None);
    }
    if let Some((_, _, program)) = AGENTS.iter().find(|(a, _, _)| *a == id) {
        return Ok(Some(format!("exec {program}")));
    }
    if let Some(machine) = id.strip_prefix("ssh:") {
        let raw = arboria_core::clusters::load_all_raw(None).map_err(|e| e.to_string())?;
        let host = raw
            .get(machine)
            .and_then(|t| t.get("host"))
            .and_then(|h| h.as_str())
            .filter(|h| !h.is_empty())
            .ok_or_else(|| format!("clusters.toml has no host for {machine}"))?;
        return Ok(Some(format!(
            "exec {}",
            arboria_core::slurm::shlex_join(&["ssh", host])
        )));
    }
    Err(format!("the terminal cannot open {id:?}"))
}

/// The argv of a tab: the login shell, or `command` run by it.
pub fn argv(shell: &str, command: Option<&str>) -> Vec<String> {
    match command {
        None => vec![shell.to_string(), "-l".to_string()],
        Some(c) => vec![
            shell.to_string(),
            "-l".to_string(),
            "-c".to_string(),
            c.to_string(),
        ],
    }
}

impl Ptys {
    pub fn open(
        &self,
        cwd: &Path,
        program: Option<&str>,
        cols: u16,
        rows: u16,
        on_data: Channel<InvokeResponseBody>,
        on_exit: Channel<()>,
    ) -> Result<u32, String> {
        let command = resolve(program.unwrap_or("shell"))?;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.max(2),
                cols: cols.max(10),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(err)?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd = CommandBuilder::from_argv(
            argv(&shell, command.as_deref())
                .into_iter()
                .map(Into::into)
                .collect(),
        );
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd).map_err(err)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(err)?;
        let writer = pair.master.take_writer().map_err(err)?;
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if on_data
                            .send(InvokeResponseBody::Raw(buf[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let _ = on_exit.send(());
        });
        self.tabs.lock().map_err(err)?.insert(
            id,
            Tab {
                master: pair.master,
                writer,
                child,
            },
        );
        Ok(id)
    }

    pub fn write(&self, id: u32, data: &[u8]) -> Result<(), String> {
        let mut tabs = self.tabs.lock().map_err(err)?;
        let tab = tabs.get_mut(&id).ok_or("no such terminal")?;
        tab.writer.write_all(data).map_err(err)?;
        tab.writer.flush().map_err(err)
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let tabs = self.tabs.lock().map_err(err)?;
        let tab = tabs.get(&id).ok_or("no such terminal")?;
        tab.master
            .resize(PtySize {
                rows: rows.max(2),
                cols: cols.max(10),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(err)
    }

    pub fn close(&self, id: u32) -> Result<(), String> {
        if let Some(mut tab) = self.tabs.lock().map_err(err)?.remove(&id) {
            if tab.child.try_wait().ok().flatten().is_none() {
                let _ = tab.child.kill();
            }
        }
        Ok(())
    }
}

impl Drop for Ptys {
    fn drop(&mut self) {
        if let Ok(mut tabs) = self.tabs.lock() {
            for (_, mut t) in tabs.drain() {
                let _ = t.child.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_run_the_login_shell_or_a_known_program() {
        assert_eq!(argv("/bin/zsh", None), ["/bin/zsh", "-l"]);
        assert_eq!(
            argv("/bin/zsh", Some("exec claude")),
            ["/bin/zsh", "-l", "-c", "exec claude"]
        );
        assert_eq!(resolve("shell").unwrap(), None);
        assert_eq!(resolve("claude").unwrap().as_deref(), Some("exec claude"));
        assert_eq!(resolve("codex").unwrap().as_deref(), Some("exec codex"));
        assert!(
            resolve("rm -rf /").is_err(),
            "nothing else is run through a tab"
        );
        assert!(resolve("ssh:nope").is_err());
        let l = launchers();
        assert_eq!(l[0].id, "shell");
        assert!(l
            .iter()
            .any(|x| x.id == "claude" && x.group == Some("agent")));
    }
}
