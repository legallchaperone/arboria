//! snapshot + records against a real git repo and ledger (mirrors tests/test_snapshot.py
//! and the record parts of tests/test_runs_notes.py).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use arboria_core::db::{self, Ledger};
use arboria_core::model::{Confidence, DecisionId, Event, Idea, RunId, SubjectType};
use arboria_core::records::{self, SnapshotOptions};
use arboria_core::{snapshot, Error};
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

fn git_bytes(cwd: &Path, args: &[&str]) -> Vec<u8> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
        .stdout
}

struct Repo {
    _tmp: TempDir,
    wt: PathBuf,
    ledger: Ledger,
    idea: Idea,
}

/// A repo with one commit (train.py, README.md, .gitignore) and an idea row whose
/// worktree is the repo itself.
fn repo() -> Repo {
    let tmp = TempDir::new("records");
    let wt = tmp.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir_all(&wt).unwrap();
    git(&wt, &["init", "-q", "-b", "idea/x"]);
    std::fs::write(wt.join("train.py"), "print('train')\n").unwrap();
    std::fs::write(wt.join("README.md"), "# r\n").unwrap();
    std::fs::write(wt.join(".gitignore"), "*.ckpt\noutputs/\n.arboria/\n").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-q", "-m", "init"]);
    let mut ledger = Ledger::open(&wt.join(".arboria")).unwrap();
    db::init_backup_repo(ledger.state_dir()).unwrap();
    let wt_s = wt.display().to_string();
    ledger
        .write(|tx| {
            tx.execute(
                "INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, created_at, \
                 updated_at) VALUES ('x', 'h', 'm', 'idea/x', ?1, ?2, ?2)",
                rusqlite::params![wt_s, db::now()],
            )?;
            Ok(())
        })
        .unwrap();
    let idea: Idea = db::one(ledger.conn(), "SELECT * FROM ideas WHERE name = 'x'", [])
        .unwrap()
        .unwrap();
    Repo {
        _tmp: tmp,
        wt,
        ledger,
        idea,
    }
}

fn no_warn() -> impl FnMut(&str) {
    |w: &str| panic!("unexpected warning {w}")
}

fn new_run(r: &mut Repo, reason: &str) -> records::NewRun {
    records::new_run(
        &mut r.ledger,
        &r.idea,
        reason,
        SnapshotOptions::default(),
        &mut no_warn(),
    )
    .unwrap()
}

fn events(ledger: &Ledger) -> Vec<Event> {
    db::all(ledger.conn(), "SELECT * FROM events ORDER BY id", []).unwrap()
}

fn tree_files(wt: &Path, commit: &str) -> Vec<String> {
    git(wt, &["ls-tree", "-r", "--name-only", commit])
        .lines()
        .map(str::to_string)
        .collect()
}

#[derive(Debug, PartialEq)]
struct State {
    head: String,
    branch: String,
    cached: String,
    unstaged: String,
    status: String,
    index: Vec<u8>,
    branches: String,
}

fn state(wt: &Path) -> State {
    let index = git(
        wt,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    );
    State {
        head: git(wt, &["rev-parse", "HEAD"]),
        branch: git(wt, &["symbolic-ref", "HEAD"]),
        cached: git(wt, &["diff", "--cached"]),
        unstaged: git(wt, &["diff"]),
        status: git(wt, &["status", "--porcelain", "--untracked-files=all"]),
        index: std::fs::read(index).unwrap(),
        branches: git(wt, &["for-each-ref", "refs/heads"]),
    }
}

