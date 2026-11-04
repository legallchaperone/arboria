//! ideas + seeds against real git repos and ledgers (mirrors tests/test_ideas.py,
//! tests/test_seeds.py and tests/test_idea_seeds.py where the behaviour is the core's).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use arboria_core::db::{self, Ledger};
use arboria_core::ideas::{self, CloseMode, NewIdea, PlantOptions, Route, SeedFields, SeedState};
use arboria_core::model::{Confidence, Event, IdeaName, IdeaStage};
use arboria_core::paths::{self, RepoContext};
use arboria_core::records::{self, SnapshotOptions};
use arboria_core::Error;
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

fn n(s: &str) -> IdeaName {
    IdeaName::parse(s).unwrap()
}

fn quiet() -> impl FnMut(&str) {
    |_: &str| {}
}

struct Project {
    _tmp: TempDir,
    root: PathBuf,
    ctx: RepoContext,
    ledger: Ledger,
}

/// An initialized repo `<tmp>/WaveFront` with one commit.
fn project() -> Project {
    let tmp = TempDir::new("ideas");
    let root = tmp.path().canonicalize().unwrap();
    let main = root.join("WaveFront");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("train.py"), "print('train')\n").unwrap();
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "init"]);
    let ctx = paths::locate(Some(&main)).unwrap();
    paths::ensure_state_excluded(&ctx.common_dir).unwrap();
    let ledger = Ledger::open(&ctx.state_dir()).unwrap();
    db::init_backup_repo(&ctx.state_dir()).unwrap();
    assert!(ctx.initialized());
    Project {
        _tmp: tmp,
        root,
        ctx,
        ledger,
    }
}

fn new(
    p: &mut Project,
    name: &str,
    parent: Option<&IdeaName>,
) -> arboria_core::Result<arboria_core::model::Idea> {
    ideas::new_idea(
        &p.ctx,
        &mut p.ledger,
        &n(name),
        NewIdea {
            hypothesis: "warmup helps",
            metric: "val loss",
            parent,
            ..Default::default()
        },
        &mut quiet(),
    )
}

fn last_event(ledger: &Ledger) -> Event {
    db::one(ledger.conn(), "SELECT * FROM events ORDER BY id DESC", [])
        .unwrap()
        .unwrap()
}

fn msg<T: std::fmt::Debug>(r: arboria_core::Result<T>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn new_idea_branch_worktree_and_row() {
    let mut p = project();
    let idea = new(&mut p, "wide-lr", None).unwrap();
    let wt = p.root.join("WaveFront-wide-lr");
    assert_eq!(idea.branch.as_deref(), Some("idea/wide-lr"));
    assert_eq!(idea.worktree_path.as_deref(), Some(wt.to_str().unwrap()));
    assert_eq!(idea.stage, IdeaStage::Open);
    assert_eq!(idea.hypothesis.as_deref(), Some("warmup helps"));
    assert_eq!(
        git(&wt, &["symbolic-ref", "--short", "HEAD"]),
        "idea/wide-lr"
    );
    assert_eq!(
        git(&wt, &["rev-parse", "HEAD"]),
        git(&p.ctx.main, &["rev-parse", "HEAD"])
    );
    let ev = last_event(&p.ledger);
    let base = git(&p.ctx.main, &["rev-parse", "HEAD"]);
    assert_eq!(
        (ev.kind.as_str(), ev.payload_json.unwrap()),
        (
            "idea_new",
            format!("{{\"base\": \"{base}\", \"branch\": \"idea/wide-lr\", \"from_run\": null, \"notebook\": null, \"parent\": null}}")
        )
    );
    assert_eq!(
        git(&p.ctx.state_dir(), &["log", "-1", "--format=%s"]),
        "arboria backup: idea_new wide-lr"
    );

    assert_eq!(
        msg(new(&mut p, "wide-lr", None)),
        "an idea named 'wide-lr' already exists (names are never reused)"
    );
    git(&p.ctx.main, &["branch", "idea/taken"]);
    assert_eq!(
        msg(new(&mut p, "taken", None)),
        "branch idea/taken already exists; pick another name"
    );
    std::fs::create_dir_all(p.root.join("WaveFront-there")).unwrap();
    assert_eq!(
        msg(new(&mut p, "there", None)),
        format!(
            "{} already exists; pick another name or move it away",
            p.root.join("WaveFront-there").display()
        )
    );
    assert_eq!(
        msg(new(&mut p, "nope", Some(&n("missing")))),
        "no idea named 'missing' (see `arb idea list`)"
    );
    assert_eq!(
        msg(ideas::new_idea(
            &p.ctx,
            &mut p.ledger,
            &n("empty"),
            NewIdea {
                hypothesis: " ",
                metric: "m",
                ..Default::default()
            },
            &mut quiet()
        )),
        "hypothesis must not be empty"
    );

    // a child branches off the parent's tip; current_idea is found from its worktree
    std::fs::write(wt.join("lr.py"), "lr = 3e-4\n").unwrap();
    git(&wt, &["add", "-A"]);
    git(&wt, &["commit", "-q", "-m", "lr"]);
    let child = new(&mut p, "wide-lr-2", Some(&n("wide-lr"))).unwrap();
    assert_eq!(child.parent_id, Some(idea.id));
    let cwt = p.root.join("WaveFront-wide-lr-2");
    assert!(cwt.join("lr.py").exists());
    let in_child = paths::locate(Some(&cwt)).unwrap();
    assert_eq!(
        ideas::current_idea(&in_child, &p.ledger)
            .unwrap()
            .unwrap()
            .name,
        n("wide-lr-2")
    );
    assert!(ideas::current_idea(&p.ctx, &p.ledger).unwrap().is_none());
    assert_eq!(
        msg(ideas::require_current_idea(&p.ctx, &p.ledger)),
        "the current directory is not an idea worktree; cd into one (see `arb idea list`) or create one with `arb idea new`"
    );

    let list = ideas::list_ideas(&p.ledger).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].parent_name.as_deref(), Some("wide-lr"));
    assert_eq!(list[0].run_count, 0);
    let shown = ideas::show_idea(&in_child, &p.ledger, None).unwrap();
    assert_eq!(shown.parent_name.as_deref(), Some("wide-lr"));
    assert_eq!(
        shown.notebook,
        p.ctx
            .state_dir()
            .join("notebooks/wide-lr-2.py")
            .display()
            .to_string()
    );
}

