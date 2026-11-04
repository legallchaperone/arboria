//! `arb init`, the managed CLAUDE.md block and lessons against real git repos (mirrors
//! tests/test_init_paths_db.py, tests/test_claudemd.py and tests/test_lessons.py where the
//! behaviour is the core's). Text Python writes is compared with CPython when the repo's
//! Python package can be imported.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use arboria_core::claudemd::{self, BEGIN, END};
use arboria_core::db::Ledger;
use arboria_core::lessons;
use arboria_core::model::{EventKind, LessonId, LessonStatus};
use arboria_core::paths::{self, RepoContext};
use arboria_core::{config, state};
use common::TempDir;

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

/// stdout of `python3 -c code` with the repo's `src` importable, None without Python.
fn python(code: &str) -> Option<String> {
    let out = Command::new("python3")
        .args(["-c", code])
        .env("PYTHONPATH", common::repo_root().join("src"))
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "python not compared: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(String::from_utf8(out.stdout).unwrap())
}

/// `<tmp>/WaveFront`: one commit, a configured identity (the CLAUDE.md commit needs one).
fn repo() -> (TempDir, PathBuf, RepoContext) {
    let tmp = TempDir::new("lessons");
    let main = tmp.path().canonicalize().unwrap().join("WaveFront");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["config", "user.name", "t"]);
    git(&main, &["config", "user.email", "t@t"]);
    std::fs::write(main.join("train.py"), "print('train')\n").unwrap();
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "init"]);
    let ctx = paths::locate(Some(&main)).unwrap();
    (tmp, main, ctx)
}

fn init(ctx: &RepoContext) -> Vec<String> {
    let mut warnings = Vec::new();
    let actions = state::init_state(ctx, &mut |w| warnings.push(w.to_string())).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    actions
}

#[test]
fn init_is_idempotent_and_writes_pythons_template() {
    let (_tmp, main, ctx) = repo();
    let actions = init(&ctx);
    let state_dir = ctx.state_dir();
    let pid = config::require_project_id(&ctx).unwrap();
    let exclude = ctx.common_dir.join("info").join("exclude");
    let config_path = ctx.config_path();
    let claude = main.join("CLAUDE.md");
    assert_eq!(
        actions,
        vec![
            format!("created {}", state_dir.display()),
            format!("added /.arboria/ to {}", exclude.display()),
            format!("initialized backup repo in {}", state_dir.display()),
            format!("created ledger {}", ctx.db_path().display()),
            "committed initial ledger backup".to_string(),
            format!(
                "wrote config template {} (commit it)",
                config_path.display()
            ),
            format!("created arboria block in {}", claude.display()),
            "committed CLAUDE.md in the main worktree".to_string(),
        ]
    );
    assert!(init(&ctx).is_empty(), "second init does nothing");
    assert_eq!(
        git(&main, &["log", "-1", "--format=%s"]),
        claudemd::COMMIT_MESSAGE
    );
    assert_eq!(git(&main, &["status", "--porcelain"]), "?? arboria.toml");

    let template = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(template, state::config_template(&pid));
    let py = python(&format!(
        "import sys; from arboria import state; sys.stdout.write(state.config_template('{pid}'))"
    ));
    if let Some(py) = py {
        assert_eq!(template, py);
    }
    let block = std::fs::read_to_string(&claude).unwrap();
    if let Some(py) = python(
        "import sys; from arboria import claudemd; sys.stdout.write(claudemd.render_block([]))",
    ) {
        assert_eq!(block, py);
    }
}

#[test]
fn init_adds_a_project_id_to_an_old_config_and_keeps_its_text() {
    let (_tmp, _main, ctx) = repo();
    std::fs::write(ctx.config_path(), "# mine\ndefault_machine = \"deltaai\"\n").unwrap();
    let actions = init(&ctx);
    let pid = config::require_project_id(&ctx).unwrap();
    assert!(actions.contains(&format!(
        "added [project] id = \"{pid}\" to {} (commit it)",
        ctx.config_path().display()
    )));
    let text = std::fs::read_to_string(ctx.config_path()).unwrap();
    assert!(
        text.starts_with("# mine\ndefault_machine = \"deltaai\"\n"),
        "{text}"
    );
}

