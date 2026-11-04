//! Writes for runs, notes and decisions, and the checks on the free-text fields an LLM
//! may write. Ports `arboria.records` plus `arboria.db.validate_text` / `LIMITS` /
//! `validate_name` / `validate_choice`.
//!
//! Invariants:
//! - LLM-writable text is length-checked before any write ([`validate_text`], real). The
//!   limits are Python's `db.LIMITS`, counted in Unicode code points after Python's
//!   `str.strip()`; they are part of the contract and must not be removed or loosened.
//! - Notes are the human's (`author = 'human'`); the CLI refuses `arb note` inside Claude Code.
//! - Every write goes through [`crate::db::Ledger::write`].

use std::path::Path;

use crate::db::{Ledger, WriteTx};
use crate::model::{Confidence, Decision, DecisionId, Idea, IdeaId, Note, Run, RunId, SubjectType};
use crate::snapshot::SnapshotResult;
use crate::{Error, Result, Warn};

// ---------------------------------------------------------------- free-text fields

/// The free-text fields with a length limit (`db.LIMITS` keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextField {
    /// An idea's hypothesis (seeds: human only).
    Hypothesis,
    /// An idea's metric (seeds: human only).
    Metric,
    /// Why a run differs from the previous one.
    ChangeReason,
    /// A note (human only).
    Note,
    /// A run's result summary (also ≤10 lines).
    Summary,
    /// A lesson (one line, no HTML comment markers).
    Lesson,
    /// A todo's result.
    Result,
    /// A todo's text (the human's).
    Todo,
    /// A seed's one-line gist (human only).
    Gist,
    /// `lab.inspect`: the human's question.
    Question,
    /// `lab.inspect` answer: title.
    InspectTitle,
    /// `lab.inspect` answer: summary.
    InspectSummary,
    /// `lab.inspect` answer: a span's note.
    SpanNote,
    /// `lab.inspect` answer: a line note.
    LineNote,
    /// `lab.inspect` answer: pseudocode / simplified code.
    InspectText,
    /// `lab.inspect` answer: Mermaid source.
    Mermaid,
}

impl TextField {
    /// Every field.
    pub const ALL: &'static [TextField] = &[
        TextField::Hypothesis,
        TextField::Metric,
        TextField::ChangeReason,
        TextField::Note,
        TextField::Summary,
        TextField::Lesson,
        TextField::Result,
        TextField::Todo,
        TextField::Gist,
        TextField::Question,
        TextField::InspectTitle,
        TextField::InspectSummary,
        TextField::SpanNote,
        TextField::LineNote,
        TextField::InspectText,
        TextField::Mermaid,
    ];

    /// The `db.LIMITS` key (also used in error messages).
    pub fn name(self) -> &'static str {
        match self {
            TextField::Hypothesis => "hypothesis",
            TextField::Metric => "metric",
            TextField::ChangeReason => "change_reason",
            TextField::Note => "note",
            TextField::Summary => "summary",
            TextField::Lesson => "lesson",
            TextField::Result => "result",
            TextField::Todo => "todo",
            TextField::Gist => "gist",
            TextField::Question => "question",
            TextField::InspectTitle => "inspect_title",
            TextField::InspectSummary => "inspect_summary",
            TextField::SpanNote => "span_note",
            TextField::LineNote => "line_note",
            TextField::InspectText => "inspect_text",
            TextField::Mermaid => "mermaid",
        }
    }

    /// Maximum length in code points (Python `db.LIMITS`).
    pub fn limit(self) -> usize {
        match self {
            TextField::Hypothesis => 300,
            TextField::Metric => 200,
            TextField::ChangeReason => 300,
            TextField::Note => 1000,
            TextField::Summary => 1500,
            TextField::Lesson => 200,
            TextField::Result => 600,
            TextField::Todo => 1000,
            TextField::Gist => 300,
            TextField::Question => 300,
            TextField::InspectTitle => 80,
            TextField::InspectSummary => 600,
            TextField::SpanNote => 600,
            TextField::LineNote => 200,
            TextField::InspectText => 8000,
            TextField::Mermaid => 8000,
        }
    }
}

/// A run summary has at most this many lines (`db.SUMMARY_MAX_LINES`).
pub const SUMMARY_MAX_LINES: usize = 10;

/// Python's `str.isspace()` for one character: Rust's `char::is_whitespace` plus the
/// information separators U+001C..U+001F, which Python strips and Rust does not.
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// The first code point of each run of ten Unicode decimal digits (category Nd, value 0
/// at the start), Unicode 15.1 as in Python 3.13. Every Nd character is in one of these
/// runs, in value order.
const ND_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x11F50, 0x16A60, 0x16AC0,
    0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950,
    0x1FBF0,
];

