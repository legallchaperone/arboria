//! Claude Code SessionStart hook: inject a brief ledger status into new sessions. Ports
//! `arboria.hook` (without the session-socket recording, which ADR 0001 removes).
//!
//! Invariants:
//! - **`arb hook session-start` never fails and prints nothing extra**: any error exits 0
//!   with no output, and is appended to `<state>/hook.log` (best effort, silent).
//! - Output is one JSON object `{"hookSpecificOutput": {"hookEventName": "SessionStart",
//!   "additionalContext": …}}` (`ensure_ascii=False`), or nothing when the cwd is neither
//!   in an initialized repo nor in the seed home.
//! - The context is at most [`MAX_CONTEXT_CHARS`] code points: lessons and rules (the
//!   tail) are kept whole; the state part is truncated with [`TRUNCATED`] to fit.
//! - The cwd comes from the hook's stdin JSON (`cwd`, if it is a directory), else the
//!   process cwd.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::paths::Home;

/// The context budget (code points).
pub const MAX_CONTEXT_CHARS: usize = 4000;
/// Appended to a truncated context.
pub const TRUNCATED: &str = "\n… (truncated; run `arb status` for the full view)";
/// The error log in the state dir.
pub const LOG_NAME: &str = "hook.log";

/// `text` cut to `limit` code points, ending with [`TRUNCATED`] when cut.
pub fn cap(text: &str, limit: usize) -> String {
    todo!("port of arboria.hook.cap")
}

/// The cwd named by the hook's stdin JSON, else the process cwd.
pub fn hook_cwd(stdin_text: &str) -> PathBuf {
    todo!("port of arboria.hook._hook_cwd")
}

/// The stdout JSON for `cwd`, or None. Errors are the caller's to swallow.
pub fn build_output(cwd: &Path) -> crate::Result<Option<String>> {
    todo!("port of arboria.hook.build_output")
}

/// The home's output (the seeds and their todos).
pub fn build_home_output(home: &Home) -> crate::Result<Option<String>> {
    todo!("port of arboria.hook.build_home_output")
}

/// Run the hook: read stdin (unless a TTY), write the JSON line. Always returns exit
/// status 0; never panics outward (the body runs under `catch_unwind`).
pub fn session_start(stdin: &mut dyn Read, stdout: &mut dyn Write) -> i32 {
    todo!("port of arboria.hook.session_start")
}
