//! Cross-language test vectors (`tests/vectors/<name>.json`, exported from the Python
//! implementation by `scripts/export_vectors.py`; format in `tests/vectors/README.md`).
//!
//! A vector file is wired here when the Rust function it covers is real. The others are
//! `#[ignore]`d tests named after their file: whoever ports the function removes the
//! `ignore` and wires it (see `docs/adr/0002-rust-contract.md`). `every_vector_file_is_known`
//! fails when a new file appears that is neither.
//!
//! Without the directory (e.g. a checkout of an older main) every test passes with a
//! message instead.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use arboria_core::model::{ProjectId, RunId};
use arboria_core::{canonical, db, model, records, Error};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct VectorFile {
    name: String,
    version: u32,
    source: String,
    #[allow(dead_code)]
    notes: String,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    input: Value,
    /// Present (even `null`) or absent: not `Option`'s usual null-means-absent.
    #[serde(default, deserialize_with = "present")]
    output: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

fn vectors_dir() -> PathBuf {
    common::repo_root().join("tests/vectors")
}

/// The vector file, or None (with a message) when it is not there.
fn load(name: &str) -> Option<VectorFile> {
    let path = vectors_dir().join(format!("{name}.json"));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            eprintln!(
                "skipped: {} is absent (vectors not exported on this checkout)",
                path.display()
            );
            return None;
        }
    };
    let file: VectorFile =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(file.name, name, "{}: name", path.display());
    assert_eq!(
        file.version,
        1,
        "{}: unknown vector format version",
        path.display()
    );
    for c in &file.cases {
        assert!(
            c.output.is_some() != c.error.is_some(),
            "{name}/{}: exactly one of output / error",
            c.id
        );
        assert_eq!(
            c.id.starts_with("err-"),
            c.error.is_some(),
            "{name}/{}: err- ids are errors",
            c.id
        );
    }
    Some(file)
}