#[test]
fn snapshot_captures_everything_and_touches_nothing() {
    let mut r = repo();
    let wt = r.wt.clone();
    std::fs::write(wt.join("train.py"), "print('train v2')\n").unwrap();
    std::fs::write(wt.join("staged.py"), "x = 1\n").unwrap();
    git(&wt, &["add", "staged.py"]);
    std::fs::write(wt.join("README.md"), "# staged\n").unwrap();
    git(&wt, &["add", "README.md"]);
    std::fs::write(wt.join("README.md"), "# unstaged\n").unwrap();
    std::fs::write(wt.join("untracked.py"), "y = 2\n").unwrap();
    std::fs::create_dir_all(wt.join("pkg/deep")).unwrap();
    std::fs::write(wt.join("pkg/deep/mod.py"), "z = 3\n").unwrap();
    std::fs::write(wt.join("model.ckpt"), [0u8; 1000]).unwrap();
    std::fs::create_dir_all(wt.join("outputs")).unwrap();
    std::fs::write(wt.join("outputs/log.txt"), "ignored\n").unwrap();
    git(&wt, &["rm", "-q", "--cached", ".gitignore"]);
    let before = state(&wt);

    let run = new_run(&mut r, "  try warmup ");
    assert_eq!(run.run.id.as_str(), "r001");
    assert_eq!(run.run.change_reason, "try warmup");
    assert_eq!(run.ref_name, "refs/arboria/runs/r001");
    assert!(!run.reused_snapshot);
    assert_eq!(
        state(&wt),
        before,
        "snapshot modified branch/index/working tree"
    );

    let commit = &run.run.snapshot_commit;
    let show = |p: &str| git(&wt, &["show", &format!("{commit}:{p}")]);
    assert_eq!(show("train.py"), "print('train v2')");
    assert_eq!(show("staged.py"), "x = 1");
    assert_eq!(show("README.md"), "# unstaged");
    assert_eq!(show("untracked.py"), "y = 2");
    assert_eq!(show("pkg/deep/mod.py"), "z = 3");
    let files = tree_files(&wt, commit);
    assert!(files.contains(&".gitignore".to_string()));
    assert!(!files
        .iter()
        .any(|f| f == "model.ckpt" || f.starts_with("outputs/")));
    assert_eq!(git(&wt, &["rev-parse", &format!("{commit}^")]), before.head);
    assert_eq!(
        git(&wt, &["rev-parse", &format!("{commit}^{{tree}}")]),
        run.run.tree_hash
    );
    assert_eq!(git(&wt, &["rev-parse", "refs/arboria/runs/r001"]), *commit);
    assert_eq!(
        git(&wt, &["log", "-1", "--format=%s", commit]),
        "arboria snapshot r001"
    );
    let ev = events(&r.ledger);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].kind, "run_new");
    assert_eq!(ev[0].subject.as_deref(), Some("r001"));
    assert_eq!(
        ev[0].payload_json.as_deref().unwrap(),
        format!(
            "{{\"commit\": \"{commit}\", \"idea\": \"x\", \"reused\": false, \"tree\": \"{}\"}}",
            run.run.tree_hash
        )
    );
}

#[test]
fn deleted_file_absent_and_identical_state_reused() {
    let mut r = repo();
    std::fs::remove_file(r.wt.join("train.py")).unwrap();
    let r1 = new_run(&mut r, "remove train");
    assert!(!tree_files(&r.wt, &r1.run.snapshot_commit).contains(&"train.py".to_string()));
    assert!(git(&r.wt, &["ls-files"]).contains("train.py"));

    let r2 = new_run(&mut r, "same");
    assert!(r2.reused_snapshot);
    assert_eq!(r2.run.snapshot_commit, r1.run.snapshot_commit);
    std::fs::write(r.wt.join("new.py"), "a\n").unwrap();
    let r3 = new_run(&mut r, "changed");
    assert_ne!(r3.run.snapshot_commit, r1.run.snapshot_commit);
    std::fs::remove_file(r.wt.join("new.py")).unwrap();
    let r4 = new_run(&mut r, "back");
    assert!(r4.reused_snapshot);
    assert_eq!(r4.run.id.as_str(), "r004");
    assert_eq!(
        git(&r.wt, &["rev-parse", "refs/arboria/runs/r004"]),
        r1.run.snapshot_commit
    );
}

#[test]
fn clean_snapshot_is_head_tree_and_unborn_head_works() {
    let r = repo();
    let res = snapshot::snapshot(
        &r.wt,
        r.ledger.conn(),
        "m",
        false,
        None,
        None,
        &mut no_warn(),
    )
    .unwrap();
    assert_eq!(res.tree, git(&r.wt, &["rev-parse", "HEAD^{tree}"]));
    assert!(!res.reused);

    let tmp = TempDir::new("unborn");
    let wt = tmp.path().canonicalize().unwrap().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    git(&wt, &["init", "-q"]);
    std::fs::write(wt.join("a.py"), "a\n").unwrap();
    let ledger = Ledger::open(&tmp.path().join("state")).unwrap();
    let res =
        snapshot::snapshot(&wt, ledger.conn(), "m", false, None, None, &mut no_warn()).unwrap();
    assert_eq!(tree_files(&wt, &res.commit), ["a.py"]);
    assert_eq!(
        git(&wt, &["rev-list", "--count", &res.commit]),
        "1",
        "no parent"
    );
}

