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

use std::collections::BTreeMap;
use std::path::Path;

use arboria_core::clusters::Profile;
use arboria_core::model::{IdeaName, ProjectId, RunId};
use arboria_core::slurm::{self, Ask, OverrideValue};
use arboria_core::{backend, canonical, cards, clusters, config, db, model, records, Error};
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
    /// An error that is not an [`Error`] (e.g. shlex's `ValueError: …`), as `<class>: <message>`.
    ErrText(String),
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
            (Got::ErrText(e), None, Some(want)) if &e == want => {}
            (Got::ErrText(e), _, _) => failures.push(format!("{}: got error {e:?}", c.id)),
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
fn git_refs() {
    use arboria_core::{ideas, inspection, records};
    run("git_refs", |i| {
        let arg = s(i, "arg").unwrap();
        let out = match s(i, "kind").unwrap() {
            "run" => records::run_ref(&RunId::parse(arg).unwrap()),
            "inspect" => inspection::ref_name(&model::TaskId::parse(arg).unwrap()),
            "branch" => ideas::branch_for(&model::IdeaName::parse(arg).unwrap()),
            "worktree" => {
                let main = std::path::PathBuf::from(s(i, "main").unwrap());
                let ctx = arboria_core::paths::RepoContext {
                    cwd: main.clone(),
                    common_dir: main.join(".git"),
                    main,
                    toplevel: None,
                };
                ideas::worktree_for(&ctx, &model::IdeaName::parse(arg).unwrap())
                    .display()
                    .to_string()
            }
            other => panic!("unknown kind {other}"),
        };
        Got::Out(serde_json::Value::String(out))
    });
}

/// A runner that answers `ssh -O check` with success and every remote command with the
/// stdout given for its first word; it records the remote commands.
#[derive(Debug, Default)]
struct ScriptedRunner {
    replies: std::collections::BTreeMap<String, String>,
    commands: std::sync::Mutex<Vec<String>>,
}

impl arboria_core::backend::Runner for ScriptedRunner {
    fn run(
        &self,
        argv: &[String],
        _input: Option<&str>,
        _timeout: std::time::Duration,
    ) -> Result<arboria_core::backend::RemoteOutput, Error> {
        let ok = |stdout: &str| arboria_core::backend::RemoteOutput {
            code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        };
        if argv[1] == "-O" {
            return Ok(ok(""));
        }
        let cmd = argv.last().unwrap().clone();
        let word = cmd.split(' ').next().unwrap().to_string();
        self.commands.lock().unwrap().push(cmd);
        Ok(ok(self.replies.get(&word).map_or("", String::as_str)))
    }
    fn pipe(
        &self,
        _: &[String],
        _: &std::path::Path,
        _: &[String],
        _: std::time::Duration,
    ) -> Result<(i32, i32, String), Error> {
        unreachable!()
    }
    fn has(&self, _: &str) -> bool {
        false
    }
}

/// The real DeltaAI profile of the restrict_path vectors.
fn deltaai() -> Profile {
    let file = load("restrict_path").expect("restrict_path vectors");
    profile_of(&file.cases[0].input)
}

fn scripted(replies: &[(&str, &str)]) -> (std::sync::Arc<ScriptedRunner>, backend::SshSlurm) {
    let runner = std::sync::Arc::new(ScriptedRunner {
        replies: replies
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    });
    let slurm = backend::SshSlurm::new(
        deltaai(),
        ProjectId::parse("wavefront-3f2a").unwrap(),
        runner.clone(),
    );
    (runner, slurm)
}

#[test]
fn find_job() {
    use arboria_core::backend::Backend;
    run("find_job", |i| {
        let (runner, slurm) = scripted(&[
            ("squeue", s(i, "squeue").unwrap()),
            ("sacct", s(i, "sacct").unwrap()),
        ]);
        match slurm.find_job(s(i, "name").unwrap()) {
            Ok(id) => Got::Out(serde_json::json!({
                "job_id": id,
                "commands": *runner.commands.lock().unwrap(),
            })),
            Err(e) => Got::Err(e),
        }
    });
}