#[test]
fn unborn_main_is_refused() {
    let tmp = TempDir::new("unborn-main");
    let main = tmp.path().canonicalize().unwrap().join("m");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    let ctx = paths::locate(Some(&main)).unwrap();
    let mut ledger = Ledger::open(&ctx.state_dir()).unwrap();
    let err = ideas::new_idea(
        &ctx,
        &mut ledger,
        &n("x"),
        NewIdea {
            hypothesis: "h",
            metric: "m",
            ..Default::default()
        },
        &mut quiet(),
    );
    assert_eq!(
        msg(err),
        format!(
            "the main worktree {} has no commits yet; commit something first",
            main.display()
        )
    );
}

#[test]
fn drop_archives_and_fork_from_its_run() {
    let mut p = project();
    let idea = new(&mut p, "dmd", None).unwrap();
    let wt = p.root.join("WaveFront-dmd");
    std::fs::write(wt.join("wip.py"), "uncommitted = True\n").unwrap();
    let run = records::new_run(
        &mut p.ledger,
        &idea,
        "try it",
        SnapshotOptions::default(),
        &mut quiet(),
    )
    .unwrap()
    .run;

    let closed = ideas::close_idea(
        &p.ctx,
        &mut p.ledger,
        Some(&n("dmd")),
        CloseMode::Drop,
        Some(Confidence::Rejected),
        false,
        &mut quiet(),
    )
    .unwrap();
    assert_eq!(closed.idea.stage, IdeaStage::Dropped);
    assert_eq!(closed.idea.confidence, Confidence::Rejected);
    assert!(closed.idea.closed_at.is_some());
    let final_commit = closed.close.final_commit.clone().unwrap();
    assert_eq!(closed.close.tag.as_deref(), Some("arboria/archive/dmd"));
    assert_eq!(
        git(&p.ctx.main, &["rev-parse", "arboria/archive/dmd"]),
        final_commit
    );
    assert_eq!(
        git(&p.ctx.main, &["rev-parse", "refs/arboria/ideas/dmd/final"]),
        final_commit
    );
    assert_eq!(
        git(&p.ctx.main, &["show", &format!("{final_commit}:wip.py")]),
        "uncommitted = True",
        "the final snapshot keeps uncommitted work"
    );
    assert!(!wt.exists());
    assert_eq!(git(&p.ctx.main, &["branch", "--list", "idea/dmd"]), "");
    let ev = last_event(&p.ledger);
    assert_eq!(ev.kind, "idea_close");
    assert_eq!(
        ev.payload_json.unwrap(),
        format!("{{\"confidence\": \"rejected\", \"final_commit\": \"{final_commit}\", \"final_ref\": \"refs/arboria/ideas/dmd/final\", \"mode\": \"drop\", \"name\": \"dmd\", \"tag\": \"arboria/archive/dmd\"}}")
    );
    assert_eq!(
        msg(ideas::close_idea(
            &p.ctx,
            &mut p.ledger,
            Some(&n("dmd")),
            CloseMode::Drop,
            None,
            false,
            &mut quiet()
        )),
        "idea 'dmd' is already dropped"
    );
    assert_eq!(
        msg(new(&mut p, "dmd-2", Some(&n("dmd")))),
        "parent idea 'dmd' is dropped; its branch is gone"
    );

    // lab.fork: a closed idea's run still seeds a new branch
    let fork = ideas::new_idea(
        &p.ctx,
        &mut p.ledger,
        &n("dmd-fork"),
        NewIdea {
            hypothesis: "h",
            metric: "m",
            from_run: Some(&run),
            ..Default::default()
        },
        &mut quiet(),
    )
    .unwrap();
    assert_eq!(fork.parent_id, Some(idea.id));
    let fwt = p.root.join("WaveFront-dmd-fork");
    assert_eq!(git(&fwt, &["rev-parse", "HEAD"]), run.snapshot_commit);
    assert_eq!(
        msg(ideas::new_idea(
            &p.ctx,
            &mut p.ledger,
            &n("dmd-fork-2"),
            NewIdea {
                hypothesis: "h",
                metric: "m",
                from_run: Some(&run),
                parent: Some(&n("other")),
                ..Default::default()
            },
            &mut quiet()
        )),
        "run r001 belongs to dmd, not other"
    );
}

