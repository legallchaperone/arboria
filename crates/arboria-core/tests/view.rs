//! The app's read-only views (`view`) and the project list (`registry`) against a real
//! repo and ledger. One test binary, one test: it sets `ARBORIA_CONFIG_DIR`,
//! `ARBORIA_HOME` and `ARBORIA_PYTHON` (no notebooks: the bridge is unavailable).
//!
//! Also checks that the app's fixture (`app/src/fixtures/board.json`, hand-written for
//! the screenshots) has exactly the shape the core emits, so the TypeScript types and the
//! Rust structs cannot drift apart. `VIEW_DUMP=<file>` writes this test's board there.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use arboria_core::db::{self, Ledger};
use arboria_core::ideas::{self, NewIdea};
use arboria_core::model::{IdeaName, RunId, TaskKind};
use arboria_core::paths::{self, RepoContext};
use arboria_core::submission::{self, SubmitArgs};
use arboria_core::view::{self, InboxKind, Place};
use arboria_core::{registry, tasks};
use common::TempDir;
use serde_json::Value;

fn git(cwd: &Path, args: &[&str]) {
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
}

fn n(s: &str) -> IdeaName {
    IdeaName::parse(s).unwrap()
}

const PROJECT: &str = "lm-3f2a";

struct World {
    _tmp: TempDir,
    root: PathBuf,
    ctx: RepoContext,
    ledger: Ledger,
}

fn world() -> World {
    let tmp = TempDir::new("view");
    let root = tmp.path().canonicalize().unwrap();
    let main = root.join("lm");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("train.py"), "print('train')\n").unwrap();
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "init"]);
    std::fs::write(
        main.join("arboria.toml"),
        format!(
            "default_machine = \"fake\"\n[project]\nid = \"{PROJECT}\"\n[budget]\ntotal_su = 400\n"
        ),
    )
    .unwrap();
    let cfg = root.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let r = root.display();
    std::fs::write(
        cfg.join("clusters.toml"),
        format!(
            "[fake]\nkind = \"fake\"\nroot = \"{r}/fake/state\"\naccount = \"acct1\"\n\
             gpus_per_node = 4\ncpus_per_gpu = 16\nmem_per_gpu_gb = 100\n\
             default_partition = \"ghx4\"\nmax_segments = 2\nenv_activate = []\n\
             [fake.partitions.ghx4]\nmax_walltime = \"48h\"\n\
             [fake.paths]\nruns = \"{r}/fake/remote/$USER/runs\"\ndata = \"{r}/fake/remote/data\"\n\
             cache = \"{r}/fake/remote/cache\"\nckpt = \"{r}/fake/remote/ckpt\"\nout = \"{r}/fake/remote/out\"\n"
        ),
    )
    .unwrap();
    std::env::set_var("ARBORIA_CONFIG_DIR", &cfg);
    std::env::set_var("ARBORIA_HOME", root.join("home"));
    std::env::set_var("ARBORIA_PYTHON", root.join("no-python"));
    let ctx = paths::locate(Some(&main)).unwrap();
    paths::ensure_state_excluded(&ctx.common_dir).unwrap();
    let ledger = Ledger::open(&ctx.state_dir()).unwrap();
    db::init_backup_repo(&ctx.state_dir()).unwrap();
    World {
        _tmp: tmp,
        root,
        ctx,
        ledger,
    }
}

fn new_idea(w: &mut World, name: &str, parent: Option<&str>, from: Option<&str>) {
    let parent = parent.map(n);
    let run =
        from.map(|r| arboria_core::records::get_run(&w.ledger, &RunId::parse(r).unwrap()).unwrap());
    ideas::new_idea(
        &w.ctx,
        &mut w.ledger,
        &n(name),
        NewIdea {
            hypothesis: "warmup 拉长到 8k，前 3k 步的尖峰会消失",
            metric: "val/loss",
            parent: parent.as_ref(),
            from_run: run.as_ref(),
            seed: None,
        },
        &mut |_: &str| {},
    )
    .unwrap();
}

fn submit(w: &mut World, idea: &str, reason: &str, lr: &str) -> String {
    let idea = ideas::get_idea(&w.ledger, &n(idea)).unwrap();
    let cmd: Vec<String> = ["python", "train.py", "--lr", lr]
        .map(String::from)
        .to_vec();
    submission::submit(
        &w.ctx,
        &mut w.ledger,
        &idea,
        SubmitArgs {
            reason,
            command: Some(&cmd),
            gpus: Some(4),
            time: Some("12h"),
            ..Default::default()
        },
        &mut |_: &str| {},
    )
    .unwrap()
    .run
    .id
    .to_string()
}

