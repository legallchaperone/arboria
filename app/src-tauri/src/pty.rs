//! The embedded terminal's processes (ADR 0004): one pseudo-terminal per tab, read on a
//! thread, bytes sent to the window over a channel. A tab runs the login shell, or one
//! program through it (`claude`), so PATH is the human's.

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

/// The argv of a tab: the login shell, or `program` run by it.
pub fn argv(shell: &str, program: Option<&str>) -> Vec<String> {
    match program {
        None => vec![shell.to_string(), "-l".to_string()],
        Some(p) => vec![
            shell.to_string(),
            "-l".to_string(),
            "-c".to_string(),
            format!("exec {p}"),
        ],
    }
}

/// The programs a tab may run besides the shell.
pub const PROGRAMS: &[&str] = &["claude"];

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
        if let Some(p) = program {
            if !PROGRAMS.contains(&p) {
                return Err(format!(
                    "the terminal runs a shell or one of {PROGRAMS:?}, not {p}"
                ));
            }
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.max(2),
                cols: cols.max(10),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(err)?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut cmd =
            CommandBuilder::from_argv(argv(&shell, program).into_iter().map(Into::into).collect());
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
    fn tabs_run_the_login_shell() {
        assert_eq!(argv("/bin/zsh", None), ["/bin/zsh", "-l"]);
        assert_eq!(
            argv("/bin/zsh", Some("claude")),
            ["/bin/zsh", "-l", "-c", "exec claude"]
        );
        assert_eq!(PROGRAMS, ["claude"], "nothing else is run through a tab");
    }
}
