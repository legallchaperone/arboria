//! tasks (todos + the v8 agent queue) against a real ledger.

mod common;

use arboria_core::db::{self, Ledger};
use arboria_core::model::{Event, Idea, TaskId, TaskKind, TaskStatus};
use arboria_core::tasks::{self, Usage};
use common::TempDir;
use serde_json::json;

struct Fixture {
    _tmp: TempDir,
    ledger: Ledger,
    idea: Idea,
    seed: Idea,
    merged: Idea,
    dropped_seed: Idea,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new("tasks");
    let mut ledger = Ledger::open(&tmp.path().join("state")).unwrap();
    db::init_backup_repo(ledger.state_dir()).unwrap();
    ledger
        .write(|tx| {
            let ts = db::now();
            for (name, stage, branch) in [
                ("wide-lr", "open", Some("idea/wide-lr")),
                ("low-rank", "seed", None),
                ("old", "merged", Some("idea/old")),
                ("gone", "dropped", None),
            ] {
                let status = if stage == "seed" { "open" } else { stage };
                tx.execute(
                    "INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, stage, \
                     status, created_at, updated_at) VALUES (?1, 'h', 'm', ?2, ?3, ?4, ?5, ?6, ?6)",
                    rusqlite::params![name, branch, branch.map(|_| "/wt"), stage, status, ts],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let get = |name: &str| -> Idea {
        db::one(ledger.conn(), "SELECT * FROM ideas WHERE name = ?1", [name])
            .unwrap()
            .unwrap()
    };
    let (idea, seed, merged, dropped_seed) =
        (get("wide-lr"), get("low-rank"), get("old"), get("gone"));
    Fixture {
        _tmp: tmp,
        ledger,
        idea,
        seed,
        merged,
        dropped_seed,
    }
}

fn last_event(ledger: &Ledger) -> (String, String, String) {
    let e: Event = db::one(ledger.conn(), "SELECT * FROM events ORDER BY id DESC", [])
        .unwrap()
        .unwrap();
    (e.kind, e.subject.unwrap(), e.payload_json.unwrap())
}

fn t(s: &str) -> TaskId {
    TaskId::parse(s).unwrap()
}

#[test]
fn add_list_and_close() {
    let mut f = fixture();
    let a = tasks::add(
        &mut f.ledger,
        &f.idea,
        " plot the loss ",
        TaskKind::Plain,
        None,
        Some("b1"),
    )
    .unwrap();
    assert_eq!(a.task.id.as_str(), "t001");
    assert_eq!(a.task.text, "plot the loss");
    assert_eq!(a.task.block_id.as_deref(), Some("b1"));
    assert_eq!(
        (a.idea.as_deref(), a.seed.as_deref()),
        (Some("wide-lr"), None)
    );
    assert_eq!(
        last_event(&f.ledger),
        (
            "todo_new".into(),
            "t001".into(),
            "{\"author\": \"human\", \"idea\": \"wide-lr\", \"kind\": \"plain\"}".into()
        )
    );
    let s = tasks::add(
        &mut f.ledger,
        &f.seed,
        "read the paper",
        TaskKind::Plain,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        (s.idea.as_deref(), s.seed.as_deref()),
        (None, Some("low-rank"))
    );
    assert_eq!(s.owner_name(), "low-rank");
    assert_eq!(
        last_event(&f.ledger).2,
        "{\"author\": \"human\", \"kind\": \"plain\", \"seed\": \"low-rank\"}"
    );

    let err = |r: arboria_core::Result<tasks::TaskView>| r.unwrap_err().to_string();
    assert_eq!(
        err(tasks::add(
            &mut f.ledger,
            &f.merged,
            "x",
            TaskKind::Plain,
            None,
            None
        )),
        "idea 'old' is merged"
    );
    assert_eq!(
        err(tasks::add(
            &mut f.ledger,
            &f.dropped_seed,
            "x",
            TaskKind::Plain,
            None,
            None
        )),
        "seed 'gone' is dropped"
    );
    assert_eq!(
        err(tasks::add(
            &mut f.ledger,
            &f.idea,
            "  ",
            TaskKind::Plain,
            None,
            None
        )),
        "todo must not be empty"
    );
    assert_eq!(
        err(tasks::get_task(&f.ledger, &t("t009"))),
        "no todo 't009' (see `arb todo list`)"
    );

    // re-running a block finds the open todo by its stripped text
    let again = tasks::open_plain_task(&f.ledger, f.idea.id, "plot the loss\n").unwrap();
    assert_eq!(again.unwrap().task.id, a.task.id);
    assert!(
        tasks::open_plain_task(&f.ledger, f.seed.id, "plot the loss")
            .unwrap()
            .is_none()
    );

    let ids = |v: Vec<tasks::TaskView>| -> Vec<String> {
        v.into_iter().map(|x| x.task.id.to_string()).collect()
    };
    assert_eq!(
        ids(tasks::list_tasks(&f.ledger, None, false).unwrap()),
        ["t001", "t002"]
    );
    let done = tasks::done(&mut f.ledger, &a.task.id, " done: see cell 3 ").unwrap();
    assert_eq!(done.task.status, TaskStatus::Done);
    assert_eq!(done.task.result.as_deref(), Some("done: see cell 3"));
    assert!(done.task.done_at.is_some());
    assert_eq!(
        last_event(&f.ledger),
        (
            "todo_done".into(),
            "t001".into(),
            "{\"idea\": \"wide-lr\", \"result_chars\": 16}".into()
        )
    );
    assert_eq!(
        err(tasks::done(&mut f.ledger, &a.task.id, "again")),
        "todo t001 is already done"
    );
    let dropped = tasks::drop_task(&mut f.ledger, &s.task.id, None).unwrap();
    assert_eq!(dropped.task.result, None);
    assert_eq!(
        last_event(&f.ledger).2,
        "{\"result_chars\": 0, \"seed\": \"low-rank\"}"
    );
    assert!(tasks::list_tasks(&f.ledger, None, false)
        .unwrap()
        .is_empty());
    assert_eq!(
        ids(tasks::list_tasks(&f.ledger, None, true).unwrap()),
        ["t001", "t002"]
    );
    assert_eq!(
        ids(tasks::list_tasks(&f.ledger, Some(f.idea.id), true).unwrap()),
        ["t001"]
    );
    assert_eq!(
        err(tasks::done(&mut f.ledger, &a.task.id, &"x".repeat(601))),
        "result is too long: 601 chars (max 600); shorten it"
    );
}

#[test]
fn command_and_inspect_todos() {
    let mut f = fixture();
    let data = json!({"name": "svd", "args": ["r"]});
    tasks::add(
        &mut f.ledger,
        &f.idea,
        "write svd",
        TaskKind::CommandNew,
        Some(&data),
        None,
    )
    .unwrap();
    let other = json!({"name": "plot2"});
    tasks::add(
        &mut f.ledger,
        &f.idea,
        "write plot2",
        TaskKind::CommandNew,
        Some(&other),
        None,
    )
    .unwrap();
    let rev = json!({"name": "svd", "ask": "faster"});
    tasks::add(
        &mut f.ledger,
        &f.seed,
        "revise svd",
        TaskKind::CommandRevise,
        Some(&rev),
        None,
    )
    .unwrap();
    let found = tasks::open_command_tasks(&f.ledger, "svd", None).unwrap();
    assert_eq!(
        found.iter().map(|x| x.task.id.as_str()).collect::<Vec<_>>(),
        ["t001", "t003"]
    );
    let only_new = tasks::open_command_tasks(&f.ledger, "svd", Some(TaskKind::CommandNew)).unwrap();
    assert_eq!(only_new.len(), 1);
    assert_eq!(
        found[0].task.payload_json.as_deref(),
        Some("{\"args\": [\"r\"], \"name\": \"svd\"}")
    );
    assert_eq!(tasks::payload(&found[0].task).unwrap(), data);

    let q = tasks::add(
        &mut f.ledger,
        &f.idea,
        "how is the loss computed",
        TaskKind::Inspect,
        None,
        None,
    )
    .unwrap();
    assert_eq!(tasks::payload(&q.task).unwrap(), json!({}));
    let msg = tasks::done(&mut f.ledger, &q.task.id, "x")
        .unwrap_err()
        .to_string();
    assert_eq!(
        msg,
        "t004 是读代码待办：用 `arb inspect done t004 --file …` 交付答案（代码由 arb 从 git 取），约定见 `arb inspect guide`；做不了就 `arb todo drop t004`"
    );
}

#[test]
fn queue_retries_then_needs_human() {
    let mut f = fixture();
    let a = tasks::add(&mut f.ledger, &f.idea, "a", TaskKind::Plain, None, None).unwrap();
    let b = tasks::add(&mut f.ledger, &f.idea, "b", TaskKind::Plain, None, None).unwrap();
    assert!(
        tasks::claim_next(&mut f.ledger, "m").unwrap().is_none(),
        "nothing queued"
    );
    tasks::enqueue(&mut f.ledger, &b.task.id).unwrap();
    tasks::enqueue(&mut f.ledger, &a.task.id).unwrap();
    assert_eq!(
        tasks::enqueue(&mut f.ledger, &a.task.id)
            .unwrap_err()
            .to_string(),
        "todo t001 is queued, not open"
    );
    assert_eq!(last_event(&f.ledger).0, "todo_queue");

    let usage = Usage {
        input_tokens: 100,
        output_tokens: 10,
        cache_read_tokens: 50,
        turns: 2,
        cost_usd: 0.25,
    };
    // oldest first; three attempts, then the human
    for attempt in 1..=3 {
        let run = tasks::claim_next(&mut f.ledger, "sonnet").unwrap().unwrap();
        assert_eq!(run.task.id, a.task.id);
        assert_eq!(run.task.status, TaskStatus::Running);
        assert_eq!(run.task.attempts, attempt);
        assert!(run.task.finished_at.is_none());
        let end = tasks::finish_attempt(
            &mut f.ledger,
            &a.task.id,
            &format!("s{attempt}"),
            &usage,
            TaskStatus::CheckFailed,
        )
        .unwrap();
        let want = if attempt < 3 {
            TaskStatus::CheckFailed
        } else {
            TaskStatus::NeedsHuman
        };
        assert_eq!(end.task.status, want);
        assert_eq!(end.task.session_id, Some(format!("s{attempt}")));
    }
    let end = tasks::get_task(&f.ledger, &a.task.id).unwrap().task;
    assert_eq!(
        end.usage_json.as_deref(),
        Some(
            "{\"cache_read_tokens\": 150, \"cost_usd\": 0.75, \"input_tokens\": 300, \"output_tokens\": 30, \"turns\": 6}"
        )
    );
    assert_eq!(
        last_event(&f.ledger),
        (
            "todo_needs_human".into(),
            "t001".into(),
            "{\"attempts\": 3}".into()
        )
    );
    assert_eq!(end.done_at, None);

    // the agent closes b itself during its attempt; finish keeps it done
    let run = tasks::claim_next(&mut f.ledger, "sonnet").unwrap().unwrap();
    assert_eq!(run.task.id, b.task.id);
    assert_eq!(
        tasks::finish_attempt(&mut f.ledger, &b.task.id, "s", &usage, TaskStatus::Done)
            .unwrap_err()
            .to_string(),
        "todo t002 has no result: the agent closes it with `arb todo done` (or `arb inspect done`)"
    );
    tasks::done(&mut f.ledger, &b.task.id, "ok").unwrap();
    let end =
        tasks::finish_attempt(&mut f.ledger, &b.task.id, "s9", &usage, TaskStatus::Done).unwrap();
    assert_eq!(end.task.status, TaskStatus::Done);
    assert_eq!(end.task.session_id.as_deref(), Some("s9"));
    assert!(tasks::claim_next(&mut f.ledger, "m").unwrap().is_none());
    assert_eq!(
        tasks::finish_attempt(
            &mut f.ledger,
            &a.task.id,
            "s",
            &usage,
            TaskStatus::CheckFailed
        )
        .unwrap_err()
        .to_string(),
        "todo t001 is needs_human, not running"
    );
    // the human can still drop it
    tasks::drop_task(&mut f.ledger, &a.task.id, Some("not worth it")).unwrap();
}
