//! Arboria core: the library the Mac app, the `arb` CLI and the LaunchAgent helper all
//! link (ADR 0001). Synchronous, no daemon; the SQLite ledger is the single arbiter.
//!
//! Each module ports one Python module of `src/arboria/` and says so in its doc comment,
//! together with the invariants it must keep. Phase 1 (ADR 0002) settled the public API;
//! every module is now ported (`ui` runs the Python dashboard until the app replaces it).
//! `tests/vectors.rs` runs the Python test vectors against the real parts.
//!
//! Rules for porting a module: one module per agent; do not change another module's
//! public signatures without recording it in `docs/adr/0002-rust-contract.md`.

pub mod backend;
pub mod canonical;
pub mod cards;
pub mod claudemd;
pub mod clusters;
pub mod commands;
pub mod config;
pub mod db;
pub mod error;
pub mod gitutil;
pub mod hook;
pub mod human;
pub mod ideas;
pub mod inspection;
pub mod install;
pub mod lessons;
pub mod machine;
pub mod model;
pub mod monitor;
pub mod notebooks;
pub mod paths;
pub mod probe;
pub mod records;
pub mod report;
pub mod schema;
pub mod slurm;
pub mod snapshot;
pub mod state;
pub mod submission;
pub mod tasks;
pub mod ui;

pub use error::{Error, Result};

/// Where non-fatal warnings go (Python passes `warn=` callables; the CLI prints them as
/// `warning: …` on stderr, the app shows them in the inbox).
pub type Warn<'a> = &'a mut dyn FnMut(&str);
