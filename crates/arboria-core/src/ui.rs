//! `arb ui`: the read-only dashboard. Ports `arboria.ui.launch` only as far as the CLI
//! needs it; ADR 0001 replaces the marimo dashboard with the Mac app's views, which read
//! the ledger with [`crate::db::open_read_only`].
//!
//! Until the app's views land, `arb-rs ui` runs the Python dashboard: the interpreter
//! [`crate::commands::python_exe`] (`$ARBORIA_PYTHON`, default `python3`) must import
//! `arboria` with its `ui` extra (marimo). The Python dashboard reads schema v8 ledgers
//! read-only (`ui/data.py`), so it serves ledgers arb-rs writes.
//!
//! Invariants:
//! - Read-only: the ledger is opened read-only; no control acts (approve, accept, cancel
//!   are shown as command text only); no network; ledger text is HTML-escaped.
//! - Binds to 127.0.0.1 only.

use std::path::Path;
use std::process::Command;

use crate::{bail, Error, Result};

/// The only address the dashboard listens on.
pub const HOST: &str = "127.0.0.1";

/// What runs in the interpreter: `arboria.ui.launch` with arb's arguments (argv), its
/// errors printed like the CLI's.
const LAUNCH: &str = r#"
import sys
from pathlib import Path
try:
    from arboria.errors import ArbError
    from arboria.ui import launch
except ImportError as exc:
    print(f"error: arb ui runs the Python dashboard, but {sys.executable} cannot import it ({exc}); "
          "install arboria with its ui extra there, or point ARBORIA_PYTHON at an interpreter that has it",
          file=sys.stderr)
    sys.exit(1)
what, root, toplevel, idea, port = sys.argv[1:6]
port = int(port) if port else None
try:
    if what == "home":
        code = launch.launch_home(Path(root), port=port)
    else:
        code = launch.launch(Path(root), toplevel=Path(toplevel) if toplevel else None,
                             idea=idea or None, port=port)
except ArbError as exc:
    print(f"error: {exc}", file=sys.stderr)
    sys.exit(1)
sys.exit(code)
"#;

fn check_port(port: Option<u16>) -> Result<()> {
    if port == Some(0) {
        bail!("--port must be between 1 and 65535");
    }
    Ok(())
}

fn run(
    what: &str,
    root: &Path,
    toplevel: Option<&Path>,
    idea: Option<&str>,
    port: Option<u16>,
) -> Result<i32> {
    check_port(port)?;
    let exe = crate::commands::python_exe();
    let status = Command::new(&exe)
        .args(["-c", LAUNCH, what])
        .arg(root)
        .arg(toplevel.map_or_else(Default::default, |t| t.as_os_str().to_owned()))
        .arg(idea.unwrap_or_default())
        .arg(port.map_or_else(String::new, |p| p.to_string()))
        .status()
        .map_err(|e| {
            Error::Arb(format!(
                "arb ui runs the Python dashboard: cannot run {exe} ({e}); set ARBORIA_PYTHON"
            ))
        })?;
    Ok(status.code().unwrap_or(1))
}

/// Open the project dashboard (`--idea` selects one); returns the exit status.
pub fn launch(
    root: &Path,
    toplevel: Option<&Path>,
    idea: Option<&str>,
    port: Option<u16>,
) -> Result<i32> {
    run("project", root, toplevel, idea, port)
}

/// Open the seed home's dashboard (种子园).
pub fn launch_home(root: &Path, port: Option<u16>) -> Result<i32> {
    run("home", root, None, None, port)
}
