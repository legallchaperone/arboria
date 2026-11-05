//! `arb status`, `arb export` and the SessionStart hook, checked against CPython: a
//! Python-made v7 ledger is reported by the repo's Python package first, then opened (and
//! migrated to v8) by the Rust core, and the texts must agree. Skipped without Python.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use arboria_core::db::Ledger;
use arboria_core::paths;
use arboria_core::{hook, report};
use common::TempDir;
use serde_json::Value;

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end()
        .to_string()
}

/// Rows the Python side inserts (v7 columns); `{wt}` is the base idea's worktree.
const ROWS: &str = r#"
INSERT INTO ideas (id, name, hypothesis, metric, branch, worktree_path, created_at, confidence)
    VALUES (1, 'base', 'wider   layers
help', 'val loss', 'idea/base', '{wt}', '2026-11-01T10:00:00Z', 'prelim');
INSERT INTO ideas (id, name, hypothesis, metric, parent_id, branch, worktree_path, created_at)
    VALUES (2, 'deep', 'deeper is better', 'acc', 1, 'idea/deep', '/w/deep', '2026-11-01T11:00:00Z');
INSERT INTO ideas (id, name, hypothesis, metric, status, branch, worktree_path, created_at, closed_at)
    VALUES (3, 'gone', 'h', 'm', 'dropped', 'idea/gone', '/w/gone', '2026-10-01T10:00:00Z',
            '2026-10-02T10:00:00Z');
INSERT INTO runs (id, idea_id, snapshot_commit, tree_hash, change_reason, status, machine,
                  "partition", gpus, walltime, est_cost, actual_cost, summary,
                  created_at, updated_at)
    VALUES ('r001', 1, 'abcdef0123456789', 't1', 'baseline', 'succeeded', 'deltaai', 'ghx4',
            4, '12:00:00', 48.0, 3.2049, 'loss 1.23
then flat', '2026-11-01T12:00:00Z', '2026-11-02T12:00:00Z'),
           ('r002', 1, 'abcdef0123456789', 't1', 'lr 3e-4', 'awaiting_approval', 'deltaai',
            'ghx4', 1, '01:00:00', 4.125, NULL, NULL, '2026-11-02T13:00:00Z',
            '2026-11-02T13:00:00Z'),
           ('r003', 2, '0123456789abcdef', 't2', 'draft one', 'draft', NULL, NULL, NULL, NULL,
            NULL, NULL, NULL, '2026-11-02T14:00:00Z', '2026-11-02T14:00:00Z');
INSERT INTO jobs (run_id, segment, job_name, job_id, state, elapsed_s, billed_su, updated_at)
    VALUES ('r001', 1, 'arb-x-r001-s1', '1001', 'completed', 3600, 1.6, '2026-11-02T12:00:00Z'),
           ('r001', 2, 'arb-x-r001-s2', NULL, 'submitting', NULL, NULL, '2026-11-02T12:00:00Z');
INSERT INTO notes (idea_id, run_id, text, confidence, created_at, author)
    VALUES (1, 'r001', 'looks   good', 'prelim', '2026-11-01T12:30:00Z', 'human'),
           (1, NULL, 'agent thought', NULL, '2026-11-01T12:40:00Z', 'claude');
INSERT INTO decisions (id, subject_type, subject_id, question, options_json, created_at)
    VALUES ('d001', 'run', 'r002', 'Submit r002 to deltaai?', '["approve", "reject"]',
            '2026-11-02T13:00:00Z'),
           ('d002', 'lesson', 'l003', '接受这条经验吗？', '["accept", "reject"]',
            '2026-11-02T13:00:00Z');
INSERT INTO lessons (id, text, source_type, source_id, status, created_at, decided_at)
    VALUES ('l001', 'pin the seed', 'idea', 'base', 'accepted', '2026-11-01T10:00:00Z',
            '2026-11-01T11:00:00Z'),
           ('l002', 'bf16 on ghx4', 'none', NULL, 'accepted', '2026-11-01T10:00:00Z', NULL),
           ('l003', 'warm up 1k steps', 'run', 'r001', 'proposed', '2026-11-02T10:00:00Z', NULL);
INSERT INTO tasks (id, idea_id, text, status, created_at, kind, payload_json)
    VALUES ('t001', 1, 'plot the   loss', 'open', '2026-11-02T10:00:00Z', 'plain', NULL),
           ('t002', 1, 'svd', 'open', '2026-11-02T10:01:00Z', 'command_new', '{"name": "svd"}'),
           ('t003', 2, 'where is the loss?', 'open', '2026-11-02T10:02:00Z', 'inspect',
            '{"question": "where is the loss computed?", "view": "pseudo"}'),
           ('t004', 2, 'old', 'done', '2026-11-02T10:03:00Z', 'plain', NULL);
INSERT INTO events (ts, kind, subject, payload_json)
    VALUES ('2026-11-02T12:00:00Z', 'run_final', 'r001', '{}');
"#;

const SCRIPT: &str = r#"
import json, os, sys
from pathlib import Path
from arboria import db, hook, report
from arboria.paths import locate
main, wt, rows = sys.argv[1], sys.argv[2], sys.argv[3]
ctx = locate(Path(main))
ledger = db.Ledger(ctx.state_dir)
ledger.conn.executescript("BEGIN;" + rows + "COMMIT;")
out = {}
for scope, cwd in (("overview", main), ("idea", wt)):
    c = locate(Path(cwd))
    data = report.status_data(c, ledger)
    out[scope] = {"full": report.format_status(data), "brief": report.format_status(data, brief=True),
                  "hook": hook.build_output(Path(cwd))}
out["export"] = report.export_markdown(ctx, ledger)
ledger.close()
print(json.dumps(out))
"#;

struct Fixture {
    _tmp: TempDir,
    main: PathBuf,
    wt: PathBuf,
    py: Value,
}

fn fixture() -> Option<Fixture> {
    let tmp = TempDir::new("report");
    let root = tmp.path().canonicalize().unwrap();
    let main = root.join("WaveFront");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let wt = root.join("WaveFront-base");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "idea/base",
            wt.to_str().unwrap(),
        ],
    );
    let ctx = paths::locate(Some(&main)).unwrap();
    std::fs::create_dir_all(ctx.state_dir()).unwrap();
    let rows = ROWS.replace("{wt}", wt.to_str().unwrap());
    let out = Command::new("python3")
        .args(["-c", SCRIPT])
        .arg(&main)
        .arg(&wt)
        .arg(&rows)
        .env("PYTHONPATH", common::repo_root().join("src"))
        .env("ARBORIA_HOME", root.join("home"))
        .env("ARBORIA_CONFIG_DIR", root.join("config"))
        .env_remove("CLAUDE_CODE_MESSAGING_SOCKET")
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "python not available, report not compared: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(Fixture {
        _tmp: tmp,
        main,
        wt,
        py: serde_json::from_slice(&out.stdout).unwrap(),
    })
}

