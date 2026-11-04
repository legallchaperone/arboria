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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::canonical::python_dumps;
use crate::db::{now, Ledger};
use crate::paths::{self, Home};
use crate::report;

/// The context budget (code points).
pub const MAX_CONTEXT_CHARS: usize = 4000;
/// Appended to a truncated context.
pub const TRUNCATED: &str = "\n… (truncated; run `arb status` for the full view)";
/// The error log in the state dir.
pub const LOG_NAME: &str = "hook.log";

/// `text` cut to `limit` code points, ending with [`TRUNCATED`] when cut.
pub fn cap(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let keep = limit.saturating_sub(TRUNCATED.chars().count());
    text.chars().take(keep).collect::<String>() + TRUNCATED
}

/// The cwd named by the hook's stdin JSON, else the process cwd.
pub fn hook_cwd(stdin_text: &str) -> PathBuf {
    if !crate::records::py_strip(stdin_text).is_empty() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdin_text) {
            if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                if !cwd.is_empty() && Path::new(cwd).is_dir() {
                    return PathBuf::from(cwd);
                }
            }
        }
    }
    std::env::current_dir().unwrap_or_default()
}

fn output_json(text: &str) -> String {
    python_dumps(&json!({"hookSpecificOutput": {
        "hookEventName": "SessionStart",
        "additionalContext": text,
    }}))
}

/// The stdout JSON for `cwd`, or None (neither in an initialized repo nor in the seed
/// home). Errors are the caller's to swallow.
pub fn build_output(cwd: &Path) -> crate::Result<Option<String>> {
    let h = paths::home();
    if h.contains(cwd) {
        return build_home_output(&h);
    }
    let Ok(ctx) = paths::locate(Some(cwd)) else {
        return Ok(None); // not a git repo, git missing, ...
    };
    if !ctx.initialized() {
        return Ok(None);
    }
    let ledger = Ledger::open(&ctx.state_dir())?;
    let data = report::status_data(&ctx, &ledger)?;
    let (head, tail) = report::status_parts(&data, true);
    // lessons + rules (tail) are kept whole; the state part is truncated to fit
    let budget = MAX_CONTEXT_CHARS
        .saturating_sub(tail.chars().count() + 1)
        .max(500);
    let text = cap(&(cap(&head, budget) + "\n" + &tail), MAX_CONTEXT_CHARS);
    Ok(Some(output_json(&text)))
}

/// The home's output (the seeds and their todos).
pub fn build_home_output(home: &Home) -> crate::Result<Option<String>> {
    if !home.initialized() {
        return Ok(None);
    }
    let ledger = Ledger::open(&home.state_dir())?;
    let text = report::home_status(home, &ledger, true)?;
    Ok(Some(output_json(&cap(&text, MAX_CONTEXT_CHARS))))
}

/// Append `what` to the hook log of the state dir `cwd` belongs to (best effort, silent).
fn log_error(cwd: Option<&Path>, what: &str) {
    let state = (|| {
        let cwd = cwd?;
        let h = paths::home();
        if h.contains(cwd) {
            Some(h.state_dir())
        } else {
            paths::locate(Some(cwd)).ok().map(|c| c.state_dir())
        }
    })();
    let Some(state) = state.filter(|s| s.is_dir()) else {
        return;
    };
    let shown = cwd.map_or("None".to_string(), |c| c.display().to_string());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join(LOG_NAME))
        .and_then(|mut f| {
            writeln!(f, "--- {} session-start hook error (cwd={shown})", now())?;
            writeln!(f, "{what}")
        });
}

/// Run the hook: read `stdin` (the caller passes an empty reader when stdin is a TTY),
/// write the JSON line. Always returns exit status 0; never panics outward (the body
/// runs under `catch_unwind`, with a silent panic hook so nothing reaches stderr).
pub fn session_start(stdin: &mut dyn Read, stdout: &mut dyn Write) -> i32 {
    let mut cwd: Option<PathBuf> = None;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut text = String::new();
        let _ = stdin.read_to_string(&mut text);
        cwd = Some(hook_cwd(&text));
        let out = build_output(cwd.as_deref().expect("set"))?;
        if let Some(out) = out {
            writeln!(stdout, "{out}")?;
            stdout.flush()?;
        }
        Ok::<(), crate::Error>(())
    }));
    std::panic::set_hook(previous);
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log_error(cwd.as_deref(), &e.to_string()),
        Err(panic) => {
            let msg = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            log_error(cwd.as_deref(), &format!("panic: {msg}"));
        }
    }
    0
}