#[test]
fn sacct_poll() {
    use arboria_core::backend::Backend;
    run("sacct_poll", |i| {
        let mut profile = deltaai();
        profile.billing_per_su = i["billing_per_su"].as_f64().unwrap();
        let runner = std::sync::Arc::new(ScriptedRunner {
            replies: [("sacct".to_string(), s(i, "stdout").unwrap().to_string())].into(),
            ..Default::default()
        });
        let slurm = backend::SshSlurm::new(
            profile,
            ProjectId::parse("wavefront-3f2a").unwrap(),
            runner.clone(),
        );
        let ids: Vec<String> = serde_json::from_value(i["job_ids"].clone()).unwrap();
        match slurm.poll(&ids) {
            Ok(res) => {
                let result: serde_json::Map<String, Value> = res
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            serde_json::json!([v.state.as_str(), v.elapsed_s, v.billed_su]),
                        )
                    })
                    .collect();
                let cmds = runner.commands.lock().unwrap();
                Got::Out(serde_json::json!({"command": cmds.first(), "result": result}))
            }
            Err(e) => Got::Err(e),
        }
    });
}

#[test]
fn restrict_path() {
    run("restrict_path", |i| {
        match ProjectId::parse(s(i, "project").unwrap()) {
            Err(e) => Got::Err(e),
            Ok(p) => got(backend::restrict_path(
                &profile_of(i),
                &p,
                s(i, "path").unwrap(),
            )),
        }
    });
}

#[test]
fn su_from_billing() {
    run("su_from_billing", |i| {
        Got::Out(serde_json::json!(backend::su_from_billing(
            s(i, "alloc_tres").unwrap(),
            i["elapsed_s"].as_u64().unwrap(),
            i["billing_per_su"].as_f64().unwrap()
        )))
    });
}