fn py<'a>(f: &'a Fixture, path: &[&str]) -> &'a str {
    let mut v = &f.py;
    for k in path {
        v = &v[*k];
    }
    v.as_str().unwrap()
}

#[test]
fn status_export_and_hook_match_python() {
    let Some(f) = fixture() else { return };
    for (scope, cwd) in [("overview", &f.main), ("idea", &f.wt)] {
        let ctx = paths::locate(Some(cwd)).unwrap();
        let ledger = Ledger::open(&ctx.state_dir()).unwrap(); // migrates to v8
        let data = report::status_data(&ctx, &ledger).unwrap();
        assert_eq!(data.0["scope"], scope);
        assert_eq!(
            report::format_status(&data, false),
            py(&f, &[scope, "full"]),
            "{scope} full"
        );
        assert_eq!(
            report::format_status(&data, true),
            py(&f, &[scope, "brief"]),
            "{scope} brief"
        );
        let ours: Value = serde_json::from_str(&hook::build_output(cwd).unwrap().unwrap()).unwrap();
        let theirs: Value = serde_json::from_str(py(&f, &[scope, "hook"])).unwrap();
        assert_eq!(ours, theirs, "{scope} hook");
        // the JSON line itself is Python's (key order, separators, no ASCII escapes)
        assert_eq!(
            hook::build_output(cwd).unwrap().unwrap(),
            py(&f, &[scope, "hook"]),
            "{scope} hook line"
        );
    }
    let ctx = paths::locate(Some(&f.main)).unwrap();
    let ledger = Ledger::open(&ctx.state_dir()).unwrap();
    let drop_stamp = |s: &str| {
        s.lines()
            .filter(|l| !l.starts_with("_Exported "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        drop_stamp(&report::export_markdown(&ctx, &ledger).unwrap()),
        drop_stamp(py(&f, &["export"]))
    );
}

#[test]
fn hook_never_fails_and_says_nothing_outside_a_repo() {
    let tmp = TempDir::new("hook");
    let dir = tmp.path().canonicalize().unwrap();
    let mut out = Vec::new();
    let stdin = format!("{{\"cwd\": {:?}}}", dir.display().to_string());
    assert_eq!(hook::session_start(&mut stdin.as_bytes(), &mut out), 0);
    assert!(out.is_empty());
    // garbage on stdin: the process cwd is used, still exit 0
    let mut out = Vec::new();
    assert_eq!(hook::session_start(&mut &b"{not json"[..], &mut out), 0);

    // a broken ledger: exit 0, no output, the error goes to hook.log
    let main = dir.join("repo");
    std::fs::create_dir_all(main.join(".arboria")).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join(".arboria/ledger.db"), b"not a database").unwrap();
    let stdin = format!("{{\"cwd\": {:?}}}", main.display().to_string());
    let mut out = Vec::new();
    assert_eq!(hook::session_start(&mut stdin.as_bytes(), &mut out), 0);
    assert!(out.is_empty());
    let log = std::fs::read_to_string(main.join(".arboria").join(hook::LOG_NAME)).unwrap();
    assert!(log.starts_with("--- "), "{log}");
    assert!(log.contains("session-start hook error"), "{log}");
}

#[test]
fn cap_keeps_the_limit() {
    assert_eq!(hook::cap("abc", 3), "abc");
    let long = "字".repeat(5000);
    let capped = hook::cap(&long, hook::MAX_CONTEXT_CHARS);
    assert_eq!(capped.chars().count(), hook::MAX_CONTEXT_CHARS);
    assert!(capped.ends_with(hook::TRUNCATED));
}

#[test]
fn status_names_probes_to_reenable() {
    let tmp = TempDir::new("report-probe");
    let main = tmp.path().canonicalize().unwrap().join("WaveFront");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let ctx = paths::locate(Some(&main)).unwrap();
    let ledger = Ledger::open(&ctx.state_dir()).unwrap();
    let probes = ctx.state_dir().join("probes");
    std::fs::create_dir_all(&probes).unwrap();
    std::fs::write(
        probes.join("credits.py"),
        "# /// arb-probe\n# api = 1\n# machine = \"deltaai\"\n# argv = [\"accounts\"]\n# every = \"30m\"\n# kind = \"number\"\n# ///\n",
    )
    .unwrap();
    let data = report::status_data(&ctx, &ledger).unwrap();
    assert!(
        data.0.get("probes_to_reenable").is_none(),
        "absent when nothing waits"
    );
    ledger
        .conn()
        .execute(
            "INSERT INTO decisions (id, subject_type, subject_id, question, options_json, choice, \
             decided_by, created_at, bound_hash) VALUES ('d001', 'probe', 'project:credits', 'q', \
             '[]', 'enable', 'human', '2026-11-05T00:00:00Z', ?1)",
            ["0".repeat(64)],
        )
        .unwrap();
    let data = report::status_data(&ctx, &ledger).unwrap();
    let text = report::format_status(&data, false);
    assert!(
        text.contains("probe 待重新启用（文件改过，停用了）：credits——由你运行 `arb probe enable NAME`\n看板：arb ui"),
        "{text}"
    );
}