/// The value of a Unicode decimal digit: what Python's regex `\d` matches and `int()`
/// accepts (`'١'` → 1, `'１'` → 1). None for anything else.
pub fn py_decimal(c: char) -> Option<u32> {
    let c = c as u32;
    let i = ND_ZEROS.partition_point(|&z| z <= c);
    let zero = *ND_ZEROS.get(i.checked_sub(1)?)?;
    (c - zero < 10).then_some(c - zero)
}

/// Python's `repr(str)` for messages: single quotes unless the text has `'` and no `"`;
/// `\\`, the quote, `\n` `\r` `\t` and other control characters escaped (`\xhh`).
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || ('\u{7f}'..='\u{a0}').contains(&c) => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `db.validate_text(field, value, required=...)`: strip, then refuse empty (when
/// required) and over-long text; summaries ≤10 lines; lessons one line without `<!--` /
/// `-->`. Returns the stripped text (None when absent / empty and not required).
/// Messages are Python's.
pub fn validate_text(
    field: TextField,
    value: Option<&str>,
    required: bool,
) -> Result<Option<String>> {
    let name = field.name();
    let Some(value) = value else {
        return if required {
            Err(Error::Arb(format!("{name} is required")))
        } else {
            Ok(None)
        };
    };
    let value = py_strip(value);
    if value.is_empty() {
        return if required {
            Err(Error::Arb(format!("{name} must not be empty")))
        } else {
            Ok(None)
        };
    }
    let limit = field.limit();
    let n = value.chars().count();
    if n > limit {
        return Err(Error::Arb(format!(
            "{name} is too long: {n} chars (max {limit}); shorten it"
        )));
    }
    if field == TextField::Summary {
        let lines = value.matches('\n').count() + 1;
        if lines > SUMMARY_MAX_LINES {
            return Err(Error::Arb(format!(
                "summary has {lines} lines (max {SUMMARY_MAX_LINES}); shorten it"
            )));
        }
    }
    if field == TextField::Lesson {
        if value.contains('\n') || value.contains('\r') {
            return Err(Error::arb(
                "lesson must be a single line; state one pitfall per lesson",
            ));
        }
        if value.contains("<!--") || value.contains("-->") {
            return Err(Error::arb(
                "lesson must not contain HTML comment markers (<!-- or -->)",
            ));
        }
    }
    Ok(Some(value.to_string()))
}

/// [`validate_text`] for a required field.
pub fn require_text(field: TextField, value: &str) -> Result<String> {
    Ok(validate_text(field, Some(value), true)?.expect("required text is Some"))
}

/// Python `db.validate_name(name, what)` (`what` = "idea", "seed", …).
pub fn validate_name(name: &str, what: &str) -> Result<()> {
    crate::model::IdeaName::parse(name).map(|_| ()).map_err(|_| {
        Error::Arb(format!(
            "invalid {what} name {}: use lowercase letters, digits and '-' (must start with a letter or digit, max 63 chars)",
            py_repr(name)
        ))
    })
}

// ---------------------------------------------------------------- runs

/// The hidden ref anchoring a run's snapshot commit: `refs/arboria/runs/<id>`.
pub fn run_ref(run_id: &RunId) -> String {
    format!("refs/arboria/runs/{run_id}")
}

/// A new draft run and how its snapshot was made (`records.new_run`'s dict).
#[derive(Debug, Clone, serde::Serialize)]
pub struct NewRun {
    #[serde(flatten)]
    pub run: Run,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub reused_snapshot: bool,
    pub warnings: Vec<String>,
}

/// Snapshot limits and `--allow-large` (passed through to [`crate::snapshot`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct SnapshotOptions {
    pub allow_large: bool,
    pub file_limit: Option<u64>,
    pub total_limit: Option<u64>,
}

/// Insert a draft run at an existing snapshot commit and its ref (inside a write).
pub fn insert_run_at(
    tx: &mut WriteTx<'_>,
    idea: &Idea,
    reason: &str,
    commit: &str,
    tree: &str,
    parent_run: Option<&RunId>,
) -> Result<RunId> {
    let _ = (tx, idea, reason, commit, tree, parent_run);
    todo!("port of arboria.records.insert_run_at")
}

/// Snapshot the idea's worktree (no ledger write; the commit is anchored by a run ref).
pub fn take_snapshot(
    ledger: &Ledger,
    idea: &Idea,
    opts: SnapshotOptions,
    warn: Warn<'_>,
) -> Result<SnapshotResult> {
    let _ = (ledger, idea, opts, warn);
    todo!("port of arboria.records.take_snapshot")
}