#[test]
fn merge_refuses_dirty_and_conflicts() {
    let mut p = project();
    new(&mut p, "feat", None).unwrap();
    let wt = p.root.join("WaveFront-feat");
    std::fs::write(wt.join("train.py"), "print('feat')\n").unwrap();
    let dirty = ideas::close_idea(
        &p.ctx,
        &mut p.ledger,
        Some(&n("feat")),
        CloseMode::Merge,
        None,
        false,
        &mut quiet(),
    );
    assert_eq!(
        msg(dirty),
        format!(
            "idea worktree {} has uncommitted changes; commit them on idea/feat (or discard them) before merging",
            wt.display()
        )
    );
    git(&wt, &["commit", "-q", "-am", "feat"]);
    // a conflicting change on main
    std::fs::write(p.ctx.main.join("train.py"), "print('main')\n").unwrap();
    git(&p.ctx.main, &["commit", "-q", "-am", "main"]);
    let main_head = git(&p.ctx.main, &["rev-parse", "HEAD"]);
    let conflict = ideas::close_idea(
        &p.ctx,
        &mut p.ledger,
        Some(&n("feat")),
        CloseMode::Merge,
        None,
        false,
        &mut quiet(),
    );
    assert_eq!(
        msg(conflict),
        "merging idea/feat into main conflicts; merge aborted, nothing changed. Resolve by merging main into idea/feat in the idea worktree, then retry."
    );
    assert_eq!(git(&p.ctx.main, &["rev-parse", "HEAD"]), main_head);
    assert_eq!(git(&p.ctx.main, &["status", "--porcelain"]), "");

    // resolve on the idea branch, then the merge goes through
    git(&wt, &["merge", "-q", "-X", "ours", "main", "-m", "sync"]);
    let closed = ideas::close_idea(
        &p.ctx,
        &mut p.ledger,
        Some(&n("feat")),
        CloseMode::Merge,
        Some(Confidence::Confirmed),
        false,
        &mut quiet(),
    )
    .unwrap();
    assert_eq!(closed.idea.stage, IdeaStage::Merged);
    assert_eq!(closed.close.merged_into.as_deref(), Some("main"));
    assert_eq!(
        closed.close.merge_commit.as_deref(),
        Some(git(&p.ctx.main, &["rev-parse", "HEAD"]).as_str())
    );
    assert_eq!(
        git(&p.ctx.main, &["log", "-1", "--format=%s"]),
        "arboria: merge idea feat"
    );
    assert_eq!(
        std::fs::read_to_string(p.ctx.main.join("train.py")).unwrap(),
        "print('feat')\n"
    );
    assert!(!wt.exists());
}