#[test]
fn large_untracked_files_refused_unless_allowed() {
    let r = repo();
    std::fs::write(r.wt.join("big.bin"), vec![b'x'; 3000]).unwrap();
    std::fs::write(r.wt.join("small.bin"), vec![b'x'; 10]).unwrap();
    let conn = r.ledger.conn();
    let mut quiet = |_: &str| {};
    let err = snapshot::snapshot(
        &r.wt,
        conn,
        "m",
        false,
        Some(1000),
        Some(10_000),
        &mut quiet,
    )
    .unwrap_err();
    assert!(matches!(err, Error::LargeUntracked(_)));
    assert_eq!(
        err.to_string(),
        "refusing to snapshot: large untracked file big.bin (0.0 MB > 0.0 MB). Add them to .gitignore, or pass --allow-large."
    );
    let err = snapshot::snapshot(
        &r.wt,
        conn,
        "m",
        false,
        Some(10_000),
        Some(2000),
        &mut quiet,
    )
    .unwrap_err();
    assert!(err
        .to_string()
        .contains("untracked files total 0.0 MB (> 0.0 MB)"));
    let mut warned = Vec::new();
    let res = snapshot::snapshot(
        &r.wt,
        conn,
        "m",
        true,
        Some(1000),
        Some(10_000),
        &mut |w: &str| warned.push(w.to_string()),
    )
    .unwrap();
    assert_eq!(warned.len(), 1);
    assert!(warned[0].contains("big.bin"));
    assert!(tree_files(&r.wt, &res.commit).contains(&"big.bin".to_string()));
    // ignored large files never count
    std::fs::remove_file(r.wt.join("big.bin")).unwrap();
    std::fs::write(r.wt.join("huge.ckpt"), vec![b'x'; 5000]).unwrap();
    snapshot::snapshot(
        &r.wt,
        conn,
        "m",
        false,
        Some(1000),
        Some(2000),
        &mut no_warn(),
    )
    .unwrap();
}

#[test]
fn size_warnings_format_like_python() {
    let tmp = TempDir::new("sizes");
    let mb = snapshot::MB as usize;
    std::fs::write(tmp.path().join("a"), vec![0u8; mb * 5 / 4]).unwrap();
    let w = snapshot::size_warnings(
        tmp.path(),
        &["a".into(), "missing".into()],
        snapshot::MB,
        snapshot::MB,
    );
    // 1.25 is exact in binary: Python f"{1.25:.1f}" rounds the tie to even, "1.2"
    assert_eq!(
        w,
        [
            "large untracked file a (1.2 MB > 1.0 MB)",
            "untracked files total 1.2 MB (> 1.0 MB)"
        ]
    );
}

