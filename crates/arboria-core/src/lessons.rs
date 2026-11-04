//! Lessons: durable project pitfalls. The agent proposes them; only the human accepts
//! them. Accepted lessons go into the managed CLAUDE.md block ([`crate::claudemd`]) and
//! the SessionStart hook. Ports `arboria.lessons`.
//!
//! Invariants:
//! - A lesson is one line ≤200 code points without `<!--` / `-->` (LLM-writable,
//!   [`crate::records::validate_text`]).
//! - `accept` only in the human's terminal ([`crate::human::require_human`]: refused in
//!   Claude Code or without a TTY, then the human types the id); install-claude denies it.
//! - At most [`crate::claudemd::MAX_ACCEPTED_LESSONS`] accepted lessons and a block of at
//!   most [`crate::claudemd::MAX_BLOCK_LINES`] lines: a lesson that does not fit is
//!   refused with [`CAP_HINT`].
//! - Every change emits its `lesson_*` event (backup commit) and syncs CLAUDE.md; a failed
//!   sync only warns.

use serde::Serialize;
use serde_json::json;

use crate::claudemd::{self, SyncResult};
use crate::db::{self, now, Ledger, WriteTx};
use crate::model::{
    DecisionId, EventKind, Lesson, LessonId, LessonSource, LessonStatus, RunId, SubjectType,
};
use crate::paths::RepoContext;
use crate::records::{self, py_repr, TextField};
use crate::{bail, human, ideas, Error, Result};

/// The lesson Decision's question.
pub const QUESTION: &str = "接受这条经验吗？";
/// Its options.
pub const OPTIONS: &[&str] = &["accept", "reject"];
/// When the block is full.
pub const CAP_HINT: &str = "retire or merge some first (`arb lesson retire ID`)";

/// What a lesson command did.
#[derive(Debug, Clone, Serialize)]
pub struct LessonResult {
    pub lesson: Lesson,
    pub sync: Option<SyncResult>,
    pub warning: Option<String>,
}

impl LessonResult {
    fn of(lesson: Lesson) -> Self {
        LessonResult {
            lesson,
            sync: None,
            warning: None,
        }
    }
}

/// A lesson by id.
pub fn get_lesson(ledger: &Ledger, lesson_id: &LessonId) -> Result<Lesson> {
    get_lesson_in(ledger.conn(), lesson_id.as_str())
}

fn get_lesson_in(conn: &rusqlite::Connection, lesson_id: &str) -> Result<Lesson> {
    db::one(conn, "SELECT * FROM lessons WHERE id = ?1", [lesson_id])?.ok_or_else(|| {
        Error::Arb(format!(
            "no lesson {} (see `arb lesson list --all`)",
            py_repr(lesson_id)
        ))
    })
}

/// `arb lesson list`: proposed + accepted (all with `include_retired`), newest first.
pub fn list_lessons(ledger: &Ledger, include_retired: bool) -> Result<Vec<Lesson>> {
    let filter = if include_retired {
        ""
    } else {
        " WHERE status IN ('proposed', 'accepted')"
    };
    db::all(
        ledger.conn(),
        &format!("SELECT * FROM lessons{filter} ORDER BY created_at DESC, id DESC"),
        [],
    )
}

/// Proposed lessons waiting for the human.
pub fn proposed_count(ledger: &Ledger) -> Result<i64> {
    Ok(ledger.conn().query_row(
        "SELECT COUNT(*) FROM lessons WHERE status = 'proposed'",
        [],
        |r| r.get(0),
    )?)
}

fn open_decision_id(tx: &rusqlite::Connection, lesson_id: &LessonId) -> Result<Option<DecisionId>> {
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM decisions WHERE subject_type = 'lesson' AND subject_id = ?1 \
             AND choice IS NULL ORDER BY id DESC LIMIT 1",
            [lesson_id],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    id.map(|s| DecisionId::parse(&s)).transpose()
}