/// Snapshot the idea's worktree and record a draft run (`arb run new`, hidden).
pub fn new_run(
    ledger: &mut Ledger,
    idea: &Idea,
    reason: &str,
    opts: SnapshotOptions,
    warn: Warn<'_>,
) -> Result<NewRun> {
    let _ = (ledger, idea, reason, opts, warn);
    todo!("port of arboria.records.new_run")
}

/// A run by id (`no run 'r009'`).
pub fn get_run(ledger: &Ledger, run_id: &RunId) -> Result<Run> {
    let _ = (ledger, run_id);
    todo!("port of arboria.records.get_run")
}

/// `arb run summary`: set a run's result summary (≤10 lines, ≤1500); the run must belong
/// to `idea`.
pub fn set_run_summary(
    ledger: &mut Ledger,
    idea: &Idea,
    run_id: &RunId,
    summary: &str,
) -> Result<Run> {
    let _ = (ledger, idea, run_id, summary);
    todo!("port of arboria.records.set_run_summary")
}

// ---------------------------------------------------------------- notes

/// Insert a note row (inside a write). Notes are written by the human.
pub fn insert_note(
    tx: &mut WriteTx<'_>,
    idea_id: IdeaId,
    text: &str,
    run_id: Option<&RunId>,
    confidence: Option<Confidence>,
    author: &str,
) -> Result<crate::model::NoteId> {
    let _ = (tx, idea_id, text, run_id, confidence, author);
    todo!("port of arboria.records.insert_note")
}

/// `arb note`: attach a human note; `confidence` also updates the idea.
pub fn add_note(
    ledger: &mut Ledger,
    idea: &Idea,
    text: &str,
    run_id: Option<&RunId>,
    confidence: Option<Confidence>,
) -> Result<Note> {
    let _ = (ledger, idea, text, run_id, confidence);
    todo!("port of arboria.records.add_note")
}

/// Set an idea's confidence (no event when unchanged).
pub fn set_confidence(ledger: &mut Ledger, idea: &Idea, confidence: Confidence) -> Result<Idea> {
    let _ = (ledger, idea, confidence);
    todo!("port of arboria.records.set_confidence")
}

/// Set an idea's metric (human only; no event when unchanged).
pub fn set_metric(ledger: &mut Ledger, idea: &Idea, metric: &str) -> Result<Idea> {
    let _ = (ledger, idea, metric);
    todo!("port of arboria.records.set_metric")
}

// ---------------------------------------------------------------- decisions

/// Insert an open decision (inside a write); emits a `decision` event.
pub fn insert_decision(
    tx: &mut WriteTx<'_>,
    subject_type: SubjectType,
    subject_id: &str,
    question: &str,
    options: &[&str],
) -> Result<DecisionId> {
    let _ = (tx, subject_type, subject_id, question, options);
    todo!("port of arboria.records.insert_decision")
}

/// Record the choice of an open decision (inside a write). `payload` (the approving
/// process chain) is stored on the decision.
pub fn set_decision(
    tx: &mut WriteTx<'_>,
    decision_id: &DecisionId,
    choice: &str,
    decided_by: &str,
    payload: Option<&serde_json::Value>,
) -> Result<()> {
    let _ = (tx, decision_id, choice, decided_by, payload);
    todo!("port of arboria.records.set_decision")
}

/// Open a decision in its own write.
pub fn open_decision(
    ledger: &mut Ledger,
    subject_type: SubjectType,
    subject_id: &str,
    question: &str,
    options: &[&str],
) -> Result<Decision> {
    let _ = (ledger, subject_type, subject_id, question, options);
    todo!("port of arboria.records.open_decision")
}

/// Decide a decision in its own write.
pub fn decide(
    ledger: &mut Ledger,
    decision_id: &DecisionId,
    choice: &str,
    decided_by: &str,
) -> Result<Decision> {
    let _ = (ledger, decision_id, choice, decided_by);
    todo!("port of arboria.records.decide")
}