#[test]
fn summary_notes_confidence_metric() {
    let mut r = repo();
    let run = new_run(&mut r, "first").run;
    let msg = records::get_run(&r.ledger, &RunId::parse("r009").unwrap())
        .unwrap_err()
        .to_string();
    assert_eq!(msg, "no run 'r009'");

    let done = records::set_run_summary(&mut r.ledger, &r.idea, &run.id, " loss 0.3 \n").unwrap();
    assert_eq!(done.summary.as_deref(), Some("loss 0.3"));
    let err = records::set_run_summary(&mut r.ledger, &r.idea, &run.id, &"l\n".repeat(11))
        .unwrap_err()
        .to_string();
    assert_eq!(err, "summary has 11 lines (max 10); shorten it");

    let mut other = r.idea.clone();
    other.id = arboria_core::model::IdeaId(99);
    let err = records::set_run_summary(&mut r.ledger, &other, &run.id, "x")
        .unwrap_err()
        .to_string();
    assert_eq!(err, "run r001 belongs to another idea");
    let err = records::add_note(&mut r.ledger, &other, "n", Some(&run.id), None)
        .unwrap_err()
        .to_string();
    assert_eq!(err, "run r001 belongs to another idea");

    let note = records::add_note(
        &mut r.ledger,
        &r.idea,
        " looks good ",
        Some(&run.id),
        Some(Confidence::Prelim),
    )
    .unwrap();
    assert_eq!(note.text, "looks good");
    assert_eq!(note.author.as_deref(), Some("human"));
    assert_eq!(note.confidence, Some(Confidence::Prelim));
    let idea: Idea = db::one(r.ledger.conn(), "SELECT * FROM ideas WHERE id = 1", [])
        .unwrap()
        .unwrap();
    assert_eq!(idea.confidence, Confidence::Prelim);

    // unchanged confidence / metric: no event
    let n = events(&r.ledger).len();
    let idea = records::set_confidence(&mut r.ledger, &idea, Confidence::Prelim).unwrap();
    let idea = records::set_metric(&mut r.ledger, &idea, " m ").unwrap();
    assert_eq!(events(&r.ledger).len(), n);
    let idea = records::set_confidence(&mut r.ledger, &idea, Confidence::Confirmed).unwrap();
    let idea = records::set_metric(&mut r.ledger, &idea, "val loss").unwrap();
    assert_eq!(idea.confidence, Confidence::Confirmed);
    assert_eq!(idea.metric.as_deref(), Some("val loss"));
    let ev = events(&r.ledger);
    let tail: Vec<(String, String)> = ev[ev.len() - 4..]
        .iter()
        .map(|e| (e.kind.clone(), e.payload_json.clone().unwrap()))
        .collect();
    assert_eq!(
        tail,
        [
            ("run_summary".into(), "{\"chars\": 8}".into()),
            (
                "note".into(),
                format!(
                    "{{\"author\": \"human\", \"confidence\": \"prelim\", \"note_id\": {}, \"run\": \"r001\"}}",
                    note.id.0
                )
            ),
            (
                "idea_confidence".into(),
                "{\"author\": \"human\", \"from\": \"prelim\", \"to\": \"confirmed\"}".into()
            ),
            (
                "idea_metric".into(),
                "{\"author\": \"human\", \"from\": \"m\"}".into()
            ),
        ]
    );
}