#[test]
fn bad_markers_only_warn_in_init() {
    let (_tmp, main, ctx) = repo();
    std::fs::write(main.join("CLAUDE.md"), format!("mine\n{END}\n")).unwrap();
    let mut warnings = Vec::new();
    state::init_state(&ctx, &mut |w| warnings.push(w.to_string())).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].starts_with(&format!(
            "CLAUDE.md not updated: {} has an arboria:end marker without arboria:begin; fix the markers",
            main.join("CLAUDE.md").display()
        )),
        "{warnings:?}"
    );
}

#[test]
fn block_keeps_the_users_bytes() {
    let block = claudemd::render_block(&[]).unwrap();
    // appended after a blank line, whatever the file ended with
    for (old, sep) in [("a", "\n\n"), ("a\n", "\n"), ("a\n\n", "")] {
        let (new, action) = claudemd::apply_block(Some(old.as_bytes()), &block, "x").unwrap();
        assert_eq!(action, "appended");
        assert_eq!(new, format!("{old}{sep}{block}").into_bytes());
    }
    assert_eq!(
        claudemd::apply_block(None, &block, "x").unwrap(),
        (block.clone().into_bytes(), "created")
    );
    // replaced in place; invalid UTF-8 and \r\n around it are kept byte for byte
    let mut old = b"\xff\xfe mine\r\n".to_vec();
    old.extend_from_slice(format!("  {BEGIN}\r\nstale\n{END}\r\n").as_bytes());
    old.extend_from_slice(b"tail \xc3\n");
    let (new, action) = claudemd::apply_block(Some(&old), &block, "x").unwrap();
    assert_eq!(action, "replaced");
    let mut want = b"\xff\xfe mine\r\n".to_vec();
    want.extend_from_slice(block.as_bytes());
    want.extend_from_slice(b"tail \xc3\n");
    assert_eq!(new, want);
    // markers are never repaired
    for (text, says) in [
        (
            format!("{BEGIN}\n{BEGIN}\n{END}\n"),
            "x has 2 arboria:begin and 1 arboria:end markers (expected one of each)",
        ),
        (
            format!("{BEGIN}\n"),
            "x has an arboria:begin marker without arboria:end",
        ),
        (
            format!("{END}\n{BEGIN}\n"),
            "x has arboria:end before arboria:begin",
        ),
    ] {
        let err = claudemd::apply_block(Some(text.as_bytes()), &block, "x")
            .unwrap_err()
            .to_string();
        assert!(err.starts_with(says), "{err}");
        assert!(err.ends_with("then run `arb claudemd sync`"), "{err}");
    }
}

