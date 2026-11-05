//! Checks that a human, in their own terminal, is the one approving. Ports
//! `arboria.human`.
//!
//! Invariants:
//! - `arb approve` and `arb lesson accept` call [`require_terminal`] before anything is
//!   recorded: refused when `CLAUDECODE` or `CLAUDE_CODE_ENTRYPOINT` is present in the
//!   environment (any value, even empty), or when stdin is not a TTY. Then the human
//!   types the id ([`confirm_typed`]). The provenance goes into the Decision payload.
//! - There is no approval function anywhere else, and no code path that approves
//!   automatically. The app's approval panel is the second entry point (ADR 0004):
//!   [`confirm_on_device`] refuses inside Claude Code, refuses a card shown for less than
//!   [`MIN_SHOWN`], then asks for Touch ID (`arboria_macos::authenticate`; refused on
//!   other systems). Only a passed check becomes a [`Confirmed`].
//! - A guard rail, not a sandbox: the hard guarantee is
//!   [`crate::submission`]'s guarded submit, which needs a human Decision in the ledger.
//!
//! Python also refuses while a notebook command (`lab.<name>`) runs in the same process.
//! That registry lives in the Python kernel, which is a separate process from the Rust
//! core, so it stays there; the Rust CLI never runs command code.
//!
//! All of it is real. Notebook commands (Python `running_command`) do not exist in the
//! Rust core, so only the environment and TTY checks apply.

use std::io::IsTerminal;

use serde::Serialize;

use crate::{Error, Result};

/// The variables Claude Code sets for its tools.
pub const CLAUDE_ENV_VARS: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];
/// `{command}` is the command to run in the human's own terminal.
pub const REFUSAL: &str = "批准只能在你自己的终端里做：请在终端运行 {command}";
/// A mistyped confirmation.
pub const MISMATCH: &str = "输入的 {got} 与 {expected} 不符，已取消（没有做任何改动）";
/// The app shows the whole card at least this long before the human may sign.
pub const MIN_SHOWN: std::time::Duration = std::time::Duration::from_secs(2);
/// `{command}` is `arb approve rNNN`.
pub const DEVICE_UNAVAILABLE: &str =
    "这里不能用触控 ID 批准（{why}）：请在你自己的终端运行 {command}";
/// The app was started from Claude Code.
pub const DEVICE_IN_CLAUDE: &str =
    "app 是从 Claude Code 里启动的（有 {var}），不能在这里批准：请在你自己的终端运行 {command}";
/// The card was not shown long enough.
pub const DEVICE_TOO_FAST: &str = "审核单要完整显示至少 2 秒才能签字";
/// Touch ID did not pass.
pub const DEVICE_FAILED: &str = "触控 ID 没有通过（{why}），没有批准";
/// How many ancestors [`process_chain`] records.
pub const MAX_ANCESTORS: usize = 6;

/// The Claude Code variable present according to `is_set`, if any.
pub fn in_claude_code_with(is_set: impl Fn(&str) -> bool) -> Option<&'static str> {
    CLAUDE_ENV_VARS.iter().copied().find(|v| is_set(v))
}

/// The Claude Code variable present in the process environment, if any.
pub fn in_claude_code() -> Option<&'static str> {
    in_claude_code_with(|v| std::env::var_os(v).is_some())
}

/// The rule of [`require_terminal`] with its inputs made explicit (for tests and the app).
pub fn check_terminal(command: &str, claude_var: Option<&str>, stdin_is_tty: bool) -> Result<()> {
    if claude_var.is_some() || !stdin_is_tty {
        return Err(Error::NotHuman(REFUSAL.replace("{command}", command)));
    }
    Ok(())
}

/// Refuse inside Claude Code or without a TTY on stdin (Python `require_terminal`).
pub fn require_terminal(command: &str) -> Result<()> {
    check_terminal(command, in_claude_code(), std::io::stdin().is_terminal())
}

/// Prompt on stderr (stdout stays clean) and read one line from stdin, stripped.
pub fn read_confirmation(prompt: &str) -> Result<String> {
    let mut err = std::io::stderr();
    let _ = std::io::Write::write_all(&mut err, prompt.as_bytes());
    let _ = std::io::Write::flush(&mut err);
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Err(Error::Arb(
            "no confirmation typed (end of input); nothing was changed".into(),
        ));
    }
    Ok(crate::records::py_strip(&line).to_string())
}

