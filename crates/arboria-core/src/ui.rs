//! `arb ui`: the read-only dashboard. Ports `arboria.ui.launch` only as far as the CLI
//! needs it; ADR 0001 replaces the marimo dashboard with the Mac app's views, which read
//! the ledger with [`crate::db::open_read_only`].
//!
//! Invariants:
//! - Read-only: the ledger is opened read-only; no control acts (approve, accept, cancel
//!   are shown as command text only); no network; ledger text is HTML-escaped.
//! - Binds to 127.0.0.1 only.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::Path;

use crate::Result;

/// The only address the dashboard listens on.
pub const HOST: &str = "127.0.0.1";

/// Open the project dashboard (`--idea` selects one); returns the exit status.
pub fn launch(
    root: &Path,
    toplevel: Option<&Path>,
    idea: Option<&str>,
    port: Option<u16>,
) -> Result<i32> {
    todo!("port of arboria.ui.launch.launch (to be replaced by the app)")
}

/// Open the seed home's dashboard (种子园).
pub fn launch_home(root: &Path, port: Option<u16>) -> Result<i32> {
    todo!("port of arboria.ui.launch.launch_home (to be replaced by the app)")
}