#[test]
fn decisions_open_and_decide_once() {
    let mut r = repo();
    let d = records::open_decision(
        &mut r.ledger,
        SubjectType::Run,
        "r001",
        "approve r001?",
        &["approve", "reject"],
    )
    .unwrap();
    assert_eq!(d.id.as_str(), "d001");
    assert_eq!(d.options, ["approve", "reject"]);
    assert_eq!(d.choice, None);
    let options_json: String = r
        .ledger
        .conn()
        .query_row("SELECT options_json FROM decisions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(options_json, "[\"approve\", \"reject\"]");

    let err = records::decide(&mut r.ledger, &d.id, "maybe", "human")
        .unwrap_err()
        .to_string();
    assert_eq!(err, "invalid choice 'maybe'; options: approve, reject");
    let err = records::decide(&mut r.ledger, &DecisionId::parse("d009").unwrap(), "x", "h")
        .unwrap_err()
        .to_string();
    assert_eq!(err, "no decision 'd009'");

    let payload = serde_json::json!({"chain": ["zsh", "arb"], "tty": true});
    r.ledger
        .write(|tx| records::set_decision(tx, &d.id, "approve", "human", Some(&payload)))
        .unwrap();
    let err = records::decide(&mut r.ledger, &d.id, "reject", "human")
        .unwrap_err()
        .to_string();
    assert_eq!(err, "decision d001 was already decided: approve");
    let (payload_json, decided_by): (String, String) = r
        .ledger
        .conn()
        .query_row(
            "SELECT payload_json, decided_by FROM decisions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        payload_json,
        "{\"chain\": [\"zsh\", \"arb\"], \"tty\": true}"
    );
    assert_eq!(decided_by, "human");
    let ev = events(&r.ledger);
    assert_eq!(
        ev.iter()
            .map(|e| e.payload_json.clone().unwrap())
            .collect::<Vec<_>>(),
        [
            "{\"action\": \"open\", \"subject\": \"run:r001\"}",
            "{\"action\": \"decide\", \"by\": \"human\", \"choice\": \"approve\", \"payload\": {\"chain\": [\"zsh\", \"arb\"], \"tty\": true}}",
        ]
    );
    // an empty payload is stored but left out of the event (Python truthiness)
    let d2 = records::open_decision(&mut r.ledger, SubjectType::Lesson, "l001", "q", &[]).unwrap();
    r.ledger
        .write(|tx| {
            records::set_decision(
                tx,
                &d2.id,
                "anything",
                "human",
                Some(&serde_json::json!({})),
            )
        })
        .unwrap();
    let last = events(&r.ledger).pop().unwrap();
    assert_eq!(
        last.payload_json.unwrap(),
        "{\"action\": \"decide\", \"by\": \"human\", \"choice\": \"anything\"}"
    );
}

#[test]
fn missing_worktree_refused() {
    let mut r = repo();
    let mut gone = r.idea.clone();
    gone.worktree_path = Some(r.wt.join("nope").display().to_string());
    let err = records::new_run(
        &mut r.ledger,
        &gone,
        "x",
        SnapshotOptions::default(),
        &mut no_warn(),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        err,
        format!("idea worktree {}/nope is missing", r.wt.display())
    );
    assert!(git_bytes(&r.wt, &["for-each-ref", "refs/arboria"]).is_empty());
}

#[test]
fn backup_commits_after_backup_events_only_and_dump_is_pythons() {
    let mut r = repo();
    let state = r.ledger.state_dir().to_path_buf();
    let log = |state: &Path| git(state, &["log", "--format=%s"]);
    new_run(&mut r, "first");
    assert_eq!(log(&state), "arboria backup: run_new r001");
    // a write without a backup event: no commit
    r.ledger
        .write(|tx| {
            tx.execute("UPDATE runs SET updated_at = 'x'", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(log(&state).lines().count(), 1);
    // an explicit backup picks the change up; a second one has nothing to commit
    assert!(r.ledger.backup("heartbeat").unwrap());
    assert!(!r.ledger.backup("again").unwrap());
    let d = records::open_decision(&mut r.ledger, SubjectType::Run, "r001", "q", &["a"]).unwrap();
    records::decide(&mut r.ledger, &d.id, "a", "human").unwrap();
    assert_eq!(
        log(&state).lines().collect::<Vec<_>>(),
        [
            "arboria backup: decision d001",
            "arboria backup: decision d001",
            "arboria backup: heartbeat",
            "arboria backup: run_new r001"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(state.join(".gitignore")).unwrap(),
        format!(
            "{}{}{}{}",
            db::BACKUP_GITIGNORE,
            db::NOTEBOOKS_GITIGNORE,
            db::NOTEBOOKS_MD_GITIGNORE,
            db::COMMANDS_GITIGNORE
        )
    );
    assert_eq!(git(&state, &["ls-files"]), ".gitignore\nbackup/ledger.sql");

    // the dump is byte-identical to CPython 3.13's iterdump of the same file
    let sql = std::fs::read_to_string(state.join("backup/ledger.sql")).unwrap();
    let py = Command::new("python3")
        .args([
            "-c",
            "import sqlite3,sys\n\
             if sys.version_info < (3, 13): print('SKIP'); sys.exit()\n\
             c = sqlite3.connect(sys.argv[1])\n\
             v = c.execute('PRAGMA user_version').fetchone()[0]\n\
             sys.stdout.write('\\n'.join([f'PRAGMA user_version = {v};', *c.iterdump()]) + '\\n')",
        ])
        .arg(r.ledger.path())
        .output();
    match py {
        Ok(out) if out.status.success() && out.stdout != b"SKIP\n" => {
            assert_eq!(String::from_utf8(out.stdout).unwrap(), sql);
        }
        other => eprintln!("python3 >= 3.13 not available, dump not compared: {other:?}"),
    }
}

#[test]
fn backup_refused_without_its_own_repo() {
    // without .arboria/.git git would fall back to the project repo and commit the
    // user's staged changes
    let tmp = TempDir::new("nobackup");
    let wt = tmp.path().canonicalize().unwrap();
    git(&wt, &["init", "-q"]);
    std::fs::write(wt.join("a.py"), "a\n").unwrap();
    git(&wt, &["add", "a.py"]);
    let ledger = Ledger::open(&wt.join(".arboria")).unwrap();
    let err = ledger.backup("x").unwrap_err().to_string();
    assert!(err.ends_with("has no backup repository (.git); run `arb init` again to create it"));
    assert_eq!(git(&wt, &["diff", "--cached", "--name-only"]), "a.py");
    assert!(git_bytes(&wt, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_empty());
}