fn seed_fields<'a>(gist: &'a str) -> SeedFields<'a> {
    SeedFields {
        gist: Some(gist),
        ..Default::default()
    }
}

#[test]
fn routing_seeds_and_planting() {
    // the only test in this binary that reads or sets ARBORIA_HOME / CLAUDECODE
    let p = project();
    let home_root = p.root.join("garden");
    std::env::set_var(paths::HOME_ENV, &home_root);
    std::env::remove_var("CLAUDECODE");
    std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");

    // routing: outside any repo → seeds; an uninitialized repo is an error
    let outside = p.root.join("plain");
    std::fs::create_dir_all(&outside).unwrap();
    assert!(matches!(ideas::route(&outside).unwrap(), Route::Seeds(h) if h.root == home_root));
    let bare = p.root.join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q"]);
    assert!(matches!(ideas::route(&bare), Err(Error::NotInitialized(_))));
    assert!(matches!(ideas::route(&p.ctx.main).unwrap(), Route::Repo(c) if c.main == p.ctx.main));

    let mut echoed = Vec::new();
    let home = ideas::ensure_home(&mut |m: &str| echoed.push(m.to_string())).unwrap();
    assert_eq!(
        echoed,
        [format!(
            "种子园建在 {}（ARBORIA_HOME 可以改位置）",
            home_root.display()
        )]
    );
    assert!(home.initialized());
    assert_eq!(
        git(&home.state_dir(), &["log", "--format=%s"]),
        "arboria backup: init home"
    );
    assert!(ideas::ensure_home(&mut |m: &str| panic!("{m}")).is_ok());
    assert!(matches!(
        ideas::route(&home.seeds_dir()).unwrap(),
        Route::Seeds(_)
    ));

    // the home's own repo-like folders still route to seeds, even inside a repo
    let mut hl = Ledger::open(&home.state_dir()).unwrap();
    std::fs::write(home.notebook("low-rank"), "# mine\n").unwrap();
    let s = ideas::new_seed(&home, &mut hl, &n("low-rank"), seed_fields(" rank-4 adam ")).unwrap();
    assert!(s.kept_existing_notebook);
    assert_eq!(s.seed_status, SeedState::Thinking);
    assert_eq!(s.idea.gist.as_deref(), Some("rank-4 adam"));
    assert_eq!(s.idea.stage, IdeaStage::Seed);
    assert_eq!(
        last_event(&hl).payload_json.unwrap(),
        "{\"kept_existing_notebook\": true, \"notebook\": \"seeds/low-rank.py\"}"
    );
    assert_eq!(
        msg(ideas::new_seed(
            &home,
            &mut hl,
            &n("low-rank"),
            seed_fields("again")
        )),
        "a seed named 'low-rank' already exists (see `arb idea show low-rank` outside any repo)"
    );
    ideas::new_seed(&home, &mut hl, &n("other"), SeedFields::default()).unwrap();
    assert_eq!(
        msg(ideas::show_seed(&home, &hl, &n("nope"))),
        "no seed named 'nope' (see `arb idea list` outside any repo)"
    );

    // set: unchanged fields write nothing; "" clears
    let before = db::all::<Event>(hl.conn(), "SELECT * FROM events", [])
        .unwrap()
        .len();
    ideas::set_seed_fields(&mut hl, &n("low-rank"), seed_fields("rank-4 adam")).unwrap();
    assert_eq!(
        db::all::<Event>(hl.conn(), "SELECT * FROM events", [])
            .unwrap()
            .len(),
        before
    );
    let set = ideas::set_seed_fields(
        &mut hl,
        &n("low-rank"),
        SeedFields {
            gist: Some(""),
            hypothesis: Some("rank 4 is enough"),
            metric: Some("val loss"),
        },
    )
    .unwrap();
    assert_eq!(set.gist, None);
    assert_eq!(
        last_event(&hl).payload_json.unwrap(),
        "{\"fields\": [\"gist\", \"hypothesis\", \"metric\"]}"
    );

    // drop / revive
    ideas::drop_seed(&mut hl, &n("other")).unwrap();
    assert_eq!(
        msg(ideas::drop_seed(&mut hl, &n("other"))),
        "seed 'other' is already dropped"
    );
    let names = |v: Vec<ideas::SeedView>| {
        v.into_iter()
            .map(|s| s.idea.name.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(ideas::list_seeds(&home, &hl, false).unwrap()),
        ["low-rank"]
    );
    assert_eq!(names(ideas::list_seeds(&home, &hl, true).unwrap()).len(), 2);
    assert_eq!(
        msg(ideas::revive_seed(&mut hl, &n("low-rank"))),
        "seed 'low-rank' is not dropped (it is thinking · 思考中); only a dropped seed can be revived"
    );
    let revived = ideas::revive_seed(&mut hl, &n("other")).unwrap();
    assert_eq!(revived.stage, IdeaStage::Seed);
    assert_eq!(revived.closed_at, None);

    // plant: needs a hypothesis and a metric
    assert_eq!(
        msg(ideas::plant(&home, &mut hl, &n("other"), &p.ctx, PlantOptions::default(), &mut quiet())),
        "种下需要假设和指标：种子 other 还没有假设和指标。用 --hypothesis / --metric 给出，或先在仓库外 arb idea set other --hypothesis …"
    );
    let idea = ideas::plant(
        &home,
        &mut hl,
        &n("low-rank"),
        &p.ctx,
        PlantOptions::default(),
        &mut quiet(),
    )
    .unwrap();
    assert_eq!(idea.name, n("low-rank"));
    assert_eq!(idea.hypothesis.as_deref(), Some("rank 4 is enough"));
    assert!(p.root.join("WaveFront-low-rank").is_dir());
    let ev: Event = db::one(
        p.ledger.conn(),
        "SELECT * FROM events WHERE kind = 'idea_new' ORDER BY id DESC",
        [],
    )
    .unwrap()
    .unwrap();
    assert!(ev.payload_json.unwrap().ends_with(&format!(
        "\"seed\": \"low-rank\", \"seed_home\": \"{}\"}}",
        home_root.display()
    )));
    let shown = ideas::show_seed(&home, &hl, &n("low-rank")).unwrap();
    assert_eq!(shown.seed_status, SeedState::Planted);
    assert_eq!(shown.planted_to.len(), 1);
    assert_eq!(shown.planted_to[0].repo, p.ctx.main.display().to_string());
    assert_eq!(
        msg(ideas::plant(&home, &mut hl, &n("low-rank"), &p.ctx, PlantOptions::default(), &mut quiet())),
        format!(
            "seed 'low-rank' is already planted ({} · low-rank); add --again to plant it into another repo too",
            p.ctx.main.display()
        )
    );
    let again = ideas::plant(
        &home,
        &mut hl,
        &n("low-rank"),
        &p.ctx,
        PlantOptions {
            idea_name: Some(&n("low-rank-2")),
            again: true,
            ..Default::default()
        },
        &mut quiet(),
    )
    .unwrap();
    assert_eq!(again.name, n("low-rank-2"));
    assert_eq!(
        ideas::show_seed(&home, &hl, &n("low-rank"))
            .unwrap()
            .planted_to
            .len(),
        2
    );

    // a planted seed that is dropped and revived is planted again (v8: plantings decide)
    ideas::drop_seed(&mut hl, &n("low-rank")).unwrap();
    assert_eq!(
        msg(ideas::plant(
            &home,
            &mut hl,
            &n("low-rank"),
            &p.ctx,
            PlantOptions::default(),
            &mut quiet()
        )),
        "seed 'low-rank' is dropped; revive it first (`arb idea revive low-rank` outside any repo)"
    );
    ideas::revive_seed(&mut hl, &n("low-rank")).unwrap();
    assert_eq!(
        ideas::show_seed(&home, &hl, &n("low-rank"))
            .unwrap()
            .seed_status,
        SeedState::Planted
    );

    // human only inside Claude Code
    assert!(ideas::require_human_for_seeds("arb idea set", "改种子").is_ok());
    std::env::set_var("CLAUDECODE", "1");
    assert_eq!(
        msg(ideas::require_human_for_seeds("arb idea set", "改种子")),
        "arb idea set 只由人在自己的终端里做（改种子）；Claude 不能运行它。种子是人的：Claude 可以读（仓库外的 arb idea list / show、seeds/<名字>.py），不写"
    );
    std::env::remove_var("CLAUDECODE");

    // notebooks are the Python side's
    assert!(msg(ideas::restyle_seeds(&home)).starts_with("arb idea style 由 Python 版 arb 做"));
}