/// The worktree of an idea, refusing a missing one (`idea worktree … is missing`).
pub fn worktree_of(idea: &Idea) -> Result<&Path> {
    let _ = idea;
    todo!("port of the worktree checks in arboria.records")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_decimal_digits() {
        assert_eq!(py_decimal('0'), Some(0));
        assert_eq!(py_decimal('9'), Some(9));
        assert_eq!(py_decimal('\u{661}'), Some(1));
        assert_eq!(py_decimal('\u{ff12}'), Some(2));
        assert_eq!(py_decimal('\u{1fbf9}'), Some(9));
        for c in [
            '/',
            ':',
            'a',
            '\u{65f}',
            '\u{66a}',
            '\u{b2}',
            '\u{2460}',
            '\u{4e00}',
            '\u{1fbfa}',
        ] {
            assert_eq!(py_decimal(c), None, "{c:?}");
        }
    }

    #[test]
    fn limits_are_pythons() {
        // db.LIMITS, verbatim
        let python = [
            ("hypothesis", 300),
            ("metric", 200),
            ("change_reason", 300),
            ("note", 1000),
            ("summary", 1500),
            ("lesson", 200),
            ("result", 600),
            ("todo", 1000),
            ("gist", 300),
            ("question", 300),
            ("inspect_title", 80),
            ("inspect_summary", 600),
            ("span_note", 600),
            ("line_note", 200),
            ("inspect_text", 8000),
            ("mermaid", 8000),
        ];
        assert_eq!(TextField::ALL.len(), python.len());
        for (f, (name, limit)) in TextField::ALL.iter().zip(python) {
            assert_eq!((f.name(), f.limit()), (name, limit));
        }
    }

    #[test]
    fn length_is_counted_in_code_points_after_strip() {
        let ok = "中".repeat(300);
        assert_eq!(
            validate_text(TextField::Hypothesis, Some(&ok), true)
                .unwrap()
                .unwrap(),
            ok
        );
        let padded = format!("  \n{ok}\u{3000}\u{1f} ");
        assert_eq!(
            validate_text(TextField::Hypothesis, Some(&padded), true)
                .unwrap()
                .unwrap(),
            ok
        );
        let long = "中".repeat(301);
        assert_eq!(
            validate_text(TextField::Hypothesis, Some(&long), true)
                .unwrap_err()
                .to_string(),
            "hypothesis is too long: 301 chars (max 300); shorten it"
        );
        // an emoji is one code point (Python len), not two UTF-16 units or four bytes
        assert!(validate_text(TextField::InspectTitle, Some(&"😀".repeat(80)), true).is_ok());
        assert!(validate_text(TextField::InspectTitle, Some(&"😀".repeat(81)), true).is_err());
    }

    #[test]
    fn required_and_optional() {
        assert_eq!(
            validate_text(TextField::Note, None, true)
                .unwrap_err()
                .to_string(),
            "note is required"
        );
        assert_eq!(
            validate_text(TextField::Note, Some(" \t\n"), true)
                .unwrap_err()
                .to_string(),
            "note must not be empty"
        );
        assert_eq!(validate_text(TextField::Result, None, false).unwrap(), None);
        assert_eq!(
            validate_text(TextField::Result, Some("   "), false).unwrap(),
            None
        );
        assert_eq!(require_text(TextField::Result, " ok ").unwrap(), "ok");
    }

    #[test]
    fn summary_lines_and_lesson_rules() {
        let ten = ["x"; 10].join("\n");
        assert!(validate_text(TextField::Summary, Some(&ten), true).is_ok());
        let eleven = ["x"; 11].join("\n");
        assert_eq!(
            validate_text(TextField::Summary, Some(&eleven), true)
                .unwrap_err()
                .to_string(),
            "summary has 11 lines (max 10); shorten it"
        );
        // trailing newlines are stripped first, as in Python
        assert!(validate_text(TextField::Summary, Some(&format!("{ten}\n\n\n")), true).is_ok());
        assert_eq!(
            validate_text(TextField::Lesson, Some("a\nb"), true)
                .unwrap_err()
                .to_string(),
            "lesson must be a single line; state one pitfall per lesson"
        );
        assert!(validate_text(TextField::Lesson, Some("a\rb"), true).is_err());
        assert_eq!(
            validate_text(TextField::Lesson, Some("x <!-- y"), true)
                .unwrap_err()
                .to_string(),
            "lesson must not contain HTML comment markers (<!-- or -->)"
        );
        assert!(validate_text(TextField::Lesson, Some("x --> y"), true).is_err());
        // other fields may contain newlines
        assert!(validate_text(TextField::Note, Some("a\nb"), true).is_ok());
    }

    #[test]
    fn python_strip_and_repr() {
        assert_eq!(py_strip(" \u{1c}\u{1d}ab\u{1e}\u{1f}\u{3000}"), "ab");
        assert_eq!(py_strip("\u{200b}a"), "\u{200b}a"); // zero-width space is not whitespace
        assert_eq!(py_repr("a'b"), "\"a'b\"");
        assert_eq!(py_repr("a\"b'"), "'a\"b\\''");
        assert_eq!(py_repr("x\ny\u{1}"), "'x\\ny\\x01'");
        assert_eq!(py_repr("中文"), "'中文'");
        assert_eq!(py_repr("a\u{85}\u{a0}b"), "'a\\x85\\xa0b'");
    }

    #[test]
    fn names() {
        assert!(validate_name("wide-lr", "seed").is_ok());
        assert_eq!(
            validate_name("Wide", "seed").unwrap_err().to_string(),
            "invalid seed name 'Wide': use lowercase letters, digits and '-' (must start with a letter or digit, max 63 chars)"
        );
    }
}