/// What a case evaluates to: an output, an error, or "not representable in the typed
/// Rust API" (e.g. a `None` where Rust takes `&str`), which is skipped and counted.
enum Got {
    Out(Value),
    Err(Error),
    Skip(&'static str),
}

fn got<T: Into<Value>>(r: Result<T, Error>) -> Got {
    match r {
        Ok(v) => Got::Out(v.into()),
        Err(e) => Got::Err(e),
    }
}

/// Run every case of `name` through `f`; returns how many were checked.
fn run(name: &str, f: impl Fn(&Value) -> Got) -> usize {
    let Some(file) = load(name) else { return 0 };
    let mut failures = Vec::new();
    let mut skipped = Vec::new();
    for c in &file.cases {
        match (f(&c.input), &c.output, &c.error) {
            (Got::Skip(why), _, _) => skipped.push(format!("{} ({why})", c.id)),
            (Got::Out(v), Some(want), None) if &v == want => {}
            (Got::Err(e), None, Some(want)) if &format!("{}: {e}", e.python_class()) == want => {}
            (Got::Out(v), _, _) => failures.push(format!(
                "{}: got output {v}, want {}",
                c.id,
                c.error.clone().map_or_else(
                    || c.output.clone().unwrap().to_string(),
                    |e| format!("error {e:?}")
                )
            )),
            (Got::Err(e), _, _) => failures.push(format!(
                "{}: got error {:?}, want {}",
                c.id,
                format!("{}: {e}", e.python_class()),
                c.error.clone().map_or_else(
                    || format!("output {}", c.output.clone().unwrap()),
                    |e| format!("error {e:?}")
                )
            )),
        }
    }
    if !skipped.is_empty() {
        eprintln!(
            "{name}: skipped {} case(s): {}",
            skipped.len(),
            skipped.join(", ")
        );
    }
    assert!(
        failures.is_empty(),
        "{name} ({}):\n  {}",
        file.source,
        failures.join("\n  ")
    );
    file.cases.len() - skipped.len()
}

fn s<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

// ---------------------------------------------------------------- wired

#[test]
fn canonical_json() {
    let n = run("canonical_json", |i| {
        Got::Out(Value::String(canonical::canonical_json(&i["obj"])))
    });
    if n > 0 {
        assert!(n >= 30, "canonical_json has too few cases: {n}");
    }
}

#[test]
fn validate_text() {
    run("validate_text", |i| {
        let field = s(i, "field").unwrap();
        let field = *records::TextField::ALL
            .iter()
            .find(|f| f.name() == field)
            .expect("known field");
        let required = i["required"].as_bool().unwrap();
        got(records::validate_text(field, s(i, "value"), required)
            .map(|v| v.map_or(Value::Null, Value::String)))
    });
}

#[test]
fn validate_name() {
    run("validate_name", |i| match s(i, "name") {
        None => Got::Skip("non-string name"),
        Some(name) => {
            got(records::validate_name(name, s(i, "what").unwrap()).map(|()| name.to_string()))
        }
    });
}

#[test]
fn check_project() {
    run("check_project", |i| match s(i, "project") {
        None => Got::Skip("non-string project"),
        Some(p) => got(ProjectId::parse(p).map(String::from)),
    });
}

#[test]
fn wandb_id() {
    run("wandb_id", |i| match s(i, "project") {
        None => Got::Skip("non-string project"),
        Some(p) => {
            got(ProjectId::parse(p)
                .map(|p| arboria_core::slurm::wandb_id(&p, s(i, "run_id").unwrap())))
        }
    });
}

#[test]
fn next_id() {
    run("next_id", |i| {
        let prefix = s(i, "prefix").unwrap();
        let existing: Vec<&str> = i["existing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let n = db::next_number(existing);
        let id = match prefix {
            "r" => RunId::from_number(n).to_string(),
            "d" => model::DecisionId::from_number(n).to_string(),
            "t" => model::TaskId::from_number(n).to_string(),
            "l" => model::LessonId::from_number(n).to_string(),
            _ => return Got::Skip("unknown prefix"),
        };
        // the table each prefix lives in is the SeqId's
        let table = match prefix {
            "r" => <RunId as db::SeqId>::TABLE,
            "d" => <model::DecisionId as db::SeqId>::TABLE,
            "t" => <model::TaskId as db::SeqId>::TABLE,
            _ => <model::LessonId as db::SeqId>::TABLE,
        };
        assert_eq!(table, s(i, "table").unwrap());
        Got::Out(Value::String(id))
    });
}

#[test]
fn check_job_ids() {
    run("check_job_ids", |i| {
        // Python stringifies each id first (`str(j)`); for ints that is their JSON text
        let ids: Option<Vec<String>> = i["job_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
                _ => None,
            })
            .collect();
        match ids {
            None => Got::Skip("non-string job id"),
            Some(ids) => got(arboria_core::backend::check_job_ids(&ids).map(|v| {
                Value::Array(
                    v.into_iter()
                        .map(|j| Value::String(j.to_string()))
                        .collect(),
                )
            })),
        }
    });
}

/// The Python constants the Rust core already defines. Python-only ones (and those of
/// modules not ported yet) are listed in `UNCHECKED` so a new constant gets noticed.
#[test]
fn constants() {
    use arboria_core::{commands, config, inspection, monitor, schema, slurm, submission};
    use model::*;
    fn names<T: Copy>(all: &[T], f: impl Fn(T) -> &'static str) -> Value {
        json!(all.iter().map(|v| f(*v)).collect::<Vec<_>>())
    }
    const UNCHECKED: &[&str] = &[
        // regex sources: the Rust side implements them by hand (model, slurm)
        "db.NAME_RE",
        "slurm.JOB_NAME_RE",
        "slurm.PROJECT_RE",
        "slurm._OVERRIDE_KEY_RE",
        "commands.NAME_RE",
        "backends.base.JOB_ID_RE",
        // not ported yet: wire them with their module
        "clusters.KINDS",
        "clusters.TOP_KEYS",
        "clusters.PARTITION_KEYS",
        "config.PROJECT_COMMENT",
        "backends.ssh.FIND_WINDOW",
        "backends.base._SLURM_STATES",
    ];
    run("constants", |i| {
        let name = s(i, "name").unwrap();
        let v = match name {
            "db.SCHEMA_VERSION" => json!(schema::PYTHON_SCHEMA_VERSION),
            "db.LIMITS" => Value::Object(
                records::TextField::ALL
                    .iter()
                    .map(|f| (f.name().to_string(), json!(f.limit())))
                    .collect(),
            ),
            "db.SUMMARY_MAX_LINES" => json!(records::SUMMARY_MAX_LINES),
            "db.CONFIDENCE" => names(Confidence::ALL, Confidence::as_str),
            "db.IDEA_STATUS" => names(IdeaStatus::ALL, IdeaStatus::as_str),
            "db.RUN_STATUS" => names(RunStatus::ALL, RunStatus::as_str),
            "db.RUN_ACTIVE" => names(RunStatus::ACTIVE, RunStatus::as_str),
            "db.RUN_FINAL" => names(RunStatus::FINAL, RunStatus::as_str),
            "db.RUN_NOT_REUSABLE" => names(RunStatus::NOT_REUSABLE, RunStatus::as_str),
            "db.JOB_STATES" => names(JobState::ALL, JobState::as_str),
            "db.JOB_TERMINAL" => names(JobState::TERMINAL, JobState::as_str),
            // v8 appends to these Python tuples; the Python prefix must be unchanged
            "db.SUBJECT_TYPES" => names(&SubjectType::ALL[..3], SubjectType::as_str),
            "db.TASK_KINDS" => names(&TaskKind::ALL[..4], TaskKind::as_str),
            "db.TASK_STATUS" => json!(["open", "done", "dropped"]),
            "db.LESSON_STATUS" => names(LessonStatus::ALL, LessonStatus::as_str),
            "db.LESSON_SOURCES" => names(LessonSource::ALL, LessonSource::as_str),
            "db.INSPECT_VIEWS" => names(InspectView::ALL, InspectView::as_str),
            "db.SEED_STATUS" => names(SeedStatus::ALL, SeedStatus::as_str),
            "db.BACKUP_EVENTS" => {
                let v8 = [
                    EventKind::TodoQueue,
                    EventKind::TodoNeedsHuman,
                    EventKind::ProbeEnable,
                    EventKind::ProbeDisable,
                ];
                let mut v: Vec<&str> = EventKind::BACKUP
                    .iter()
                    .filter(|k| !v8.contains(k))
                    .map(|k| k.as_str())
                    .collect();
                v.sort();
                json!(v)
            }
            "event_kinds" => {
                // every Python kind must exist here, spelled the same
                let ours: BTreeSet<&str> = EventKind::ALL.iter().map(|k| k.as_str()).collect();
                let want = load("constants")
                    .unwrap()
                    .cases
                    .into_iter()
                    .find(|c| c.id == "event_kinds")
                    .unwrap();
                let missing: Vec<String> = want
                    .output
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|k| k.as_str().unwrap().to_string())
                    .filter(|k| !ours.contains(k.as_str()))
                    .collect();
                assert!(
                    missing.is_empty(),
                    "event kinds missing in model::EventKind: {missing:?}"
                );
                return Got::Skip("checked as a subset");
            }
            "slurm.MIN_WALLTIME_S" => json!(slurm::MIN_WALLTIME_S),
            "slurm.MAX_JOB_NAME" => json!(slurm::MAX_JOB_NAME),
            "clusters.PATH_ROLES" => json!(arboria_core::clusters::PATH_ROLES),
            "config.OVERRIDE_STYLES" => json!(config::OVERRIDE_STYLES),
            "config.DEFAULT_OVERRIDE_STYLE" => json!(config::DEFAULT_OVERRIDE_STYLE),
            "config.DEFAULT_OBSERVATION_MINUTES" => json!(config::DEFAULT_OBSERVATION_MINUTES),
            "config.DEFAULT_FAIL_PATTERNS" => json!(config::DEFAULT_FAIL_PATTERNS),
            "commands.RESERVED_KWARGS" => json!(commands::RESERVED_KWARGS),
            "inspection.MAX_SPANS" => json!(inspection::MAX_SPANS),
            "inspection.MAX_LINES" => json!(inspection::MAX_LINES),
            "inspection.MAX_LINE_NOTES" => json!(inspection::MAX_LINE_NOTES),
            "inspection.MAX_NODES" => json!(inspection::MAX_NODES),
            "inspection.MAX_FILE_BYTES" => json!(inspection::MAX_FILE_BYTES),
            "inspection.MERMAID_KINDS" => json!(inspection::MERMAID_KINDS),
            "inspection.REF_PREFIX" => json!(inspection::REF_PREFIX),
            "submission.QUESTION" => json!(submission::QUESTION),
            "submission.OPTIONS" => json!(submission::OPTIONS),
            "monitor.HIT_LINE_MAX" => json!(monitor::HIT_LINE_MAX),
            "monitor.TAIL_LINES" => json!(monitor::TAIL_LINES),
            "backends.ssh.SSH_OPTS" => json!(arboria_core::backend::SSH_OPTS),
            other if UNCHECKED.contains(&other) => return Got::Skip("not in the Rust core yet"),
            other => {
                panic!("constants: new constant {other:?}: check it here or list it in UNCHECKED")
            }
        };
        Got::Out(v)
    });
}

