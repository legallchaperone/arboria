//! Tell the window when a ledger changed (ADR 0004): every second, `PRAGMA data_version`
//! of each listed project's ledger (a connection kept open read-only) and the size of its
//! `notifications.log`; a change emits `ledger-changed` with the root. A change of the
//! project list emits `projects-changed`; a change of clusters.toml, `clusters-changed`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use arboria_core::{db, paths, registry};
use rusqlite::Connection;
use tauri::{AppHandle, Emitter};

struct Seen {
    conn: Option<Connection>,
    version: i64,
    log_len: u64,
}

fn roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = registry::list()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.path)
        .collect();
    let home = paths::home();
    if home.initialized() {
        out.push(home.root);
    }
    out
}

/// Start the watcher thread.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut seen: HashMap<PathBuf, Seen> = HashMap::new();
        let mut listed: Vec<PathBuf> = Vec::new();
        let mtime = || {
            std::fs::metadata(arboria_core::clusters::config_path())
                .and_then(|m| m.modified())
                .ok()
        };
        let mut clusters_seen = mtime();
        loop {
            let now = roots();
            if now != listed {
                let _ = app.emit("projects-changed", ());
                seen.retain(|r, _| now.contains(r));
                listed = now.clone();
            }
            for root in &now {
                let state = root.join(paths::STATE_DIR_NAME);
                let entry = seen.entry(root.clone()).or_insert(Seen {
                    conn: None,
                    version: -1,
                    log_len: 0,
                });
                if entry.conn.is_none() {
                    entry.conn = db::open_read_only(&state.join(db::DB_NAME)).ok();
                }
                let version = entry
                    .conn
                    .as_ref()
                    .and_then(|c| db::data_version(c).ok())
                    .unwrap_or(-1);
                if version < 0 {
                    entry.conn = None; // reopen next time (migrated, moved)
                }
                let log_len = std::fs::metadata(state.join(arboria_core::monitor::NOTIFICATIONS))
                    .map(|m| m.len())
                    .unwrap_or(0);
                let changed =
                    entry.version != -1 && (version != entry.version || log_len != entry.log_len);
                entry.version = version;
                entry.log_len = log_len;
                if changed {
                    let _ = app.emit("ledger-changed", root.display().to_string());
                }
            }
            let now_m = mtime();
            if now_m != clusters_seen {
                clusters_seen = now_m;
                let _ = app.emit("clusters-changed", ());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}