/// Mark a run as finished with one billed job (what `poll` records).
fn finish(w: &mut World, run: &str, status: &str, su: f64, at: &str) {
    w.ledger
        .write(|tx| {
            tx.execute(
                "UPDATE runs SET status = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![run, status, at],
            )?;
            tx.execute(
                "INSERT INTO jobs (run_id, segment, job_name, job_id, state, elapsed_s, billed_su, \
                 submitted_at, updated_at) VALUES (?1, 1, ?4, '1001', ?2, 3600, ?3, ?5, ?5)",
                rusqlite::params![
                    run,
                    if status == "succeeded" { "completed" } else { "failed" },
                    su,
                    format!("arb-{PROJECT}-{run}-s1"),
                    at
                ],
            )?;
            tx.execute(
                "INSERT INTO events (ts, kind, subject, payload_json) VALUES (?1, 'run_final', ?2, '{}')",
                rusqlite::params![at, run],
            )?;
            Ok(())
        })
        .unwrap();
}

fn note(w: &mut World, idea: &str, at: &str) {
    let id = ideas::get_idea(&w.ledger, &n(idea)).unwrap().id.0;
    w.ledger
        .write(|tx| {
            tx.execute(
                "INSERT INTO notes (idea_id, text, created_at, author) VALUES (?1, '看过了', ?2, 'human')",
                rusqlite::params![id, at],
            )?;
            Ok(())
        })
        .unwrap();
}

/// `fixture` has `rust`'s shape: the same keys at every level, the same JSON types where
/// both are set (null fits anything), every array element shaped like `rust`'s first.
fn same_shape(rust: &Value, fixture: &Value, path: &str) -> Result<(), String> {
    match (rust, fixture) {
        (Value::Null, _) | (_, Value::Null) => Ok(()),
        (Value::Object(a), Value::Object(b)) => {
            if path.ends_with(".result") {
                return Ok(()); // a probe result: one of three kinds, checked by `probe`
            }
            let ka: Vec<&String> = a.keys().collect();
            let kb: Vec<&String> = b.keys().collect();
            if ka != kb {
                return Err(format!("{path}: keys {ka:?} (core) vs {kb:?} (fixture)"));
            }
            for (k, v) in a {
                same_shape(v, &b[k], &format!("{path}.{k}"))?;
            }
            Ok(())
        }
        (Value::Array(a), Value::Array(b)) => match a.first() {
            None => Ok(()),
            Some(first) => b
                .iter()
                .enumerate()
                .try_for_each(|(i, v)| same_shape(first, v, &format!("{path}[{i}]"))),
        },
        (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_))
        | (Value::Bool(_), Value::Bool(_)) => Ok(()),
        _ => Err(format!("{path}: {rust} (core) vs {fixture} (fixture)")),
    }
}

