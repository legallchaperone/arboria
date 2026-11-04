//! 读代码 against a real repo (mirrors tests/test_inspection.py where the behaviour is the
//! core's): ask → todo bound to a read-only snapshot of the worktree, deliver, fresh /
//! stale / refresh / dropped, the files at code_ref; the text rendering and the guide are
//! compared with CPython.

mod common;

use std::path::Path;
use std::process::Command;

use arboria_core::db::{self, Ledger};
use arboria_core::ideas::{self, NewIdea};
use arboria_core::inspection::{self, InspectStatus};
use arboria_core::model::{EventKind, IdeaName, InspectView, TaskStatus};
use arboria_core::{gitutil, paths, tasks};
use common::TempDir;
use serde_json::json;

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

fn python(code: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("python3")
        .args(["-c", code])
        .args(args)
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

#[test]
fn ask_deliver_stale_refresh_drop() {
    let tmp = TempDir::new("inspect");
    let root = tmp.path().canonicalize().unwrap();
    let main = root.join("WaveFront");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    let train: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    std::fs::write(main.join("train.py"), &train).unwrap();
    std::fs::write(main.join("crlf.py"), "a = 1\r\nb = 2\r\n").unwrap();
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "init"]);
    let ctx = paths::locate(Some(&main)).unwrap();
    let mut ledger = Ledger::open(&ctx.state_dir()).unwrap();
    db::init_backup_repo(&ctx.state_dir()).unwrap();
    let idea = ideas::new_idea(
        &ctx,
        &mut ledger,
        &IdeaName::parse("base").unwrap(),
        NewIdea {
            hypothesis: "h",
            metric: "m",
            ..Default::default()
        },
        &mut |_| {},
    )
    .unwrap();
    let wt = Path::new(idea.worktree_path.as_deref().unwrap()).to_path_buf();
    // uncommitted work counts; the user's index and HEAD stay as they are
    std::fs::write(wt.join("new.py"), "x = 1\n").unwrap();
    let head = git(&wt, &["rev-parse", "HEAD"]);
    let status = git(&wt, &["status", "--porcelain"]);

    let q = "  训练   循环 ";
    let peek =
        inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, false, false).unwrap();
    assert!(peek.todo.is_none() && peek.answer.is_none());
    let st = inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, false, true).unwrap();
    assert!(st.created);
    assert_eq!(st.question, "训练 循环");
    assert_eq!(st.status(), InspectStatus::Pending);
    let todo = st.todo.clone().unwrap();
    let payload = tasks::payload(&todo.task).unwrap();
    let code_ref = payload["code_ref"].as_str().unwrap().to_string();
    assert_eq!(
        gitutil::rev_parse(&inspection::ref_name(&todo.task.id), &main).unwrap(),
        Some(code_ref.clone())
    );
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&wt, &["status", "--porcelain"]), status);
    assert!(todo
        .task
        .text
        .starts_with("读代码（代码摘录）：训练 循环\n代码：当前代码 @ "));
    assert_eq!(
        inspection::status_line(&st),
        format!("已交给 agent（{}）：代码摘录 · 训练 循环。", todo.task.id)
    );
    // re-running the cell reuses the open todo
    let again =
        inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, false, true).unwrap();
    assert!(!again.created);
    assert_eq!(again.todo.unwrap().task.id, todo.task.id);

    // the files at code_ref include the uncommitted one
    let files = inspection::source_text(&ledger, &main, &todo.task.id, None).unwrap();
    assert_eq!(files, "crlf.py\nnew.py\ntrain.py\n");
    let src = inspection::source_text(&ledger, &main, &todo.task.id, Some("crlf.py")).unwrap();
    assert_eq!(
        src,
        format!(
            "# crlf.py @ {} (2 lines)\n1  a = 1\r\n2  b = 2\r\n",
            &code_ref[..7]
        )
    );

    // deliver: arb takes the lines from git; the todo is done
    let answer = json!({"title": "训练循环", "summary": "两句话",
        "spans": [{"id": "loop", "path": "train.py", "start": 3, "end": 5, "note": "核心",
                   "line_notes": {"4": "要点"}},
                  {"path": "crlf.py", "start": 1, "end": 2}]});
    let checked = inspection::check(&ledger, &main, &todo.task.id, &answer).unwrap();
    assert!(checked.ends_with(&format!(
        "ok: valid for {0} (nothing saved; deliver with `arb inspect done {0} --file …`)",
        todo.task.id
    )));
    let result = inspection::deliver(&mut ledger, &main, &todo.task.id, &answer).unwrap();
    assert_eq!(
        result["spans"][0]["lines"],
        json!(["line 3", "line 4", "line 5"])
    );
    assert_eq!(result["spans"][1]["lines"], json!(["a = 1\r", "b = 2\r"]));
    let done = tasks::get_task(&ledger, &todo.task.id).unwrap();
    assert_eq!(done.task.status, TaskStatus::Done);
    assert_eq!(done.task.result.as_deref(), Some("训练循环：两句话"));
    let kinds: Vec<String> = ledger
        .conn()
        .prepare("SELECT kind FROM events WHERE subject = ?1 ORDER BY id")
        .unwrap()
        .query_map([todo.task.id.as_str()], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        &kinds[kinds.len() - 2..],
        [
            EventKind::InspectDone.as_str(),
            EventKind::TodoDone.as_str()
        ]
    );
    assert!(tasks::done(&mut ledger, &todo.task.id, "x").is_err());
    assert_eq!(
        inspection::deliver(&mut ledger, &main, &todo.task.id, &answer)
            .unwrap_err()
            .to_string(),
        format!("todo {} is already done", todo.task.id)
    );

    // the plain text is Python's
    if let Some(py) = python(
        "import json, sys; from arboria import inspection as i\n\
         sys.stdout.write(i.to_text(None, json.loads(sys.argv[1])))",
        &[&result.to_string()],
    ) {
        assert_eq!(inspection::to_text(None, Some(&result)), py);
    }

    let st = inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, false, true).unwrap();
    assert_eq!(st.status(), InspectStatus::Fresh);
    assert!(!st.created && st.todo.is_none());
    let text = inspection::to_text(Some(&st), None);
    assert!(
        text.starts_with("训练循环  [读代码 · 代码摘录]\n\n两句话"),
        "{text}"
    );

    // the worktree changes: stale, nothing asked until refresh
    std::fs::write(wt.join("train.py"), format!("{train}line 21\n")).unwrap();
    let st = inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, false, true).unwrap();
    assert_eq!(st.status(), InspectStatus::Stale);
    assert!(!st.created);
    assert_eq!(
        inspection::status_line(&st),
        format!(
            "代码已变（答案基于 {}，之后工作区改过），.refresh() 重新生成",
            &code_ref[..7]
        )
    );
    let st = inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, true, true).unwrap();
    assert!(st.created);
    assert_eq!(st.status(), InspectStatus::Pending);
    assert_eq!(
        inspection::status_line(&st),
        format!(
            "已交给 agent（{}）：代码摘录 · 训练 循环，下面是上一版。",
            st.todo.as_ref().unwrap().task.id
        )
    );
    // the agent drops it: shown as dropped, refresh asks again
    let t2 = st.todo.unwrap().task.id;
    tasks::drop_task(&mut ledger, &t2, Some("太长了")).unwrap();
    let st = inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, false, true).unwrap();
    assert_eq!(st.status(), InspectStatus::Dropped);
    assert_eq!(
        inspection::status_line(&st),
        format!("agent 没有做（{t2}：太长了）。.refresh() 重新交给 agent")
    );
    let st = inspection::ask(&mut ledger, &idea, q, InspectView::Code, None, true, true).unwrap();
    assert!(st.created);
}

#[test]
fn guide_is_pythons() {
    let Some(py) = python(
        "import sys; from arboria import inspection as i; sys.stdout.write(i.GUIDE + '\\0' + i.guide())",
        &[],
    ) else {
        return;
    };
    let (template, rendered) = py.split_once('\0').unwrap();
    assert_eq!(include_str!("../src/inspection_guide.md"), template);
    assert_eq!(inspection::guide(), rendered);
}