/// What the human typed must be `expected` (the [`MISMATCH`] refusal otherwise).
pub fn check_typed(got: &str, expected: &str) -> Result<()> {
    if got != expected {
        return Err(Error::Arb(
            MISMATCH
                .replace("{got}", &crate::records::py_repr(got))
                .replace("{expected}", expected),
        ));
    }
    Ok(())
}

/// The human types `expected` (a run or lesson id) on stdin; prompt on stderr.
pub fn confirm_typed(expected: &str) -> Result<()> {
    let got = read_confirmation(&format!("输入 {expected} 确认："))?;
    check_typed(&got, expected)
}

/// One process of the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub comm: Option<String>,
}

/// `(parent pid, command name)` of `pid` from `/proc`, else `ps`.
fn proc_parent(pid: u32) -> Option<(u32, String)> {
    if let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // "pid (comm) state ppid ..." where comm may itself contain spaces or ')'
        if let (Some(open), Some(close)) = (text.find('('), text.rfind(')')) {
            let comm = text[open + 1..close].to_string();
            if let Some(ppid) = text
                .get(close + 2..)
                .and_then(|rest| rest.split_whitespace().nth(1))
                .and_then(|p| p.parse().ok())
            {
                return Some((ppid, comm));
            }
        }
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    let (ppid, comm) = text.split_once(' ').unwrap_or((text, ""));
    Some((ppid.trim().parse().ok()?, comm.trim().to_string()))
}

/// This process and up to `limit` ancestors, best effort (`/proc`, else `ps`).
pub fn process_chain(limit: usize) -> Vec<Proc> {
    let mut pid = std::process::id();
    let mut chain = Vec::new();
    for _ in 0..=limit {
        let Some((ppid, comm)) = proc_parent(pid) else {
            chain.push(Proc { pid, comm: None });
            break;
        };
        chain.push(Proc {
            pid,
            comm: Some(comm),
        });
        if ppid == 0 || ppid == pid {
            break;
        }
        pid = ppid;
    }
    chain
}

/// What is recorded in the Decision payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Provenance {
    pub author: String,
    pub pid: u32,
    pub process_chain: Vec<Proc>,
    pub confirmed_by: String,
}

/// The provenance of this process.
pub fn provenance() -> Provenance {
    Provenance {
        author: crate::records::AUTHOR_HUMAN.into(),
        pid: std::process::id(),
        process_chain: process_chain(MAX_ANCESTORS),
        confirmed_by: "typed id on a TTY".into(),
    }
}

/// Proof that the human typed the id in their own terminal just now. Only
/// [`confirm`] makes one (plus a test-only constructor in this crate's unit tests), so
/// no library caller can approve on the human's behalf.
#[derive(Debug, Clone)]
pub struct Confirmed {
    provenance: Provenance,
}

impl Confirmed {
    /// What goes into the Decision payload.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// For this crate's unit tests only (never in a build of the library).
    #[cfg(test)]
    pub(crate) fn for_tests() -> Confirmed {
        Confirmed {
            provenance: Provenance {
                author: crate::records::AUTHOR_HUMAN.into(),
                pid: 1,
                process_chain: Vec::new(),
                confirmed_by: "unit test".into(),
            },
        }
    }
}

/// [`confirm_typed`], then the provenance as a [`Confirmed`].
pub fn confirm(expected: &str) -> Result<Confirmed> {
    confirm_typed(expected)?;
    Ok(Confirmed {
        provenance: provenance(),
    })
}

/// The rule of [`confirm_on_device`] before Touch ID, inputs explicit (tests).
pub fn check_device(
    command: &str,
    claude_var: Option<&str>,
    shown: std::time::Duration,
) -> Result<()> {
    if let Some(var) = claude_var {
        return Err(Error::NotHuman(
            DEVICE_IN_CLAUDE
                .replace("{var}", var)
                .replace("{command}", command),
        ));
    }
    if shown < MIN_SHOWN {
        return Err(Error::NotHuman(DEVICE_TOO_FAST.into()));
    }
    Ok(())
}

/// What the Touch ID prompt says (the run and the card the human just read).
pub fn device_reason(run_id: &str, card_hash: &str) -> String {
    let short: String = card_hash.chars().take(8).collect();
    format!("批准 run {run_id}（审核单 {short}）提交到集群")
}

/// Whether [`confirm_on_device`] can ask here (macOS with Touch ID or a password the
/// system will take); Err says why not. Asks the system, shows nothing.
pub fn device_available() -> std::result::Result<(), String> {
    arboria_macos::available()
}