// ---------------------------------------------------------------- not wired yet

/// Wired above.
const WIRED: &[&str] = &[
    "canonical_json",
    "validate_text",
    "validate_name",
    "check_project",
    "wandb_id",
    "next_id",
    "check_job_ids",
    "constants",
];

#[test]
fn every_vector_file_is_known() {
    let Ok(entries) = std::fs::read_dir(vectors_dir()) else {
        eprintln!("skipped: no tests/vectors/ on this checkout");
        return;
    };
    let on_disk: BTreeSet<String> = entries
        .filter_map(|e| {
            e.ok()?
                .file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(String::from)
        })
        .collect();
    let known: BTreeSet<String> = PENDING.iter().chain(WIRED).map(|s| s.to_string()).collect();
    let new: Vec<_> = on_disk.difference(&known).collect();
    let gone: Vec<_> = known.difference(&on_disk).collect();
    assert!(
        new.is_empty(),
        "new vector files: wire them or add them to PENDING: {new:?}"
    );
    assert!(
        gone.is_empty(),
        "vector files listed here but absent: {gone:?}"
    );
}

/// Vector files whose Rust function is still `todo!()`: one ignored test each, and the
/// `PENDING` list.
macro_rules! pending {
    ($($name:ident),+ $(,)?) => {
        const PENDING: &[&str] = &[$(stringify!($name)),+];

        mod pending {
            use super::load;
            $(
                #[test]
                #[ignore = concat!("tests/vectors/", stringify!($name), ".json: Rust port pending")]
                fn $name() {
                    let file = load(stringify!($name)).expect("vector file");
                    panic!("{} ({} cases): wire it here when {} is ported", file.name, file.cases.len(), file.source);
                }
            )+
        }
    };
}

pending!(
    add_project_id,
    arboria_toml,
    billed_gpus,
    billed_su,
    build_command,
    build_plan,
    card_hash,
    card_payload,
    check_mermaid,
    check_override,
    check_remote_path,
    ckpt_dir,
    clusters_parse_profile,
    code_dir,
    command_name,
    derive_project_id,
    derive_status,
    dq,
    find_job,
    first_hit,
    git_refs,
    human_duration,
    inspect_check_path,
    inspect_validate_result,
    job_name,
    job_project,
    log_path,
    log_pattern,
    machine_propose,
    map_slurm_state,
    merge_overrides,
    out_dir,
    parse_accounts,
    parse_job_name,
    parse_override,
    parse_walltime,
    parse_weights,
    render_overrides,
    render_sbatch,
    restrict_path,
    run_dir,
    sacct_poll,
    sbatch_value,
    script_path,
    segment_plan,
    shell_quote,
    shell_split,
    slurm_time,
    su_from_billing,
    validate_project_id,
    validate_question,
);