#[test]
fn sync_skips_the_commit_with_a_hint() {
    let (_tmp, main, ctx) = repo();
    init(&ctx);
    let ledger = Ledger::open(&ctx.state_dir()).unwrap();
    // autocommit off: written, not committed
    std::fs::write(main.join("CLAUDE.md"), "mine\n").unwrap();
    let cfg = std::fs::read_to_string(ctx.config_path())
        .unwrap()
        .replace("autocommit = true", "autocommit = false");
    std::fs::write(ctx.config_path(), cfg).unwrap();
    let r = claudemd::sync(&ctx, &ledger, true).unwrap();
    assert_eq!(
        r.messages(),
        vec![
            format!(
                "appended arboria block in {}",
                main.join("CLAUDE.md").display()
            ),
            "hint: [claudemd] autocommit is off; commit CLAUDE.md yourself".to_string(),
        ]
    );
    let r = claudemd::sync(&ctx, &ledger, true).unwrap();
    assert_eq!(
        r.messages(),
        vec![format!(
            "{} already up to date",
            main.join("CLAUDE.md").display()
        )]
    );
    // detached HEAD: skipped with a hint
    let cfg = std::fs::read_to_string(ctx.config_path())
        .unwrap()
        .replace("autocommit = false", "autocommit = true");
    std::fs::write(ctx.config_path(), cfg).unwrap();
    std::fs::write(main.join("CLAUDE.md"), "other\n").unwrap();
    git(&main, &["checkout", "-q", "--detach"]);
    let r = claudemd::sync(&ctx, &ledger, true).unwrap();
    assert!(!r.committed);
    assert_eq!(
        r.hints,
        vec!["the main worktree is on a detached HEAD; commit CLAUDE.md yourself later"]
    );
    // a bad autocommit value is refused before anything is written
    std::fs::write(ctx.config_path(), "[claudemd]\nautocommit = 1\n").unwrap();
    std::fs::write(main.join("CLAUDE.md"), "third\n").unwrap();
    let err = claudemd::sync(&ctx, &ledger, true).unwrap_err().to_string();
    assert_eq!(
        err,
        format!(
            "{}: [claudemd] autocommit must be true or false",
            ctx.config_path().display()
        )
    );
    assert_eq!(
        std::fs::read_to_string(main.join("CLAUDE.md")).unwrap(),
        "third\n"
    );
}

#[test]
fn propose_reject_retire() {
    let (_tmp, _main, ctx) = repo();
    init(&ctx);
    let mut ledger = Ledger::open(&ctx.state_dir()).unwrap();
    let (l1, d1) = lessons::propose(&ctx, &mut ledger, "  use bf16 on ghx4  ", None, None).unwrap();
    assert_eq!((l1.id.as_str(), d1.as_str()), ("l001", "d001"));
    assert_eq!(l1.text, "use bf16 on ghx4");
    assert_eq!(l1.source_type.as_str(), "none");
    assert_eq!(l1.status, LessonStatus::Proposed);
    assert_eq!(
        lessons::propose(&ctx, &mut ledger, "use bf16 on ghx4", None, None)
            .unwrap_err()
            .to_string(),
        "the same lesson already exists as l001 (proposed)"
    );
    assert_eq!(
        lessons::propose(&ctx, &mut ledger, "x", Some("nope"), None)
            .unwrap_err()
            .to_string(),
        "no idea named 'nope' (see `arb idea list`)"
    );
    assert!(lessons::propose(&ctx, &mut ledger, "a <!-- b", None, None).is_err());
    assert_eq!(lessons::proposed_count(&ledger).unwrap(), 1);

    let id = LessonId::parse("l001").unwrap();
    assert_eq!(
        lessons::retire(&ctx, &mut ledger, &id)
            .unwrap_err()
            .to_string(),
        "lesson l001 is proposed, not accepted; use `arb lesson reject` for a proposed one"
    );
    let r = lessons::reject(&ctx, &mut ledger, &id).unwrap();
    assert_eq!(r.lesson.status, LessonStatus::Retired);
    assert!(r.sync.is_none());
    let (choice, by): (String, String) = ledger
        .conn()
        .query_row(
            "SELECT choice, decided_by FROM decisions WHERE id = 'd001'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((choice.as_str(), by.as_str()), ("reject", "human"));
    let kinds: Vec<String> = ledger
        .conn()
        .prepare("SELECT kind FROM events WHERE subject = 'l001' ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        kinds,
        [EventKind::LessonPropose, EventKind::LessonReject].map(|k| k.as_str().to_string())
    );
    assert_eq!(
        lessons::get_lesson(&ledger, &LessonId::parse("l009").unwrap())
            .unwrap_err()
            .to_string(),
        "no lesson 'l009' (see `arb lesson list --all`)"
    );
    // a rejected lesson may be proposed again; list hides it unless asked
    lessons::propose(&ctx, &mut ledger, "use bf16 on ghx4", None, None).unwrap();
    assert_eq!(lessons::list_lessons(&ledger, false).unwrap().len(), 1);
    assert_eq!(lessons::list_lessons(&ledger, true).unwrap().len(), 2);
}