/// The app's approval (ADR 0004): the human read the card with `card_hash` for `shown`
/// and confirms with Touch ID (or the account password). Refused inside Claude Code, for
/// a card shown under [`MIN_SHOWN`], and on systems without Touch ID.
pub fn confirm_on_device(
    run_id: &str,
    card_hash: &str,
    shown: std::time::Duration,
) -> Result<Confirmed> {
    let command = format!("arb approve {run_id}");
    check_device(&command, in_claude_code(), shown)?;
    if !cfg!(target_os = "macos") {
        return Err(Error::NotHuman(
            DEVICE_UNAVAILABLE
                .replace("{why}", "不是 Mac")
                .replace("{command}", &command),
        ));
    }
    arboria_macos::authenticate(&device_reason(run_id, card_hash))
        .map_err(|why| Error::NotHuman(DEVICE_FAILED.replace("{why}", &why)))?;
    let mut provenance = provenance();
    provenance.confirmed_by = "Touch ID in Arboria.app".into();
    Ok(Confirmed { provenance })
}

/// [`require_terminal`], then [`confirm_typed`]; returns the provenance.
pub fn require_human(command: &str, expected: &str) -> Result<Provenance> {
    require_terminal(command)?;
    Ok(confirm(expected)?.provenance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_inside_claude_code_and_without_tty() {
        assert!(check_terminal("arb approve r001", None, true).is_ok());
        let e = check_terminal("arb approve r001", Some("CLAUDECODE"), true).unwrap_err();
        assert!(matches!(e, Error::NotHuman(_)));
        assert_eq!(
            e.to_string(),
            "批准只能在你自己的终端里做：请在终端运行 arb approve r001"
        );
        assert!(check_terminal("arb approve r001", Some("CLAUDE_CODE_ENTRYPOINT"), true).is_err());
        assert!(check_terminal("arb lesson accept l001", None, false).is_err());
    }

    #[test]
    fn device_approval_refusals() {
        use std::time::Duration;
        let cmd = "arb approve r001";
        assert!(check_device(cmd, None, Duration::from_secs(2)).is_ok());
        assert_eq!(
            check_device(cmd, Some("CLAUDECODE"), Duration::from_secs(9))
                .unwrap_err()
                .to_string(),
            "app 是从 Claude Code 里启动的（有 CLAUDECODE），不能在这里批准：请在你自己的终端运行 arb approve r001"
        );
        assert_eq!(
            check_device(cmd, None, Duration::from_millis(1999))
                .unwrap_err()
                .to_string(),
            DEVICE_TOO_FAST
        );
        assert_eq!(
            device_reason("r014", "3f9ac21e77aa"),
            "批准 run r014（审核单 3f9ac21e）提交到集群"
        );
        if !cfg!(target_os = "macos") && in_claude_code().is_none() {
            let e = confirm_on_device("r001", "abc", Duration::from_secs(3)).unwrap_err();
            assert!(matches!(e, Error::NotHuman(_)));
            assert_eq!(
                e.to_string(),
                "这里不能用触控 ID 批准（不是 Mac）：请在你自己的终端运行 arb approve r001"
            );
        }
    }

    #[test]
    fn typed_confirmation_and_process_chain() {
        assert!(check_typed("r001", "r001").is_ok());
        assert_eq!(
            check_typed("r00", "r001").unwrap_err().to_string(),
            "输入的 'r00' 与 r001 不符，已取消（没有做任何改动）"
        );
        let chain = process_chain(MAX_ANCESTORS);
        assert_eq!(chain[0].pid, std::process::id());
        assert!(chain.len() <= MAX_ANCESTORS + 1);
        let p = provenance();
        assert_eq!(
            (p.author.as_str(), p.confirmed_by.as_str()),
            ("human", "typed id on a TTY")
        );
    }

    #[test]
    fn any_claude_variable_counts_even_empty() {
        assert_eq!(
            in_claude_code_with(|v| v == "CLAUDE_CODE_ENTRYPOINT"),
            Some("CLAUDE_CODE_ENTRYPOINT")
        );
        assert_eq!(
            in_claude_code_with(|v| v == "CLAUDECODE"),
            Some("CLAUDECODE")
        );
        assert_eq!(in_claude_code_with(|_| false), None);
        assert_eq!(
            in_claude_code_with(|v| v == "CLAUDE_CODE_MESSAGING_TOKEN"),
            None
        );
    }
}