#[test]
fn map_slurm_state() {
    run("map_slurm_state", |i| {
        Got::Out(
            backend::map_slurm_state(s(i, "raw").unwrap())
                .as_str()
                .into(),
        )
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

// ---------------------------------------------------------------- slurm

fn u64_of(i: &Value, key: &str) -> u64 {
    i[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key}: not a u64 in {i}"))
}

fn u32_of(i: &Value, key: &str) -> u32 {
    u32::try_from(u64_of(i, key)).unwrap()
}

/// `[[k, v], …]` pairs.
fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|kv| {
            (
                kv[0].as_str().unwrap().to_string(),
                kv[1].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pairs_json(v: Vec<(String, String)>) -> Value {
    json!(v
        .into_iter()
        .map(|(k, v)| json!([k, v]))
        .collect::<Vec<_>>())
}

/// The project id (Python's check_project error when invalid), or a skip for non-strings.
fn project_of(i: &Value) -> Result<Result<ProjectId, Error>, Got> {
    match s(i, "project") {
        Some(p) => Ok(ProjectId::parse(p)),
        None => Err(Got::Skip("non-string project")),
    }
}

/// The typed run id, or a skip: the Rust API takes a validated `RunId` where Python
/// took any string.
fn run_id_of(i: &Value) -> Result<RunId, Got> {
    s(i, "run_id")
        .and_then(|r| RunId::parse(r).ok())
        .ok_or(Got::Skip("run id is not a RunId"))
}

/// The normalized profile of a vector (its `source` is not deserialized).
fn profile_of(i: &Value) -> Profile {
    let raw = &i["profile"];
    let mut p: Profile = serde_json::from_value(raw.clone()).expect("profile");
    p.source = raw["source"].as_str().unwrap_or_default().to_string();
    assert_eq!(p.account_set(), raw["account_set"].as_bool().unwrap());
    p
}

/// `build_plan(profile, ask, project)` of a vector input.
fn plan_of(i: &Value) -> Result<Result<slurm::Plan, Error>, Got> {
    let ask: Ask =
        serde_json::from_value(i["ask"].clone()).map_err(|_| Got::Skip("ask not representable"))?;
    Ok(project_of(i)?.and_then(|p| slurm::build_plan(&profile_of(i), &ask, &p)))
}

macro_rules! try_got {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(skip) => return skip,
        }
    };
}

#[test]
fn parse_walltime() {
    run("parse_walltime", |i| match s(i, "text") {
        None => Got::Skip("non-string walltime"),
        Some(t) => got(slurm::parse_walltime(t)),
    });
}

#[test]
fn slurm_time() {
    run("slurm_time", |i| {
        Got::Out(json!(slurm::slurm_time(u64_of(i, "seconds"))))
    });
}

#[test]
fn human_duration() {
    run("human_duration", |i| {
        Got::Out(json!(slurm::human_duration(u64_of(i, "seconds"))))
    });
}

#[test]
fn segment_plan() {
    run("segment_plan", |i| {
        got(slurm::segment_plan(
            u64_of(i, "total_s"),
            u64_of(i, "max_s"),
            u32_of(i, "max_segments"),
        )
        .map(|v| json!(v)))
    });
}

#[test]
fn billed_gpus() {
    run("billed_gpus", |i| {
        Got::Out(json!(slurm::billed_gpus(
            u32_of(i, "gpus"),
            i["mem_gb"].as_f64().unwrap(),
            u32_of(i, "cpus"),
            i["mem_per_gpu_gb"].as_f64().unwrap(),
            u32_of(i, "cpus_per_gpu"),
        )))
    });
}

#[test]
fn check_override() {
    run("check_override", |i| {
        let Some(key) = s(i, "key") else {
            return Got::Skip("non-string key");
        };
        let value = match &i["value"] {
            Value::String(v) => OverrideValue::Str(v.clone()),
            Value::Bool(b) => OverrideValue::Bool(*b),
            Value::Null => OverrideValue::Null,
            Value::Number(n) if n.is_f64() => OverrideValue::Float(n.as_f64().unwrap()),
            Value::Number(n) => OverrideValue::Int(n.as_i64().unwrap()),
            _ => return Got::Skip("list / dict value"),
        };
        got(slurm::check_override(key, &value).map(|(k, v)| json!([k, v])))
    });
}

fn override_of(v: &Value) -> Option<OverrideValue> {
    Some(match v {
        Value::String(v) => OverrideValue::Str(v.clone()),
        Value::Bool(b) => OverrideValue::Bool(*b),
        Value::Null => OverrideValue::Null,
        Value::Number(n) if n.is_f64() => OverrideValue::Float(n.as_f64().unwrap()),
        Value::Number(n) => OverrideValue::Int(n.as_i64().unwrap()),
        _ => return None,
    })
}

#[test]
fn build_command() {
    run("build_command", |i| {
        let mut table = toml::Table::new();
        let mut run_t = toml::Table::new();
        let cmd = s(i, "config_command").unwrap();
        if !cmd.is_empty() {
            run_t.insert("command".into(), cmd.into());
        }
        run_t.insert(
            "override_style".into(),
            s(i, "override_style").unwrap().into(),
        );
        table.insert("run".into(), run_t.into());
        let cfg = config::parse_project_config(&table, Path::new("/repo/arboria.toml")).unwrap();
        let mut overrides = Vec::new();
        for kv in i["overrides"].as_array().unwrap() {
            let (Some(k), Some(v)) = (kv[0].as_str(), override_of(&kv[1])) else {
                return Got::Skip("non-string key or list value");
            };
            match slurm::check_override(k, &v) {
                Ok(pair) => overrides.push(pair),
                Err(e) => return Got::Err(e),
            }
        }
        let explicit: Option<Vec<String>> = serde_json::from_value(i["explicit"].clone()).unwrap();
        let parent = (!i["parent_request"].is_null()).then(|| i["parent_request"].clone());
        match arboria_core::submission::build_command(
            &cfg,
            explicit.as_deref(),
            &overrides,
            parent.as_ref(),
        ) {
            Ok(spec) => Got::Out(json!({
                "command": spec.command,
                "base": spec.base,
                "source": spec.source,
                "overrides": spec.overrides.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                "style": spec.style,
                "request_extra": spec.request_extra(),
            })),
            Err(e) => Got::Err(e),
        }
    });
}

#[test]
fn derive_status() {
    use arboria_core::model::{JobState, RunStatus};
    run("derive_status", |i| {
        let states: Vec<JobState> = serde_json::from_value(i["states"].clone()).unwrap();
        let current: RunStatus = serde_json::from_value(i["current"].clone()).unwrap();
        let n = i["n_segments"].as_u64().unwrap() as usize;
        Got::Out(
            arboria_core::monitor::derive_status(&states, n, current)
                .as_str()
                .into(),
        )
    });
}

#[test]
fn billed_su() {
    run("billed_su", |i| {
        Got::Out(json!(arboria_core::monitor::billed_su(
            &i["request"],
            i["elapsed"].as_u64(),
            i["reported"].as_f64()
        )))
    });
}

#[test]
fn first_hit() {
    run("first_hit", |i| {
        let patterns: Vec<String> = match &i["patterns"] {
            Value::Null => config::DEFAULT_FAIL_PATTERNS
                .iter()
                .map(|p| p.to_string())
                .collect(),
            v => serde_json::from_value(v.clone()).unwrap(),
        };
        match arboria_core::monitor::first_hit(s(i, "text").unwrap(), &patterns) {
            Ok(hit) => Got::Out(json!(hit.map(|(p, l)| json!([p, l])))),
            Err(e) => Got::Err(e),
        }
    });
}

#[test]
fn parse_override() {
    run("parse_override", |i| {
        got(slurm::parse_override(s(i, "text").unwrap()).map(|(k, v)| json!([k, v])))
    });
}

#[test]
fn merge_overrides() {
    run("merge_overrides", |i| {
        Got::Out(pairs_json(slurm::merge_overrides(
            &pairs(&i["base"]),
            &pairs(&i["new"]),
        )))
    });
}

#[test]
fn render_overrides() {
    run("render_overrides", |i| {
        got(
            slurm::render_overrides(s(i, "style").unwrap(), &pairs(&i["overrides"]))
                .map(|v| json!(v)),
        )
    });
}

#[test]
fn job_name() {
    run("job_name", |i| {
        let project = try_got!(project_of(i));
        let run_id = match project {
            Err(e) => return Got::Err(e),
            Ok(_) => try_got!(run_id_of(i)),
        };
        got(slurm::job_name(
            &project.unwrap(),
            &run_id,
            u32_of(i, "segment"),
        ))
    });
}

#[test]
fn parse_job_name() {
    run("parse_job_name", |i| match s(i, "name") {
        None => Got::Skip("non-string name"),
        Some(n) => got(slurm::parse_job_name(n).map(|(p, r, k)| json!([p, r, k]))),
    });
}

#[test]
fn job_project() {
    run("job_project", |i| match s(i, "name") {
        None => Got::Skip("non-string name"),
        Some(n) => Got::Out(json!(slurm::job_project(n))),
    });
}

/// The `<role>/<project>/<run>…` path functions share one input shape.
fn path_vector(
    name: &str,
    f: impl Fn(&BTreeMap<String, String>, &ProjectId, &RunId, &Value) -> String,
) {
    run(name, |i| {
        let paths: BTreeMap<String, String> = serde_json::from_value(i["paths"].clone()).unwrap();
        let project = match try_got!(project_of(i)) {
            Ok(p) => p,
            Err(e) => return Got::Err(e),
        };
        let run_id = try_got!(run_id_of(i));
        Got::Out(json!(f(&paths, &project, &run_id, i)))
    });
}

#[test]
fn run_dir() {
    path_vector("run_dir", |p, pr, r, _| slurm::run_dir(p, pr, r));
}

#[test]
fn code_dir() {
    path_vector("code_dir", |p, pr, r, _| slurm::code_dir(p, pr, r));
}

#[test]
fn out_dir() {
    path_vector("out_dir", |p, pr, r, _| slurm::out_dir(p, pr, r));
}

#[test]
fn ckpt_dir() {
    path_vector("ckpt_dir", |p, pr, r, _| slurm::ckpt_dir(p, pr, r));
}

#[test]
fn script_path() {
    path_vector("script_path", |p, pr, r, i| {
        slurm::script_path(p, pr, r, u32_of(i, "segment"))
    });
}

#[test]
fn log_pattern() {
    path_vector("log_pattern", |p, pr, r, i| {
        slurm::log_pattern(p, pr, r, u32_of(i, "segment"))
    });
}

#[test]
fn log_path() {
    path_vector("log_path", |p, pr, r, i| {
        slurm::log_path(p, pr, r, u32_of(i, "segment"), s(i, "job_id").unwrap())
    });
}

#[test]
fn sbatch_value() {
    run("sbatch_value", |i| {
        Got::Out(json!(slurm::sbatch_value(s(i, "path").unwrap())))
    });
}

#[test]
fn dq() {
    run("dq", |i| got(slurm::dq(s(i, "path").unwrap())));
}

#[test]
fn shell_quote() {
    run("shell_quote", |i| {
        let argv: Vec<String> = serde_json::from_value(i["argv"].clone()).unwrap();
        let quoted: Vec<String> = argv.iter().map(|a| slurm::shlex_quote(a)).collect();
        Got::Out(json!({"join": slurm::shlex_join(&argv), "quoted": quoted}))
    });
}

#[test]
fn build_plan() {
    let n = run("build_plan", |i| {
        got(try_got!(plan_of(i)).map(|plan| {
            let mut out = plan.request();
            out["billed_per_node"] = json!(plan.billed_per_node);
            out["max_walltime_s"] = json!(plan.max_walltime_s);
            out["over_billed"] = json!(plan.over_billed());
            // `Plan.request()` is also written with ledger_json: the floats must stay floats
            assert!(canonical::ledger_json(&out).contains("\"charge\": "));
            out
        }))
    });
    if n > 0 {
        assert!(n >= 40, "build_plan has too few cases: {n}");
    }
}

#[test]
fn card_payload() {
    run("card_payload", |i| {
        got(try_got!(plan_of(i))
            .map(|plan| cards::card_payload(s(i, "snapshot_commit").unwrap(), &plan)))
    });
}

#[test]
fn card_hash() {
    let n = run("card_hash", |i| {
        got(try_got!(plan_of(i))
            .map(|plan| cards::card_hash(s(i, "snapshot_commit").unwrap(), &plan)))
    });
    if n > 0 {
        assert!(n >= 20, "card_hash has too few cases: {n}");
    }
}

#[test]
fn render_sbatch() {
    run("render_sbatch", |i| {
        let plan = match try_got!(plan_of(i)) {
            Ok(p) => p,
            Err(e) => return Got::Err(e),
        };
        let run_id = try_got!(run_id_of(i));
        let idea = IdeaName::parse(s(i, "idea").unwrap()).unwrap();
        got(slurm::render_sbatch(
            &plan,
            &run_id,
            &idea,
            u32_of(i, "segment"),
        ))
    });
}

// ---------------------------------------------------------------- config / clusters

/// A TOML value from a vector's JSON input (ints stay ints, floats floats).
fn toml_of(v: &Value) -> Option<toml::Value> {
    Some(match v {
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) if n.is_f64() => toml::Value::Float(n.as_f64()?),
        Value::Number(n) => toml::Value::Integer(n.as_i64()?),
        Value::Array(a) => toml::Value::Array(a.iter().map(toml_of).collect::<Option<_>>()?),
        Value::Object(o) => toml::Value::Table(
            o.iter()
                .map(|(k, v)| Some((k.clone(), toml_of(v)?)))
                .collect::<Option<_>>()?,
        ),
        Value::Null => return None,
    })
}

#[test]
fn shell_split() {
    run("shell_split", |i| {
        match slurm::shlex_split(s(i, "text").unwrap()) {
            Ok(v) => Got::Out(json!(v)),
            Err(e) => Got::ErrText(format!("ValueError: {e}")),
        }
    });
}

#[test]
fn validate_project_id() {
    run("validate_project_id", |i| {
        let where_ = s(i, "where").unwrap();
        match toml_of(&i["value"]) {
            Some(v) => got(config::validate_project_id(&v, where_).map(String::from)),
            // Python repr(None); TOML has no null, the command line passes strings
            None => Got::Err(Error::arb(format!(
                "{where_} must match ^[a-z0-9][a-z0-9-]{{0,30}}$ (lowercase letters, digits, '-', at most 31 chars); got None"
            ))),
        }
    });
}

#[test]
fn derive_project_id() {
    run("derive_project_id", |i| {
        Got::Out(json!(config::project_id_for(Path::new(
            s(i, "main").unwrap()
        ))))
    });
}

#[test]
fn add_project_id() {
    run("add_project_id", |i| {
        got(
            config::validate_project_id_str(s(i, "project_id").unwrap(), "project id")
                .and_then(|id| config::add_project_id(s(i, "text").unwrap(), &id)),
        )
    });
}

#[test]
fn arboria_toml() {
    run("arboria_toml", |i| {
        let cfg = match s(i, "toml") {
            None => toml::Table::new(),
            Some(text) => text.parse().expect("vectors leave invalid TOML out"),
        };
        got(
            config::parse_project_config(&cfg, Path::new("/repo/arboria.toml")).map(|c| {
                let pid = c.project_id.as_ref().map_or("p", ProjectId::as_str);
                json!({
                    "claude": {"push": c.claude.push},
                    "default_machine": c.default_machine,
                    "project_id": c.project_id,
                    "run": {"command": c.run.command, "override_style": c.run.override_style},
                    "total_su": c.total_su,
                    "wandb": {
                        "configured": c.wandb.configured(),
                        "entity": c.wandb.entity,
                        "project": c.wandb.project,
                        "run_url_r001": c.wandb.run_url(&format!("{pid}-r001")),
                    },
                    "watch": {
                        "fail_patterns": c.watch.fail_patterns,
                        "observation_minutes": c.watch.observation_minutes,
                    },
                })
            }),
        )
    });
}

#[test]
fn check_remote_path() {
    run("check_remote_path", |i| {
        // a missing / null value is Python None: "must be a non-empty string"
        let v = toml_of(&i["value"]).unwrap_or(toml::Value::Integer(0));
        got(clusters::check_remote_path(&v, s(i, "where").unwrap()))
    });
}

#[test]
fn clusters_parse_profile() {
    run("clusters_parse_profile", |i| {
        let all: toml::Table = s(i, "toml").unwrap().parse().unwrap();
        let name = s(i, "name").unwrap();
        let raw = all.get(name).expect("the case's table");
        got(
            clusters::parse_profile(name, raw, s(i, "source").unwrap()).map(|p| {
                let mut v = serde_json::to_value(&p).unwrap();
                v["account_set"] = json!(p.account_set());
                v["source"] = json!(p.source);
                // the TOML order of the partitions (the map itself is sorted)
                let order: Vec<&str> = raw["partitions"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                v["partition_order"] = json!(order);
                v
            }),
        )
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
            "clusters.KINDS" => json!(clusters::KINDS),
            // Python sets, exported sorted
            "clusters.TOP_KEYS" => {
                let mut k = clusters::TOP_KEYS.to_vec();
                k.sort_unstable();
                json!(k)
            }
            "clusters.PARTITION_KEYS" => {
                let mut k = clusters::PARTITION_KEYS.to_vec();
                k.sort_unstable();
                json!(k)
            }
            "config.PROJECT_COMMENT" => json!(config::PROJECT_COMMENT),
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

#[test]
fn command_name() {
    run("command_name", |i| match s(i, "name") {
        None => Got::Skip("non-string name"),
        Some(n) => got(arboria_core::commands::validate_name(n, false).map(|()| n.to_string())),
    });
}

#[test]
fn check_mermaid() {
    run("check_mermaid", |i| match s(i, "src") {
        None => Got::Skip("non-string mermaid"),
        Some(src) => got(arboria_core::inspection::check_mermaid(src)),
    });
}

#[test]
fn inspect_check_path() {
    run("inspect_check_path", |i| {
        got(arboria_core::inspection::check_path(&i["path"]))
    });
}

#[test]
fn validate_question() {
    run("validate_question", |i| match s(i, "question") {
        None => Got::Skip("non-string question"),
        Some(q) => got(arboria_core::inspection::validate_question(q)),
    });
}

/// A bare repo holding `files` as one tree (blobs, mode 100644); returns its dir and the
/// tree id.
fn tree_repo(files: &serde_json::Map<String, Value>) -> (common::TempDir, String) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let dir = common::TempDir::new("vector-tree");
    let git = |args: &[&str], input: Option<&[u8]>| -> String {
        let mut child = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_INDEX_FILE", dir.path().join("index"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.unwrap_or_default())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "--bare"], None);
    for (path, content) in files {
        let sha = git(
            &["hash-object", "-w", "--stdin"],
            Some(content.as_str().unwrap().as_bytes()),
        );
        git(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{sha},{path}"),
            ],
            None,
        );
    }
    let tree = git(&["write-tree"], None);
    (dir, tree)
}

#[test]
fn inspect_validate_result() {
    let repos: std::cell::RefCell<BTreeMap<String, common::TempDir>> = Default::default();
    run("inspect_validate_result", |i| {
        let code_ref = s(i, "code_ref").unwrap().to_string();
        let mut repos = repos.borrow_mut();
        if !repos.contains_key(&code_ref) {
            let (dir, tree) = tree_repo(i["files"].as_object().unwrap());
            assert_eq!(tree, code_ref, "the vector's tree id");
            repos.insert(code_ref.clone(), dir);
        }
        let task = &i["task"];
        got(arboria_core::inspection::validate_answer(
            repos[&code_ref].path(),
            task["id"].as_str().unwrap(),
            task["kind"].as_str().unwrap(),
            &task["payload"],
            &i["answer"],
        ))
    });
}

#[test]
fn parse_accounts() {
    run("parse_accounts", |i| {
        let rows = arboria_core::machine::parse_accounts(s(i, "text").unwrap());
        Got::Out(json!(rows))
    });
}

#[test]
fn parse_weights() {
    run("parse_weights", |i| {
        Got::Out(json!(arboria_core::machine::parse_weights(
            s(i, "text").unwrap()
        )))
    });
}

#[test]
fn machine_propose() {
    use arboria_core::machine;
    run("machine_propose", |i| {
        let sections = machine::split_sections(s(i, "probe").unwrap());
        let r = machine::propose(
            s(i, "name").unwrap(),
            s(i, "host").unwrap(),
            &sections,
            s(i, "account"),
            i["existing"].as_object(),
        );
        match r {
            Err(e) => Got::Err(e),
            Ok(p) => {
                let toml =
                    machine::render_profile(s(i, "name").unwrap(), &p, s(i, "note").unwrap());
                Got::Out(json!({"values": p.values, "warnings": p.warnings,
                                "other_partitions": p.other_partitions, "accounts": p.accounts,
                                "tools": p.tools, "toml": toml}))
            }
        }
    });
}

// ---------------------------------------------------------------- not wired yet

/// Wired above.
const WIRED: &[&str] = &[
    "parse_accounts",
    "parse_weights",
    "machine_propose",
    "check_mermaid",
    "inspect_check_path",
    "validate_question",
    "inspect_validate_result",
    "command_name",
    "canonical_json",
    "validate_text",
    "validate_name",
    "check_project",
    "wandb_id",
    "next_id",
    "git_refs",
    "find_job",
    "map_slurm_state",
    "restrict_path",
    "sacct_poll",
    "su_from_billing",
    "billed_su",
    "build_command",
    "derive_status",
    "first_hit",
    "check_job_ids",
    "constants",
    "billed_gpus",
    "build_plan",
    "card_hash",
    "card_payload",
    "check_override",
    "ckpt_dir",
    "code_dir",
    "dq",
    "human_duration",
    "job_name",
    "job_project",
    "log_path",
    "log_pattern",
    "merge_overrides",
    "out_dir",
    "parse_job_name",
    "parse_override",
    "parse_walltime",
    "render_overrides",
    "render_sbatch",
    "run_dir",
    "sbatch_value",
    "script_path",
    "segment_plan",
    "shell_quote",
    "slurm_time",
    "add_project_id",
    "arboria_toml",
    "check_remote_path",
    "clusters_parse_profile",
    "derive_project_id",
    "shell_split",
    "validate_project_id",
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

/// Vector files whose Rust function is not ported yet (none left: every file is wired).
const PENDING: &[&str] = &[];