/// Refuse if accepting `candidate` would exceed the lesson count or the block line cap.
fn check_room(ledger: &Ledger, candidate: &Lesson) -> Result<()> {
    let accepted = claudemd::accepted_lessons(ledger, None)?;
    if accepted.len() >= claudemd::MAX_ACCEPTED_LESSONS {
        bail!(
            "there are already {} accepted lessons (max {}); {CAP_HINT}",
            accepted.len(),
            claudemd::MAX_ACCEPTED_LESSONS
        );
    }
    let mut all = vec![candidate.clone()];
    all.extend(accepted);
    if claudemd::render_block(&all).is_err() {
        bail!(
            "accepting this lesson would overflow the CLAUDE.md block (max {} lines); {CAP_HINT}",
            claudemd::MAX_BLOCK_LINES
        );
    }
    Ok(())
}

fn sync_after(
    ctx: &RepoContext,
    ledger: &Ledger,
    mut result: LessonResult,
) -> Result<LessonResult> {
    match claudemd::sync(ctx, ledger, true) {
        Ok(s) => result.sync = Some(s),
        // the ledger change is committed; the CLI tells the user how to finish
        Err(e) if e.is_arb() => result.warning = Some(format!("CLAUDE.md not updated: {e}")),
        Err(e) => return Err(e),
    }
    Ok(result)
}

/// `arb lesson propose TEXT [--idea] [--run]`: a proposed lesson plus an open Decision
/// for the human. Returns the lesson and that Decision's id.
pub fn propose(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    text: &str,
    idea: Option<&str>,
    run: Option<&RunId>,
) -> Result<(Lesson, DecisionId)> {
    let text = records::validate_text(TextField::Lesson, Some(text), true)?.expect("required text");
    let (source_type, source_id) = match (idea, run) {
        (Some(_), Some(_)) => bail!("use only one of --idea / --run"),
        (_, Some(run)) => {
            records::get_run(ledger, run)?;
            (LessonSource::Run, Some(run.to_string()))
        }
        (Some(name), None) => {
            let found = ideas::idea_by_name(ledger.conn(), name)?.ok_or_else(|| {
                Error::Arb(format!(
                    "no idea named {} (see `arb idea list`)",
                    py_repr(name)
                ))
            })?;
            (LessonSource::Idea, Some(found.name.to_string()))
        }
        (None, None) => match ideas::current_idea(ctx, ledger)? {
            Some(cur) => (LessonSource::Idea, Some(cur.name.to_string())),
            None => (LessonSource::None, None),
        },
    };
    let dup: Option<(String, String)> = ledger
        .conn()
        .query_row(
            "SELECT id, status FROM lessons WHERE text = ?1 AND status != 'retired'",
            [&text],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    if let Some((id, status)) = dup {
        bail!("the same lesson already exists as {id} ({status})");
    }
    let ts = now();
    let candidate = Lesson {
        id: LessonId::parse("l000")?,
        text: text.clone(),
        source_type,
        source_id: source_id.clone(),
        status: LessonStatus::Proposed,
        created_at: ts.clone(),
        decided_at: Some(ts.clone()),
    };
    check_room(ledger, &candidate)?;
    let (lesson_id, dec_id) = ledger.write(|tx: &mut WriteTx<'_>| {
        let lesson_id: LessonId = tx.next_id()?;
        tx.execute(
            "INSERT INTO lessons (id, text, source_type, source_id, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'proposed', ?5)",
            rusqlite::params![lesson_id, text, source_type.as_str(), source_id, ts],
        )?;
        let dec_id = records::insert_decision(
            tx,
            SubjectType::Lesson,
            lesson_id.as_str(),
            QUESTION,
            OPTIONS,
        )?;
        tx.event(
            EventKind::LessonPropose,
            lesson_id.as_str(),
            &json!({"source": format!("{}:{}", source_type.as_str(), source_id.as_deref().unwrap_or("")),
                    "decision": dec_id.as_str()}),
        )?;
        Ok((lesson_id, dec_id))
    })?;
    Ok((get_lesson(ledger, &lesson_id)?, dec_id))
}