#[test]
fn board_inbox_cards_registry_and_the_app_fixture() {
    let mut w = world();
    new_idea(&mut w, "warmup-long", None, None);
    let r1 = submit(&mut w, "warmup-long", "warmup 2k 基线", "3e-4");
    finish(&mut w, &r1, "succeeded", 11.86, "2026-11-04T08:00:00Z");
    new_idea(&mut w, "label-smooth", Some("warmup-long"), Some(&r1));
    let r2 = submit(&mut w, "label-smooth", "label smoothing 0.1", "2e-4");
    finish(&mut w, &r2, "failed", 0.62, "2026-11-04T09:00:00Z");
    let r3 = submit(&mut w, "warmup-long", "warmup 8k", "1e-4");
    let r4 = submit(&mut w, "label-smooth", "label smoothing 0.05", "1e-4");
    finish(&mut w, &r4, "succeeded", 5.0, "2026-11-04T10:00:00Z");
    note(&mut w, "label-smooth", "2026-11-04T11:00:00Z"); // after r2 and r4: seen
    let idea = ideas::get_idea(&w.ledger, &n("warmup-long")).unwrap();
    let t = tasks::add(
        &mut w.ledger,
        &idea,
        "画 loss 曲线",
        TaskKind::CommandNew,
        None,
        None,
    )
    .unwrap();
    tasks::done(&mut w.ledger, &t.task.id, "写好了 plot_loss").unwrap();
    arboria_core::lessons::propose(&w.ctx, &mut w.ledger, "warmup 至少 4k", None, None).unwrap();
    std::fs::write(
        w.ctx.state_dir().join("notifications.log"),
        format!("2026-11-04T08:00:01Z {r1} 已成功（11.86 SU）\nnot a stamp line\n"),
    )
    .unwrap();

    let before = std::fs::read(w.ctx.db_path()).unwrap();
    let b = view::board(&w.ctx.main).unwrap();
    assert_eq!(std::fs::read(w.ctx.db_path()).unwrap(), before, "read-only");

    assert_eq!(b.project.kind, Place::Project);
    assert_eq!(b.project.name, "lm");
    assert_eq!(b.project.project_id.as_deref(), Some(PROJECT));
    assert_eq!(b.project.total_su, 400.0);
    let names: Vec<&str> = b.ideas.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["warmup-long", "label-smooth"]);
    let ls = &b.ideas[1];
    assert_eq!(ls.parent.as_deref(), Some("warmup-long"));
    assert_eq!(ls.from_run.as_deref(), Some(r1.as_str()));
    assert_eq!((ls.runs, ls.su_used), (2, 5.62));
    assert!(b.ideas[0].notebook.is_none(), "no Python here: no notebook");
    assert_eq!(b.spend.used, 17.48);
    assert_eq!(b.spend.by_idea[0].idea, "warmup-long");
    assert_eq!(
        b.runs[0].finished_at.as_deref(),
        Some("2026-11-04T08:00:00Z")
    );
    assert_eq!(b.runs[2].finished_at, None);
    assert_eq!(b.runs[0].jobs[0].state, "completed");

    let kinds: Vec<(InboxKind, Option<&str>)> = b
        .inbox
        .iter()
        .map(|i| {
            (
                i.kind,
                i.run
                    .as_deref()
                    .or(i.task.as_deref())
                    .or(i.lesson.as_deref()),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (InboxKind::Approve, Some(r3.as_str())),
            (InboxKind::Finished, Some(r1.as_str())),
            (InboxKind::AgentDone, Some("t001")),
            (InboxKind::Lesson, Some("l001")),
        ],
        "r002 and r004 ended before the human's note on label-smooth"
    );
    let approve = &b.inbox[0];
    assert_eq!(approve.title, format!("{r3} 待签字"));
    assert_eq!(
        approve.command.as_deref(),
        Some(format!("arb approve {r3}").as_str())
    );
    assert!(
        approve
            .meta
            .starts_with("warmup-long · 4 GPU × 12h · fake · ≈ "),
        "{}",
        approve.meta
    );
    assert!(
        approve.meta.ends_with("预算剩 382.52 SU"),
        "{}",
        approve.meta
    );
    assert_eq!(
        b.inbox[1].meta,
        "warmup-long · warmup 2k 基线 · 花了 11.86 SU"
    );

    // the card is `arb card`'s, to the byte
    assert_eq!(b.cards.len(), 1);
    let card = &b.cards[0];
    let (expected, same) =
        submission::card_for(&w.ctx, &w.ledger, &RunId::parse(&r3).unwrap()).unwrap();
    assert!(same);
    assert_eq!(card.text.as_deref(), Some(expected.text().as_str()));
    assert_eq!(card.matches, Some(true));
    assert_eq!(card.card_hash.as_deref(), Some(expected.card_hash.as_str()));

    assert_eq!(b.notifications.len(), 2);
    assert_eq!(b.notifications[0].at, None);
    assert_eq!(
        b.notifications[1].at.as_deref(),
        Some("2026-11-04T08:00:01Z")
    );
    assert_eq!(b.lessons[0].text, "warmup 至少 4k");

    // the project list
    assert!(registry::list().unwrap().is_empty());
    let (e, new) = registry::add(&w.ctx.main.join("train.py").parent().unwrap().join(".")).unwrap();
    assert!(new && e.path == w.ctx.main);
    assert!(!registry::add(&w.ctx.main).unwrap().1);
    let missing = w.root.join("gone");
    std::fs::create_dir_all(missing.join(".arboria")).unwrap();
    let all = view::projects().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].inbox.len(), 4);
    assert_eq!(all[0].open_ideas, 2);
    let bad = view::summary(&missing);
    assert!(bad.error.is_some() && bad.inbox.is_empty());
    assert!(registry::add(&w.root).is_err(), "not a repo");
    assert!(registry::remove(&w.ctx.main).unwrap());
    assert!(!registry::remove(&w.ctx.main).unwrap());
    assert!(registry::list().unwrap().is_empty());

    // the app's fixture has this shape
    let rust = serde_json::to_value(&b).unwrap();
    if let Some(out) = std::env::var_os("VIEW_DUMP") {
        std::fs::write(out, serde_json::to_string_pretty(&rust).unwrap()).unwrap();
    }
    let fixture_path = common::repo_root().join("app/src/fixtures/board.json");
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture_path).unwrap()).unwrap();
    for (i, board) in fixture["boards"].as_array().unwrap().iter().enumerate() {
        same_shape(&rust, board, &format!("boards[{i}]")).unwrap();
    }
    let summary = serde_json::to_value(&all[0]).unwrap();
    for (i, p) in fixture["projects"].as_array().unwrap().iter().enumerate() {
        same_shape(&summary, p, &format!("projects[{i}]")).unwrap();
    }
}

#[test]
fn view_reads_only() {
    // the module's own guard (the same rule as Python tests/test_ui.py for the dashboard)
    let src =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/view.rs")).unwrap();
    let code: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for banned in [
        "Ledger",
        ".write(",
        "std::process",
        "Command",
        "backend",
        "submission",
        "monitor::poll",
        "TcpStream",
        "registry::add",
        "registry::remove",
        "new_idea",
        "ensure_notebook",
        "set_probe_result",
        "notebooks::",
        "execute(",
    ] {
        assert!(!code.contains(banned), "view.rs uses {banned}");
    }
    assert!(code.contains("db::open_read_only"));
}
