//! Checks that a human, in their own terminal, is the one approving. Ports
//! `arboria.human`.
//!
//! Invariants:
//! - `arb approve` and `arb lesson accept` call [`require_terminal`] before anything is
//!   recorded: refused when `CLAUDECODE` or `CLAUDE_CODE_ENTRYPOINT` is present in the
//!   environment (any value, even empty), or when stdin is not a TTY. Then the human
//!   types the id ([`confirm_typed`]). The provenance goes into the Decision payload.
//! - There is no approval function anywhere else, and no code path that approves
//!   automatically. The app's approval panel (Touch ID) will be the second entry point.
//! - A guard rail, not a sandbox: the hard guarantee is
//!   [`crate::submission`]'s guarded submit, which needs a human Decision in the ledger.
//!
//! Python also refuses while a notebook command (`lab.<name>`) runs in the same process.
//! That registry lives in the Python kernel, which is a separate process from the Rust
//! core, so it stays there; the Rust CLI never runs command code.
//!
//! Real in phase 1: [`in_claude_code`], [`check_terminal`], [`require_terminal`].

use std::io::IsTerminal;

use serde::Serialize;

use crate::{Error, Result};

/// The variables Claude Code sets for its tools.
pub const CLAUDE_ENV_VARS: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"];
/// `{command}` is the command to run in the human's own terminal.
pub const REFUSAL: &str = "批准只能在你自己的终端里做：请在终端运行 {command}";
/// A mistyped confirmation.
pub const MISMATCH: &str = "输入的 {got} 与 {expected} 不符，已取消（没有做任何改动）";
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

/// The human types `expected` (a run or lesson id) on stdin; prompt on stderr.
pub fn confirm_typed(expected: &str) -> Result<()> {
    let _ = expected;
    todo!("port of arboria.human.confirm_typed")
}

/// One process of the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub comm: Option<String>,
}

/// This process and up to `limit` ancestors, best effort (`/proc`, else `ps`).
pub fn process_chain(limit: usize) -> Vec<Proc> {
    let _ = limit;
    todo!("port of arboria.human.process_chain")
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
    todo!("port of arboria.human.provenance")
}

/// [`require_terminal`], then [`confirm_typed`]; returns the provenance.
pub fn require_human(command: &str, expected: &str) -> Result<Provenance> {
    let _ = (command, expected);
    todo!("port of arboria.human.require_human")
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