/// `arb lesson accept` (human only): refused inside Claude Code or without a TTY; the
/// human sees the lesson (`show`) and types its id. Decides its Decision and syncs
/// CLAUDE.md.
pub fn accept(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    lesson_id: &LessonId,
    show: &mut dyn FnMut(&str),
) -> Result<LessonResult> {
    human::require_terminal(&format!("arb lesson accept {lesson_id}"))?;
    accept_confirmed(ctx, ledger, lesson_id, show, &mut |id| human::confirm(id))
}

/// [`accept`] after the terminal check; `confirm` is [`human::confirm`] (unit tests pass
/// the test-only token).
fn accept_confirmed(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    lesson_id: &LessonId,
    show: &mut dyn FnMut(&str),
    confirm: &mut dyn FnMut(&str) -> Result<human::Confirmed>,
) -> Result<LessonResult> {
    let lesson = get_lesson(ledger, lesson_id)?;
    if lesson.status != LessonStatus::Proposed {
        bail!("lesson {lesson_id} is {}, not proposed", lesson.status);
    }
    let ts = now();
    let mut candidate = lesson.clone();
    candidate.decided_at = Some(ts.clone());
    check_room(ledger, &candidate)?;
    let mut all = vec![candidate];
    all.extend(claudemd::accepted_lessons(ledger, None)?);
    claudemd::check(ctx, &all)?;
    show(&format!("{lesson_id}: {}", lesson.text));
    let confirmed = confirm(lesson_id.as_str())?;
    let prov = serde_json::to_value(confirmed.provenance())?;
    ledger.write(|tx| {
        tx.execute(
            "UPDATE lessons SET status = 'accepted', decided_at = ?1 WHERE id = ?2",
            rusqlite::params![ts, lesson_id],
        )?;
        let dec_id = open_decision_id(tx, lesson_id)?;
        if let Some(d) = &dec_id {
            records::set_decision(tx, d, "accept", records::AUTHOR_HUMAN, Some(&prov))?;
        }
        tx.event(
            EventKind::LessonAccept,
            lesson_id.as_str(),
            &json!({"decision": dec_id.as_ref().map(|d| d.as_str()),
                    "author": records::AUTHOR_HUMAN}),
        )
    })?;
    let lesson = get_lesson(ledger, lesson_id)?;
    sync_after(ctx, ledger, LessonResult::of(lesson))
}

/// `arb lesson reject`: a proposed lesson becomes retired.
pub fn reject(
    _ctx: &RepoContext,
    ledger: &mut Ledger,
    lesson_id: &LessonId,
) -> Result<LessonResult> {
    let lesson = get_lesson(ledger, lesson_id)?;
    if lesson.status != LessonStatus::Proposed {
        let hint = if lesson.status == LessonStatus::Accepted {
            "; use `arb lesson retire` for an accepted one"
        } else {
            ""
        };
        bail!(
            "lesson {lesson_id} is {}, not proposed{hint}",
            lesson.status
        );
    }
    ledger.write(|tx| {
        tx.execute(
            "UPDATE lessons SET status = 'retired', decided_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), lesson_id],
        )?;
        let dec_id = open_decision_id(tx, lesson_id)?;
        if let Some(d) = &dec_id {
            records::set_decision(tx, d, "reject", records::AUTHOR_HUMAN, None)?;
        }
        tx.event(
            EventKind::LessonReject,
            lesson_id.as_str(),
            &json!({"decision": dec_id.as_ref().map(|d| d.as_str())}),
        )
    })?;
    Ok(LessonResult::of(get_lesson(ledger, lesson_id)?))
}

/// `arb lesson retire`: an accepted lesson leaves CLAUDE.md.
pub fn retire(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    lesson_id: &LessonId,
) -> Result<LessonResult> {
    let lesson = get_lesson(ledger, lesson_id)?;
    if lesson.status != LessonStatus::Accepted {
        let hint = if lesson.status == LessonStatus::Proposed {
            "; use `arb lesson reject` for a proposed one"
        } else {
            ""
        };
        bail!(
            "lesson {lesson_id} is {}, not accepted{hint}",
            lesson.status
        );
    }
    let rest: Vec<Lesson> = claudemd::accepted_lessons(ledger, None)?
        .into_iter()
        .filter(|l| l.id != *lesson_id)
        .collect();
    claudemd::check(ctx, &rest)?;
    ledger.write(|tx| {
        tx.execute(
            "UPDATE lessons SET status = 'retired', decided_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), lesson_id],
        )?;
        tx.event(EventKind::LessonRetire, lesson_id.as_str(), &json!({}))
    })?;
    let lesson = get_lesson(ledger, lesson_id)?;
    sync_after(ctx, ledger, LessonResult::of(lesson))
}

#[cfg(test)]
mod tests {
    //! `accept` past the terminal check, with the test-only token (Python's
    //! tests/test_lessons.py fakes the TTY and the typed id instead).

    use super::*;
    use crate::paths;
    use std::path::Path;
    use std::process::Command;

    fn git(cwd: &Path, args: &[&str]) {
        let o = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {o:?}");
    }

    #[test]
    fn accept_records_the_humans_decision_and_syncs_the_block() {
        let tmp = std::env::temp_dir().join(format!("arb-lessons-{}", std::process::id()));
        let main = tmp.join("WaveFront");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["config", "user.name", "t"]);
        git(&main, &["config", "user.email", "t@t"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let ctx = paths::locate(Some(&main)).unwrap();
        crate::state::init_state(&ctx, &mut |_| {}).unwrap();
        let mut ledger = Ledger::open(&ctx.state_dir()).unwrap();
        let (l, _) = propose(&ctx, &mut ledger, "pin the seed", None, None).unwrap();

        let mut shown = Vec::new();
        let mut typed = Vec::new();
        let r = accept_confirmed(
            &ctx,
            &mut ledger,
            &l.id,
            &mut |t| shown.push(t.to_string()),
            &mut |id| {
                typed.push(id.to_string());
                Ok(human::Confirmed::for_tests())
            },
        )
        .unwrap();
        assert_eq!(shown, ["l001: pin the seed"]);
        assert_eq!(typed, ["l001"]);
        assert_eq!(r.lesson.status, LessonStatus::Accepted);
        let sync = r.sync.unwrap();
        assert_eq!(sync.action.as_deref(), Some("replaced"));
        assert!(sync.committed);
        let block = std::fs::read_to_string(main.join("CLAUDE.md")).unwrap();
        let date = &r.lesson.decided_at.unwrap()[..10];
        assert!(
            block.contains(&format!("- pin the seed（{date}）\n")),
            "{block}"
        );
        let (by, payload): (String, String) = ledger
            .conn()
            .query_row(
                "SELECT decided_by, payload_json FROM decisions WHERE id = 'd001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(by, records::AUTHOR_HUMAN);
        assert!(
            payload.contains("\"confirmed_by\": \"unit test\""),
            "{payload}"
        );
        assert_eq!(
            accept_confirmed(
                &ctx,
                &mut ledger,
                &l.id,
                &mut |_| {},
                &mut |_| unreachable!()
            )
            .unwrap_err()
            .to_string(),
            "lesson l001 is accepted, not proposed"
        );

        // retire takes it out of the block again
        let r = retire(&ctx, &mut ledger, &l.id).unwrap();
        assert_eq!(r.lesson.status, LessonStatus::Retired);
        let block = std::fs::read_to_string(main.join("CLAUDE.md")).unwrap();
        assert!(block.contains("- 暂无\n"), "{block}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
