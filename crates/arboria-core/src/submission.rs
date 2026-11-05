//! `arb submit` / `arb approve` / `arb reject`: the human approval gate in front of every
//! machine. Ports `arboria.submission`.
//!
//! Invariants:
//! - [`submit`] is purely local: snapshot (or the `--from` run's), request, card, an open
//!   Decision. It never builds a backend. An identical usable run (same card hash in the
//!   same idea) is returned with `existed = true` and nothing is written, unless
//!   `--repeat` (a random nonce goes into the request and so into the card hash).
//! - [`approve`] is the only path to [`crate::backend::Backend::submit`]: the backend
//!   method needs a [`SubmitPermit`], whose fields are private to this module, and the
//!   only constructor (`guarded_permit`) re-reads the ledger for the run's human
//!   `approve` Decision right before every submit (Python `_guarded_submit`). There is no
//!   automatic approval anywhere.
//! - The approval binds to the card hash: a changed request or profile is refused
//!   ([`crate::cards::CARD_CHANGED`]). Approving happens only in the human's terminal
//!   ([`crate::human::require_human`]) or, later, the app's Touch ID panel.
//! - Resumable: segments already submitted (job id recorded) are never submitted again;
//!   an existing job with the same name is adopted. A per-run lock
//!   (`.arboria/locks/<run>.lock`) is held across remote calls; the ledger lock never is.
//! - Runs of another project id, or from before project ids, are refused.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};

use crate::backend::Backend;
use crate::cards::{self, Card, CARD_CHANGED, LEGACY_RUN};
use crate::db::{self, now, Ledger};
use crate::model::{
    Decision, DecisionId, EventKind, Idea, Job, ProjectId, Run, RunId, RunStatus, SubjectType,
};
use crate::paths::RepoContext;
use crate::records::{self, require_text, validate_text, SnapshotOptions, TextField};
use crate::slurm::{self, Ask, OverrideValue, Plan};
use crate::{bail, clusters, config, gitutil, human, Error, Result, Warn};

/// The approval Decision's question.
pub const QUESTION: &str = "批准这个 run 上集群吗？";
/// Its options.
pub const OPTIONS: &[&str] = &["approve", "reject", "revise"];
/// `arb submit` returned an existing run.
pub const EXISTS: &str = "已存在：{run}";
/// No command after `--` and none in `arboria.toml`.
pub const NO_COMMAND: &str =
    "no command: put it after `--` (arb submit ... -- python train.py) or set [run] command in arboria.toml";

/// Proof that the ledger holds a human approval of this run, checked just now. Only this
/// module can make one; [`Backend::submit`] requires it.
#[derive(Debug)]
pub struct SubmitPermit<'a> {
    run_id: &'a RunId,
    decision_id: DecisionId,
    card_hash: String,
}

impl SubmitPermit<'_> {
    /// The approved run.
    pub fn run_id(&self) -> &RunId {
        self.run_id
    }
    /// The human's approve Decision.
    pub fn decision_id(&self) -> &DecisionId {
        &self.decision_id
    }
    /// The card hash that Decision approved.
    pub fn card_hash(&self) -> &str {
        &self.card_hash
    }
}

/// Re-read the ledger: the run is `approved` and has a human `approve` Decision. The only
/// constructor of [`SubmitPermit`] (Python `_guarded_submit`'s check).
fn guarded_permit<'a>(
    ledger: &Ledger,
    run_id: &'a RunId,
    job_name: &str,
) -> Result<SubmitPermit<'a>> {
    let run = records::get_run(ledger, run_id).ok();
    let decision = approval_decision(ledger, run_id)?;
    match (run, decision) {
        (Some(run), Some(dec)) if run.status == RunStatus::Approved => Ok(SubmitPermit {
            run_id,
            decision_id: dec.id,
            card_hash: run.card_hash.unwrap_or_default(),
        }),
        _ => Err(Error::Arb(format!(
            "refusing to submit {job_name}: run {run_id} has no human approval Decision in the ledger"
        ))),
    }
}

/// A per-run lock (`.arboria/locks/<run>.lock`) held across remote calls; the ledger lock
/// never is. Released on drop.
#[derive(Debug)]
pub struct RunLock(std::fs::File);

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Take the run's lock without waiting (`another arb process is working on r001; …`).
pub fn run_lock(ctx: &RepoContext, run_id: &RunId) -> Result<RunLock> {
    let dir = ctx.state_dir().join("locks");
    std::fs::create_dir_all(&dir)?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(dir.join(format!("{run_id}.lock")))?;
    match f.try_lock() {
        Ok(()) => Ok(RunLock(f)),
        Err(std::fs::TryLockError::WouldBlock) => Err(Error::Arb(format!(
            "another arb process is working on {run_id}; try again shortly"
        ))),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The run's latest human `approve` Decision.
pub fn approval_decision(ledger: &Ledger, run_id: &RunId) -> Result<Option<Decision>> {
    approval_decision_in(ledger.conn(), run_id)
}

fn approval_decision_in(conn: &rusqlite::Connection, run_id: &RunId) -> Result<Option<Decision>> {
    db::one(
        conn,
        "SELECT * FROM decisions WHERE subject_type = 'run' AND subject_id = ?1 \
         AND choice = 'approve' AND decided_by = 'human' ORDER BY id DESC LIMIT 1",
        [run_id],
    )
}

fn open_run_decision(conn: &rusqlite::Connection, run_id: &RunId) -> Result<Option<Decision>> {
    db::one(
        conn,
        "SELECT * FROM decisions WHERE subject_type = 'run' AND subject_id = ?1 \
         AND choice IS NULL ORDER BY id DESC LIMIT 1",
        [run_id],
    )
}

/// The run's jobs, by segment.
pub fn jobs_of(ledger: &Ledger, run_id: &RunId) -> Result<Vec<Job>> {
    db::all(
        ledger.conn(),
        "SELECT * FROM jobs WHERE run_id = ?1 ORDER BY segment",
        [run_id],
    )
}

/// The current project id, refusing runs submitted under another one (or none).
pub fn check_same_project(ctx: &RepoContext, run: &Run) -> Result<ProjectId> {
    let current = config::require_project_id(ctx)?;
    let Some(project) = cards::run_project(run)? else {
        return Err(Error::Arb(LEGACY_RUN.replace("{run}", run.id.as_str())));
    };
    if project != current {
        bail!(
            "run {} was submitted for project {project}, but {} [project] id is {current}; refused",
            run.id,
            ctx.config_path().display()
        );
    }
    Ok(current)
}

fn idea_of(conn: &rusqlite::Connection, run: &Run) -> Result<Idea> {
    db::one(conn, "SELECT * FROM ideas WHERE id = ?1", [run.idea_id.0])?
        .ok_or_else(|| Error::Arb(format!("run {} has no idea", run.id)))
}

/// `arb card`: the card recomputed now, and whether it still matches the stored hash.
pub fn card_for(ctx: &RepoContext, ledger: &Ledger, run_id: &RunId) -> Result<(Card, bool)> {
    let run = records::get_run(ledger, run_id)?;
    let idea = idea_of(ledger.conn(), &run)?;
    let (profile, plan) = cards::replan(&run)?;
    let card = cards::build_card(ctx, ledger.conn(), &run, &idea, &profile, &plan, &[])?;
    let same = run.card_hash.as_deref() == Some(card.card_hash.as_str());
    Ok((card, same))
}

/// How the run's command was built (stored in `request_json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    pub command: Vec<String>,
    pub base: Vec<String>,
    /// `explicit` (`-- CMD`) | `config` (`[run] command`).
    pub source: String,
    pub overrides: Vec<(String, String)>,
    pub style: String,
}

impl CommandSpec {
    /// The keys it adds to `request_json`.
    pub fn request_extra(&self) -> Value {
        json!({
            "command_source": self.source,
            "base_command": self.base,
            "overrides": self.overrides.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "override_style": self.style,
        })
    }
}

fn strings_of(v: Option<&Value>) -> Option<Vec<String>> {
    v?.as_array()?
        .iter()
        .map(|x| x.as_str().map(str::to_string))
        .collect()
}

/// The command: `-- CMD`, else the parent's base or `[run] command`, plus overrides (the
/// parent's, updated by the new ones) rendered in the configured style.
pub fn build_command(
    cfg: &crate::config::ProjectConfig,
    explicit: Option<&[String]>,
    overrides: &[(String, String)],
    parent_request: Option<&serde_json::Value>,
) -> Result<CommandSpec> {
    let overrides = overrides
        .iter()
        .map(|(k, v)| slurm::check_override(k, &OverrideValue::Str(v.clone())))
        .collect::<Result<Vec<_>>>()?;
    let (base, source, ov, style) = match (explicit.filter(|e| !e.is_empty()), parent_request) {
        (Some(e), _) => (
            e.to_vec(),
            "explicit".to_string(),
            overrides,
            cfg.run.override_style.clone(),
        ),
        (None, Some(parent)) => {
            // runs from before v4 always had an explicit `-- CMD` and no overrides
            let source = parent
                .get("command_source")
                .and_then(Value::as_str)
                .unwrap_or("explicit")
                .to_string();
            let old: Vec<(String, String)> = parent
                .get("overrides")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|kv| {
                            Some((
                                kv.get(0)?.as_str()?.to_string(),
                                kv.get(1)?.as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let ov = slurm::merge_overrides(&old, &overrides);
            if source == "explicit" {
                let base = strings_of(parent.get("base_command"))
                    .or_else(|| strings_of(parent.get("command")))
                    .ok_or_else(|| Error::arb("the parent run's request has no command"))?;
                let style = parent
                    .get("override_style")
                    .and_then(Value::as_str)
                    .map_or_else(|| cfg.run.override_style.clone(), str::to_string);
                (base, source, ov, style)
            } else {
                (
                    cfg.run.command.clone(),
                    source,
                    ov,
                    cfg.run.override_style.clone(),
                )
            }
        }
        (None, None) => (
            cfg.run.command.clone(),
            "config".to_string(),
            overrides,
            cfg.run.override_style.clone(),
        ),
    };
    if base.is_empty() {
        return Err(Error::arb(NO_COMMAND));
    }
    let mut command = base.clone();
    command.extend(slurm::render_overrides(&style, &ov)?);
    Ok(CommandSpec {
        command,
        base,
        source,
        overrides: ov,
        style,
    })
}

/// What `arb submit` was asked (CLI flags / `lab.run` arguments).
#[derive(Debug, Clone, Default)]
pub struct SubmitArgs<'a> {
    pub reason: &'a str,
    /// The command after `--` (None: `[run] command` or the parent's).
    pub command: Option<&'a [String]>,
    pub gpus: Option<u32>,
    /// Walltime text (`12h`).
    pub time: Option<&'a str>,
    pub nodes: Option<u32>,
    pub partition: Option<&'a str>,
    pub machine: Option<&'a str>,
    pub mem_gb: Option<u32>,
    pub cpus: Option<u32>,
    /// `--from rNNN`: reuse that run's snapshot, resources and overrides.
    pub from_run: Option<&'a RunId>,
    /// `--set k=v`, parsed.
    pub overrides: Vec<(String, String)>,
    pub allow_large: bool,
    pub repeat: bool,
    /// The notebook block that asked (v8 `runs.block_id`).
    pub block_id: Option<&'a str>,
}

/// What [`submit`] returns (`--json`: the run row plus these keys).
#[derive(Debug, Clone, Serialize)]
pub struct Submitted {
    #[serde(flatten)]
    pub run: Run,
    /// An identical earlier run was returned; nothing was written.
    pub existed: bool,
    pub decision_id: Option<DecisionId>,
    pub request: serde_json::Value,
    #[serde(skip)]
    pub card: Card,
}

/// An earlier run of this idea with the same card hash that is still usable.
fn find_same_run(conn: &rusqlite::Connection, idea: &Idea, card_hash: &str) -> Result<Option<Run>> {
    let marks = RunStatus::NOT_REUSABLE
        .iter()
        .map(|s| format!("'{}'", s.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    db::one(
        conn,
        &format!(
            "SELECT * FROM runs WHERE idea_id = ?1 AND card_hash = ?2 AND status NOT IN ({marks}) \
             ORDER BY id LIMIT 1"
        ),
        rusqlite::params![idea.id.0, card_hash],
    )
}

/// 4 random bytes as hex (Python `secrets.token_hex(4)`).
fn nonce() -> Result<String> {
    use std::io::Read;
    let mut buf = [0u8; 4];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// `true` when `s` is None or "" (Python `x or …`).
fn blank(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.is_empty())
}

/// `arb submit`: snapshot + request + card + open Decision (see module docs).
pub fn submit(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    idea: &Idea,
    args: SubmitArgs<'_>,
    warn: Warn<'_>,
) -> Result<Submitted> {
    let reason = require_text(TextField::ChangeReason, args.reason)?;
    let project = config::require_project_id(ctx)?;
    let cfg = config::project_config(ctx)?;
    let mut parent = None;
    let mut parent_req = None;
    if let Some(from) = args.from_run {
        let p = records::get_run(ledger, from)?;
        if p.idea_id != idea.id {
            bail!(
                "run {from} belongs to another idea; --from only reuses runs of {} (use lab.fork to start a new idea from it)",
                idea.name
            );
        }
        if p.request_json.as_deref().is_some_and(|r| !r.is_empty()) {
            parent_req = Some(cards::request_of(&p)?);
        }
        parent = Some(p);
    }
    let pask: Option<Ask> = match &parent_req {
        Some(r) => Some(serde_json::from_value(r["ask"].clone())?),
        None => None,
    };
    let gpus = args.gpus.or(pask.as_ref().map(|a| a.gpus));
    let time = args
        .time
        .map(str::to_string)
        .or_else(|| pask.as_ref().map(|a| slurm::human_duration(a.walltime_s)));
    let (Some(gpus), Some(time)) = (gpus, time) else {
        let hint = args
            .from_run
            .map(|f| format!(" ({f} was never submitted, so there is nothing to default to)"))
            .unwrap_or_default();
        bail!("--gpus and --time are required{hint}");
    };
    let machine = blank(args.machine)
        .or(pask
            .as_ref()
            .map(|a| a.machine.as_str())
            .filter(|m| !m.is_empty()))
        .or(blank(Some(cfg.default_machine.as_str())))
        .map(str::to_string);
    let Some(machine) = machine else {
        bail!("no machine: pass --machine or set default_machine in arboria.toml");
    };
    let profile = clusters::load_profile(&machine, None)?;
    let same_machine = pask.as_ref().is_some_and(|a| a.machine == machine);
    let same = |f: fn(&Ask) -> Option<u32>| {
        if same_machine {
            pask.as_ref().and_then(f)
        } else {
            None
        }
    };
    let spec = build_command(&cfg, args.command, &args.overrides, parent_req.as_ref())?;
    let partition = blank(args.partition)
        .map(str::to_string)
        .or_else(|| {
            same_machine
                .then(|| pask.as_ref().map(|a| a.partition.clone()))
                .flatten()
                .filter(|p| !p.is_empty())
        })
        .unwrap_or_else(|| profile.default_partition.clone());
    let ask = Ask {
        command: spec.command.clone(),
        gpus,
        nodes: args
            .nodes
            .unwrap_or_else(|| pask.as_ref().map_or(1, |a| a.nodes)),
        walltime_s: slurm::parse_walltime(&time)?,
        partition,
        machine: machine.clone(),
        mem_gb: args.mem_gb.or_else(|| same(|a| a.mem_gb)),
        cpus: args.cpus.or_else(|| same(|a| a.cpus)),
        repeat: if args.repeat { Some(nonce()?) } else { None },
    };
    // refuses before anything is written
    let plan = slurm::build_plan(&profile, &ask, &project)?;

    // snapshot outside the ledger lock (ADR 0002)
    let mut warnings: Vec<String> = Vec::new();
    let (commit, tree) = match &parent {
        Some(p) => {
            if !gitutil::ok(
                &[
                    "cat-file",
                    "-e",
                    &format!("{}^{{commit}}", p.snapshot_commit),
                ],
                &ctx.main,
            )? {
                let short: String = p.snapshot_commit.chars().take(12).collect();
                bail!("snapshot {short} of {} is missing from the repo", p.id);
            }
            (p.snapshot_commit.clone(), p.tree_hash.clone())
        }
        None => {
            let snap = records::take_snapshot(
                ledger,
                idea,
                SnapshotOptions {
                    allow_large: args.allow_large,
                    ..Default::default()
                },
                &mut |m: &str| {
                    warnings.push(m.to_string());
                    warn(m);
                },
            )?;
            (snap.commit, snap.tree)
        }
    };
    let h = cards::card_hash(&commit, &plan);
    let parent_id = parent.as_ref().map(|p| p.id.clone());
    let created = ledger.write(|tx| {
        if !args.repeat {
            if let Some(existing) = find_same_run(tx, idea, &h)? {
                return Ok(Err(existing));
            }
        }
        let run_id = records::insert_run_at(tx, idea, &reason, &commit, &tree, parent_id.as_ref())?;
        let mut request = plan.request();
        for (k, v) in spec.request_extra().as_object().expect("object") {
            request[k] = v.clone();
        }
        request["parent_run"] = json!(parent_id.as_ref().map(RunId::as_str));
        let wandb_url = cfg
            .wandb
            .run_url(&slurm::wandb_id(&project, run_id.as_str()));
        tx.execute(
            "UPDATE runs SET status = 'awaiting_approval', machine = ?1, \"partition\" = ?2, \
             gpus = ?3, walltime = ?4, est_cost = ?5, request_json = ?6, card_hash = ?7, \
             wandb_url = ?8, block_id = ?9, nonce = ?10, updated_at = ?11 WHERE id = ?12",
            rusqlite::params![
                machine,
                plan.partition,
                ask.gpus,
                slurm::human_duration(ask.walltime_s),
                plan.est_su(),
                crate::canonical::ledger_json(&request),
                h,
                wandb_url,
                args.block_id,
                ask.repeat,
                now(),
                run_id
            ],
        )?;
        let dec_id =
            records::insert_decision(tx, SubjectType::Run, run_id.as_str(), QUESTION, OPTIONS)?;
        tx.event(
            EventKind::RunSubmit,
            run_id.as_str(),
            &json!({"idea": idea.name.as_str(), "commit": commit, "machine": machine,
                    "card_hash": h, "est_su": plan.est_su(), "decision": dec_id.as_str(),
                    "parent_run": parent_id.as_ref().map(RunId::as_str), "repeat": ask.repeat}),
        )?;
        Ok(Ok((run_id, dec_id)))
    })?;
    let (run, existed, decision_id) = match created {
        Err(existing) => {
            let dec = open_run_decision(ledger.conn(), &existing.id)?.map(|d| d.id);
            (existing, true, dec)
        }
        Ok((run_id, dec_id)) => (records::get_run(ledger, &run_id)?, false, Some(dec_id)),
    };
    let card = cards::build_card(ctx, ledger.conn(), &run, idea, &profile, &plan, &warnings)?;
    Ok(Submitted {
        request: cards::request_of(&run)?,
        run,
        existed,
        decision_id,
        card,
    })
}

/// One submitted segment: `(segment, job id, adopted an existing job)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmittedSegment {
    pub segment: u32,
    pub job_id: String,
    pub adopted: bool,
}

/// What [`approve`] did.
#[derive(Debug, Clone, Serialize)]
pub struct ApproveResult {
    pub run: Run,
    /// None when resuming an earlier approval.
    pub decision_id: Option<DecisionId>,
    pub submitted: Vec<SubmittedSegment>,
}

fn set_job_submitted(
    ledger: &mut Ledger,
    run_id: &RunId,
    segment: u32,
    job_id: &str,
) -> Result<()> {
    ledger.write(|tx| {
        let ts = now();
        tx.execute(
            "UPDATE jobs SET job_id = ?1, state = 'pending', submitted_at = ?2, updated_at = ?2 \
             WHERE run_id = ?3 AND segment = ?4",
            rusqlite::params![job_id, ts, run_id, segment],
        )?;
        tx.event(
            EventKind::JobSubmit,
            run_id.as_str(),
            &json!({"segment": segment, "job_id": job_id}),
        )
    })
}

/// Ship the code once, then submit each missing segment idempotently (each through a
/// fresh `guarded_permit`).
pub fn submit_segments(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run: &Run,
    plan: &Plan,
    backend: &dyn Backend,
) -> Result<Vec<SubmittedSegment>> {
    let run_id = &run.id;
    let idea = idea_of(ledger.conn(), run)?;
    let paths = &plan.env.paths;
    let rows = jobs_of(ledger, run_id)?;
    if !rows.iter().any(|j| j.job_id.is_some()) {
        backend.ship(
            &ctx.main,
            &run.snapshot_commit,
            &slurm::code_dir(paths, &plan.project, run_id),
        )?;
    }
    let mut done = Vec::new();
    let mut prev: Option<String> = None;
    for k in 1..=plan.segments.len() as u32 {
        let name = slurm::job_name(&plan.project, run_id, k)?;
        let row = rows.iter().find(|j| j.segment == k);
        if let Some(id) = row.and_then(|j| j.job_id.clone()) {
            prev = Some(id);
            continue;
        }
        if row.is_none() {
            ledger.write(|tx| {
                tx.execute(
                    "INSERT INTO jobs (run_id, segment, job_name, state, updated_at) \
                     VALUES (?1, ?2, ?3, 'submitting', ?4)",
                    rusqlite::params![run_id, k, name, now()],
                )?;
                Ok(())
            })?;
        }
        let found = backend.find_job(&name)?;
        let adopted = found.is_some();
        let job_id = match found {
            Some(id) => id,
            None => {
                let script = slurm::render_sbatch(plan, run_id, &idea.name, k)?;
                // the single call site of Backend::submit
                let permit = guarded_permit(ledger, run_id, &name)?;
                backend.submit(&permit, &script, &name, prev.as_deref())?
            }
        };
        set_job_submitted(ledger, run_id, k, &job_id)?;
        done.push(SubmittedSegment {
            segment: k,
            job_id: job_id.clone(),
            adopted,
        });
        prev = Some(job_id);
    }
    Ok(done)
}

/// `arb approve` (human only): show the card (`show`), the human types the run id, record
/// the Decision bound to the card hash, then submit. Re-run to resume.
pub fn approve(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    show: &mut dyn FnMut(&str),
) -> Result<ApproveResult> {
    human::require_terminal(&format!("arb approve {run_id}"))?;
    approve_confirmed(
        ctx,
        ledger,
        run_id,
        show,
        &mut |id| human::confirm(id),
        None,
    )
}

/// The app's approval (ADR 0004): the human read the card whose hash is `shown_hash` for
/// `shown_for`, then confirms with Touch ID ([`human::confirm_on_device`]). The same path
/// as `arb approve` after that: the Decision is bound to the card hash, then submitted.
pub fn approve_on_device(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    shown_hash: &str,
    shown_for: std::time::Duration,
) -> Result<ApproveResult> {
    let run = records::get_run(ledger, run_id)?;
    if run.card_hash.as_deref() != Some(shown_hash) {
        return Err(Error::arb(CARD_CHANGED_SINCE_SHOWN));
    }
    approve_confirmed(
        ctx,
        ledger,
        run_id,
        &mut |_: &str| {},
        &mut |id| human::confirm_on_device(id, shown_hash, shown_for),
        None,
    )
}

/// The card the app showed is not the run's card any more.
pub const CARD_CHANGED_SINCE_SHOWN: &str = "审核单在你看过之后变了：请重新打开它再签字";

/// [`approve`] after the terminal check. `confirm` is [`human::confirm`] (a unit test
/// passes its test-only token); `backend` overrides the profile's (unit tests).
fn approve_confirmed(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    show: &mut dyn FnMut(&str),
    confirm: &mut dyn FnMut(&str) -> Result<human::Confirmed>,
    backend: Option<&dyn Backend>,
) -> Result<ApproveResult> {
    let _lock = run_lock(ctx, run_id)?;
    let run = records::get_run(ledger, run_id)?;
    if !matches!(
        run.status,
        RunStatus::AwaitingApproval | RunStatus::Approved
    ) {
        bail!(
            "run {run_id} is {}; only awaiting_approval runs can be approved",
            run.status
        );
    }
    let project = check_same_project(ctx, &run)?;
    let (profile, plan) = cards::replan(&run)?;
    if run.card_hash.as_deref() != Some(cards::card_hash(&run.snapshot_commit, &plan).as_str()) {
        return Err(Error::arb(CARD_CHANGED));
    }
    if run.status == RunStatus::AwaitingApproval {
        let idea = idea_of(ledger.conn(), &run)?;
        show(&cards::build_card(ctx, ledger.conn(), &run, &idea, &profile, &plan, &[])?.text());
    }
    let confirmed = confirm(run_id.as_str())?;
    let mut dec_id = None;
    if run.status == RunStatus::AwaitingApproval {
        let prov = serde_json::to_value(confirmed.provenance())?;
        dec_id = Some(ledger.write(|tx| {
            let current: String =
                tx.query_row("SELECT status FROM runs WHERE id = ?1", [run_id], |r| {
                    r.get(0)
                })?;
            if current != "awaiting_approval" {
                bail!("run {run_id} changed to {current} meanwhile");
            }
            let Some(dec) = open_run_decision(tx, run_id)? else {
                bail!("run {run_id} has no open approval decision");
            };
            records::set_decision(tx, &dec.id, "approve", records::AUTHOR_HUMAN, Some(&prov))?;
            tx.execute(
                "UPDATE runs SET status = 'approved', updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now(), run_id],
            )?;
            tx.event(
                EventKind::RunApprove,
                run_id.as_str(),
                &json!({"decision": dec.id.as_str(), "card_hash": run.card_hash,
                        "author": records::AUTHOR_HUMAN}),
            )?;
            Ok(dec.id)
        })?);
    } else if approval_decision(ledger, run_id)?.is_none() {
        bail!("run {run_id} is approved but has no human approval Decision");
    }
    let run = records::get_run(ledger, run_id)?;
    let submitted = (|| {
        let owned;
        let backend = match backend {
            Some(b) => b,
            None => {
                owned = crate::backend::get_backend(&profile, &project)?;
                owned.as_ref()
            }
        };
        submit_segments(ctx, ledger, &run, &plan, backend)
    })()
    .map_err(|e| match e {
        Error::Backend(_) | Error::NeedLogin(_) => Error::Arb(format!(
            "{e}\n{run_id} 已批准，但提交没有完成。处理好之后重新运行 `arb approve {run_id}` 继续（已提交的段不会重复提交）。"
        )),
        other => other,
    })?;
    let job_ids: Vec<Option<String>> = jobs_of(ledger, run_id)?
        .into_iter()
        .map(|j| j.job_id)
        .collect();
    ledger.write(|tx| {
        tx.execute(
            "UPDATE runs SET status = 'queued', updated_at = ?1 WHERE id = ?2 AND status = 'approved'",
            rusqlite::params![now(), run_id],
        )?;
        tx.event(EventKind::RunQueued, run_id.as_str(), &json!({"jobs": job_ids}))
    })?;
    Ok(ApproveResult {
        run: records::get_run(ledger, run_id)?,
        decision_id: dec_id,
        submitted,
    })
}

/// `arb reject`: decide the open Decision as reject (reason saved as a note). No backend.
pub fn reject(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    reason: Option<&str>,
) -> Result<Run> {
    let _ = ctx;
    let run = records::get_run(ledger, run_id)?;
    if run.status != RunStatus::AwaitingApproval {
        bail!(
            "run {run_id} is {}; only awaiting_approval runs can be rejected (use `arb cancel` for submitted runs)",
            run.status
        );
    }
    let reason = validate_text(TextField::Note, reason, false)?;
    ledger.write(|tx| {
        let dec = open_run_decision(tx, run_id)?;
        if let Some(d) = &dec {
            records::set_decision(tx, &d.id, "reject", records::AUTHOR_HUMAN, None)?;
        }
        tx.execute(
            "UPDATE runs SET status = 'rejected', updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now(), run_id],
        )?;
        let note = match &reason {
            Some(r) => Some(
                records::insert_note(
                    tx,
                    run.idea_id,
                    r,
                    Some(run_id),
                    None,
                    records::AUTHOR_HUMAN,
                )?
                .0,
            ),
            None => None,
        };
        tx.event(
            EventKind::RunReject,
            run_id.as_str(),
            &json!({"decision": dec.map(|d| d.id.to_string()), "note": note}),
        )
    })?;
    records::get_run(ledger, run_id)
}

/// `.arboria/runs/<run>`: where `ARBORIA_OUT` is fetched to.
pub fn local_run_dir(ctx: &RepoContext, run_id: &RunId) -> PathBuf {
    ctx.state_dir().join("runs").join(run_id.as_str())
}

#[cfg(test)]
mod tests {
    //! The whole path with FakeSlurm, mirroring tests/test_submit.py. One test: it sets
    //! `ARBORIA_CONFIG_DIR` (no other unit test in this crate reads it).

    use super::*;
    use crate::backend::{FakeSlurm, Scenario};
    use crate::model::{Event, JobState};
    use crate::monitor;
    use std::path::Path;
    use std::process::Command;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {o:?}");
        String::from_utf8(o.stdout).unwrap().trim_end().to_string()
    }

    /// `world()` sets the process-wide `ARBORIA_CONFIG_DIR`: one world at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct World {
        _serial: std::sync::MutexGuard<'static, ()>,
        _tmp: Tmp,
        ctx: RepoContext,
        ledger: Ledger,
        idea: Idea,
        fake: FakeSlurm,
        clusters_toml: PathBuf,
    }

    const PROJECT: &str = "wavefront-3f2a";

    fn world(tag: &str) -> World {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let root =
            std::env::temp_dir().join(format!("arboria-submit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let tmp = Tmp(root.clone());
        let main = root.join("WaveFront");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("train.py"), "print('train')\n").unwrap();
        git(&main, &["add", "-A"]);
        git(&main, &["commit", "-q", "-m", "init"]);
        std::fs::write(
            main.join("arboria.toml"),
            format!("default_machine = \"fake\"\n[project]\nid = \"{PROJECT}\"\n[watch]\nobservation_minutes = 30\n"),
        )
        .unwrap();
        let ctx = crate::paths::locate(Some(&main)).unwrap();
        crate::paths::ensure_state_excluded(&ctx.common_dir).unwrap();
        let mut ledger = Ledger::open(&ctx.state_dir()).unwrap();
        db::init_backup_repo(&ctx.state_dir()).unwrap();
        let r = root.display();
        let cfg_dir = root.join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let clusters_toml = cfg_dir.join("clusters.toml");
        std::fs::write(
            &clusters_toml,
            format!(
                "[fake]\nkind = \"fake\"\nroot = \"{r}/fake/state\"\naccount = \"acct1\"\n\
                 gpus_per_node = 4\ncpus_per_gpu = 16\nmem_per_gpu_gb = 100\n\
                 default_partition = \"ghx4\"\nmax_segments = 2\nenv_activate = []\n\
                 [fake.partitions.ghx4]\nmax_walltime = \"48h\"\n\
                 [fake.partitions.\"ghx4-interactive\"]\nmax_walltime = \"2h\"\ncharge = 2\n\
                 [fake.paths]\nruns = \"{r}/fake/remote/$USER/runs\"\ndata = \"{r}/fake/remote/data\"\n\
                 cache = \"{r}/fake/remote/cache\"\nckpt = \"{r}/fake/remote/ckpt\"\nout = \"{r}/fake/remote/out\"\n"
            ),
        )
        .unwrap();
        std::env::set_var(clusters::CONFIG_DIR_ENV, &cfg_dir);
        let idea = crate::ideas::new_idea(
            &ctx,
            &mut ledger,
            &crate::model::IdeaName::parse("warmup").unwrap(),
            crate::ideas::NewIdea {
                hypothesis: "warmup helps",
                metric: "val loss",
                ..Default::default()
            },
            &mut |_: &str| {},
        )
        .unwrap();
        let profile = clusters::load_profile("fake", None).unwrap();
        let fake = FakeSlurm::new(profile, ProjectId::parse(PROJECT).unwrap()).unwrap();
        World {
            _serial: serial,
            _tmp: tmp,
            ctx,
            ledger,
            idea,
            fake,
            clusters_toml,
        }
    }

    fn cmd() -> Vec<String> {
        ["python", "train.py", "--lr", "3e-4"]
            .map(String::from)
            .to_vec()
    }

    fn submit_run(w: &mut World, time: &str) -> Submitted {
        let c = cmd();
        submit(
            &w.ctx,
            &mut w.ledger,
            &w.idea.clone(),
            SubmitArgs {
                reason: "try warmup",
                command: Some(&c),
                gpus: Some(4),
                time: Some(time),
                ..Default::default()
            },
            &mut |_: &str| {},
        )
        .unwrap()
    }

    fn approve_run(w: &mut World, run: &str) -> Result<ApproveResult> {
        let id = RunId::parse(run).unwrap();
        let fake = w.fake.clone();
        approve_confirmed(
            &w.ctx,
            &mut w.ledger,
            &id,
            &mut |_: &str| {},
            &mut |_| Ok(human::Confirmed::for_tests()),
            Some(&fake),
        )
    }

    fn jn(run: &str, k: u32) -> String {
        format!("arb-{PROJECT}-{run}-s{k}")
    }

    fn rid(s: &str) -> RunId {
        RunId::parse(s).unwrap()
    }

    fn job_rows(w: &World, run: &str) -> Vec<(Option<String>, JobState)> {
        jobs_of(&w.ledger, &rid(run))
            .unwrap()
            .into_iter()
            .map(|j| (j.job_id, j.state))
            .collect()
    }

    fn backups(w: &World) -> Vec<String> {
        git(&w.ctx.state_dir(), &["log", "--format=%s"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn calls(w: &World, m: &str) -> Vec<Value> {
        w.fake.calls(Some(m)).unwrap()
    }

    fn poll(w: &mut World, run: &str) -> monitor::PollOutcome {
        let fake = w.fake.clone();
        monitor::poll_run(&w.ctx, &mut w.ledger, &rid(run), Some(&fake)).unwrap()
    }

    #[test]
    fn the_helper_polls_and_notifies() {
        use crate::helper::{self, Deps, HelperState};
        let mut w = world("helper");
        let roots = vec![w.ctx.main.clone()];
        let mut st = HelperState::default();
        let fake = w.fake.clone();
        let mut sent: Vec<String> = Vec::new();
        let pass = |st: &mut HelperState, sent: &mut Vec<String>| {
            let f = fake.clone();
            let mut make = move |_: &RepoContext, _: &Run| -> Result<Box<dyn Backend>> {
                Ok(Box::new(f.clone()))
            };
            let mut notify = |_: &str, b: &str| sent.push(b.to_string());
            let mut deps = Deps {
                make_backend: &mut make,
                runner: std::sync::Arc::new(crate::backend::SystemRunner),
                notify: &mut notify,
                now: 1_800_000_000,
                probes: false,
            };
            helper::tick_at(st, &roots, None, &mut deps)
        };
        let s1 = submit_run(&mut w, "12h");
        let p = pass(&mut st, &mut sent);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert!(sent.is_empty(), "the first pass only sets the cursor");
        let s2 = submit_run(&mut w, "2h");
        pass(&mut st, &mut sent);
        assert_eq!(sent, [format!("WaveFront · {} 等你签字", s2.run.id)]);
        sent.clear();
        approve_run(&mut w, s1.run.id.as_str()).unwrap();
        fake.advance(100_000).unwrap();
        let p = pass(&mut st, &mut sent);
        let run = records::get_run(&w.ledger, &s1.run.id).unwrap();
        assert_eq!(run.status, RunStatus::Succeeded, "{p:?}");
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(
            sent[0].starts_with(&format!("WaveFront · {} ", s1.run.id)),
            "{sent:?}"
        );
        sent.clear();
        approve_run(&mut w, s2.run.id.as_str()).unwrap();
        fake.set_need_login(true).unwrap();
        pass(&mut st, &mut sent);
        assert!(
            sent.iter().any(|s| s.starts_with("fake 需要登录：")),
            "{sent:?}"
        );
        assert!(st.needs_login.contains_key("fake"));
        sent.clear();
        pass(&mut st, &mut sent);
        assert!(
            !sent.iter().any(|s| s.starts_with("fake 需要登录")),
            "reported once: {sent:?}"
        );
        fake.set_need_login(false).unwrap();
        sent.clear();
        pass(&mut st, &mut sent);
        assert!(
            st.needs_login.is_empty(),
            "cleared once the machine answers"
        );
        assert_eq!(st.last_tick.as_deref(), Some("2027-01-15T08:00:00Z"));
    }

    #[test]
    fn approve_on_device_needs_the_shown_card_and_touch_id() {
        let mut w = world("device");
        let s = submit_run(&mut w, "12h");
        let hash = s.run.card_hash.clone().unwrap();
        let long = std::time::Duration::from_secs(3);
        let e = approve_on_device(&w.ctx, &mut w.ledger, &s.run.id, "0000", long).unwrap_err();
        assert_eq!(e.to_string(), CARD_CHANGED_SINCE_SHOWN);
        let short = std::time::Duration::from_millis(500);
        let e = approve_on_device(&w.ctx, &mut w.ledger, &s.run.id, &hash, short).unwrap_err();
        assert!(matches!(e, Error::NotHuman(_)), "{e}");
        if !cfg!(target_os = "macos") {
            let e = approve_on_device(&w.ctx, &mut w.ledger, &s.run.id, &hash, long).unwrap_err();
            assert!(e.to_string().contains("arb approve r001"), "{e}");
        }
        assert!(approval_decision(&w.ledger, &s.run.id).unwrap().is_none());
        let run = records::get_run(&w.ledger, &s.run.id).unwrap();
        assert_eq!(run.status, RunStatus::AwaitingApproval);
        assert!(w.fake.clone().jobs().unwrap_or_default().is_empty());
    }

    #[test]
    fn submit_approve_poll_watch_cancel_reject() {
        let mut w = world("flow");

        // ---- submit: local only, idempotent, --repeat makes a new one
        let s = submit_run(&mut w, "12h");
        assert_eq!(s.run.id.as_str(), "r001");
        assert!(!s.existed);
        assert_eq!(s.run.status, RunStatus::AwaitingApproval);
        assert_eq!(s.decision_id.as_ref().unwrap().as_str(), "d001");
        assert_eq!(s.request["project"], PROJECT);
        assert_eq!(s.request["command_source"], "explicit");
        assert_eq!(s.run.walltime.as_deref(), Some("12h"));
        assert_eq!(s.run.est_cost, Some(48.0));
        assert!(s.card.text().contains("审核单"), "{}", s.card.text());
        assert!(
            w.fake.calls(None).unwrap().is_empty(),
            "submit never touches a backend"
        );
        let again = submit_run(&mut w, "12h");
        assert!(again.existed);
        assert_eq!(again.run.id.as_str(), "r001");
        assert_eq!(again.decision_id.as_ref().unwrap().as_str(), "d001");
        let c = cmd();
        let rep = submit(
            &w.ctx,
            &mut w.ledger,
            &w.idea.clone(),
            SubmitArgs {
                reason: "again",
                command: Some(&c),
                gpus: Some(4),
                time: Some("12h"),
                repeat: true,
                ..Default::default()
            },
            &mut |_: &str| {},
        )
        .unwrap();
        assert_eq!(rep.run.id.as_str(), "r002");
        assert_eq!(rep.run.nonce.as_ref().map(String::len), Some(8));
        assert_ne!(rep.run.card_hash, s.run.card_hash);
        // --from: resources and command default to the parent's
        let child = submit(
            &w.ctx,
            &mut w.ledger,
            &w.idea.clone(),
            SubmitArgs {
                reason: "longer",
                from_run: Some(&rid("r001")),
                time: Some("20h"),
                overrides: vec![("seed".into(), "1".into())],
                ..Default::default()
            },
            &mut |_: &str| {},
        )
        .unwrap();
        assert_eq!(
            child.run.parent_run.as_ref().map(RunId::as_str),
            Some("r001")
        );
        assert_eq!(child.run.gpus, Some(4));
        assert_eq!(child.run.snapshot_commit, s.run.snapshot_commit);
        assert_eq!(
            child.request["ask"]["command"],
            json!(["python", "train.py", "--lr", "3e-4", "seed=1"])
        );
        let err = submit(
            &w.ctx,
            &mut w.ledger,
            &w.idea.clone(),
            SubmitArgs {
                reason: "x",
                command: Some(&c),
                ..Default::default()
            },
            &mut |_: &str| {},
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "--gpus and --time are required");

        // ---- reject r002 (no backend), with a note
        let rejected = reject(&w.ctx, &mut w.ledger, &rid("r002"), Some("lr too high")).unwrap();
        assert_eq!(rejected.status, RunStatus::Rejected);
        assert_eq!(
            backups(&w)[0],
            "arboria backup: decision d002; run_reject r002"
        );
        assert!(reject(&w.ctx, &mut w.ledger, &rid("r002"), None)
            .unwrap_err()
            .to_string()
            .contains("is rejected"));

        // ---- the guard: no human Decision, no submit
        let err = guarded_permit(&w.ledger, &rid("r001"), "x").unwrap_err();
        assert_eq!(
            err.to_string(),
            "refusing to submit x: run r001 has no human approval Decision in the ledger"
        );

        // ---- card change refuses approval
        let original = std::fs::read_to_string(&w.clusters_toml).unwrap();
        std::fs::write(
            &w.clusters_toml,
            original.replace("env_activate = []", "env_activate = [\"pixi shell\"]"),
        )
        .unwrap();
        assert_eq!(
            approve_run(&mut w, "r001").unwrap_err().to_string(),
            cards::CARD_CHANGED
        );
        let (_, same) = card_for(&w.ctx, &w.ledger, &rid("r001")).unwrap();
        assert!(!same);
        std::fs::write(&w.clusters_toml, &original).unwrap();
        assert!(w.fake.calls(None).unwrap().is_empty());

        // ---- approve r001: decision first, then ship, find, submit
        w.fake
            .script(
                &jn("r001", 1),
                Scenario {
                    run_s: 3600,
                    queue_s: 60,
                    billed: Some(4.25),
                    log: "epoch 1 loss 2.1\n".into(),
                    out_files: [("config.yaml".to_string(), "lr: 3e-4\n".to_string())].into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let res = approve_run(&mut w, "r001").unwrap();
        assert_eq!(res.decision_id.as_ref().unwrap().as_str(), "d001");
        assert_eq!(res.run.status, RunStatus::Queued);
        assert_eq!(
            res.submitted,
            [SubmittedSegment {
                segment: 1,
                job_id: "1001".into(),
                adopted: false
            }]
        );
        let calls_now: Vec<String> = w
            .fake
            .calls(None)
            .unwrap()
            .iter()
            .map(|c| c[0].as_str().unwrap().to_string())
            .collect();
        assert_eq!(calls_now, ["ship", "find_job", "submit"]);
        let b = backups(&w);
        assert_eq!(b[0], "arboria backup: run_queued r001");
        assert!(
            b[1].starts_with("arboria backup: decision d001; run_approve r001"),
            "{b:?}"
        );
        let dec = approval_decision(&w.ledger, &rid("r001")).unwrap().unwrap();
        assert_eq!(dec.decided_by.as_deref(), Some("human"));
        assert!(dec
            .payload_json
            .unwrap()
            .contains("\"confirmed_by\": \"unit test\""));
        let script = std::fs::read_to_string(crate::backend::fake_local_path(&format!(
            "{}/sbatch/s1.sh",
            slurm::run_dir(&w.fake.profile.paths, &w.fake.project, &rid("r001"))
        )))
        .unwrap();
        assert!(script.contains(&format!("#SBATCH --job-name={}", jn("r001", 1))));
        assert!(script.contains("--account=acct1"));
        assert!(script.trim_end().ends_with("python train.py --lr 3e-4"));
        let code = crate::backend::fake_local_path(&slurm::code_dir(
            &w.fake.profile.paths,
            &w.fake.project,
            &rid("r001"),
        ));
        assert_eq!(
            std::fs::read_to_string(code.join("train.py")).unwrap(),
            "print('train')\n"
        );
        assert!(approve_run(&mut w, "r001")
            .unwrap_err()
            .to_string()
            .contains("is queued"));

        // ---- poll: queued → running (local estimate, no backup) → succeeded, fetch, config
        assert_eq!(poll(&mut w, "r001").status, RunStatus::Queued);
        w.fake.advance(120).unwrap();
        let p = poll(&mut w, "r001");
        assert_eq!(
            (p.previous, p.status),
            (RunStatus::Queued, RunStatus::Running)
        );
        let n_backups = backups(&w).len();
        w.fake.advance(600).unwrap();
        poll(&mut w, "r001");
        let job = &jobs_of(&w.ledger, &rid("r001")).unwrap()[0];
        assert_eq!((job.state, job.elapsed_s), (JobState::Running, Some(660)));
        assert_eq!(
            job.billed_su,
            Some(slurm::py_round(660.0 / 3600.0 * 4.0, 4))
        );
        assert_eq!(backups(&w).len(), n_backups, "heartbeats never back up");
        w.fake.advance(3600).unwrap();
        let p = poll(&mut w, "r001");
        assert_eq!(p.status, RunStatus::Succeeded);
        assert!(p.became_final());
        assert!(
            p.messages[0].ends_with("r001 成功（4.25 SU）"),
            "{:?}",
            p.messages
        );
        assert!(p.messages[1].starts_with("r001: fetched ARBORIA_OUT into "));
        assert_eq!(p.messages[2], "r001: saved config.yaml into the ledger");
        let run = records::get_run(&w.ledger, &rid("r001")).unwrap();
        assert_eq!(run.actual_cost, Some(4.25));
        assert_eq!(
            run.config_json.as_deref(),
            Some("{\"_raw_yaml\": \"lr: 3e-4\\n\"}")
        );
        assert!(backups(&w).contains(&"arboria backup: run_final r001".to_string()));
        assert!(local_run_dir(&w.ctx, &rid("r001"))
            .join("out/config.yaml")
            .is_file());
        assert!(poll(&mut w, "r001").messages.is_empty(), "fetched once");

        // ---- logs / remote ls (read-only, project paths only)
        let (_, job) = monitor::log_job(&w.ctx, &w.ledger, &rid("r001"), None, 10).unwrap();
        assert_eq!(job.segment, 1);
        let log = w
            .fake
            .tail(
                &format!(
                    "{}/logs/s1.1001.out",
                    slurm::run_dir(&w.fake.profile.paths, &w.fake.project, &rid("r001"))
                ),
                10,
            )
            .unwrap();
        assert_eq!(log, "epoch 1 loss 2.1\n");
        assert_eq!(
            monitor::log_job(&w.ctx, &w.ledger, &rid("r001"), None, 0)
                .unwrap_err()
                .to_string(),
            "-n must be between 1 and 10000"
        );
        let (_, dir) =
            monitor::remote_dir(&w.ctx, &w.ledger, &rid("r001"), Some("logs/"), false).unwrap();
        assert!(dir.ends_with("/wavefront-3f2a/r001/logs"));
        assert_eq!(w.fake.list_dir(&dir).unwrap(), ["s1.1001.out"]);
        assert_eq!(
            monitor::remote_dir(&w.ctx, &w.ledger, &rid("r001"), Some("../x"), false)
                .unwrap_err()
                .to_string(),
            "invalid subpath '../x'"
        );

        // ---- two segments; s2 fails to submit once, resume submits only s2
        let s3 = submit_run(&mut w, "60h");
        assert_eq!(s3.run.id.as_str(), "r004");
        assert_eq!(s3.request["segments"].as_array().unwrap().len(), 2);
        w.fake
            .script(
                &jn("r004", 1),
                Scenario {
                    run_s: 100_000,
                    ..Default::default()
                },
            )
            .unwrap();
        w.fake
            .script(
                &jn("r004", 2),
                Scenario {
                    submit_error: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let err = approve_run(&mut w, "r004").unwrap_err().to_string();
        assert!(
            err.contains("已批准，但提交没有完成") && err.contains("arb approve r004"),
            "{err}"
        );
        assert_eq!(
            job_rows(&w, "r004"),
            [
                (Some("1002".into()), JobState::Pending),
                (None, JobState::Submitting)
            ]
        );
        let res = approve_run(&mut w, "r004").unwrap();
        assert_eq!(res.decision_id, None, "resuming: no second Decision");
        assert_eq!(res.submitted.len(), 1);
        assert_eq!(res.submitted[0].segment, 2);
        assert_eq!(
            w.fake.jobs().unwrap()[&res.submitted[0].job_id]
                .dependency
                .as_deref(),
            Some("1002")
        );
        assert_eq!(calls(&w, "ship").len(), 2, "shipped once per run");

        // ---- watch: a failure pattern in the window cancels the run
        w.fake
            .script(
                &jn("r005", 1),
                Scenario {
                    run_s: 7200,
                    log: "step 1 loss 2.5\nstep 10 loss nan\n".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        submit_run(&mut w, "61h");
        approve_run(&mut w, "r005").unwrap();
        // r004 is still active too
        w.fake.advance(120).unwrap();
        let mut echoed = Vec::new();
        let fake = w.fake.clone();
        let code = monitor::watch_with(
            &w.ctx,
            &mut w.ledger,
            monitor::WatchOptions {
                once: true,
                interval_s: 0,
                window_minutes: None,
                max_iterations: None,
            },
            &mut |m: &str| echoed.push(m.to_string()),
            &mut |_, _| Ok(Box::new(fake.clone())),
        )
        .unwrap();
        assert_eq!(code, 0);
        let pattern = r"(?i)\bloss\b[^\n]{0,40}\bnan\b";
        let want = format!(
            "r005 失败：s1 日志出现异常（匹配 {pattern}）：step 10 loss nan，已取消剩余作业"
        );
        assert!(echoed.iter().any(|m| m.ends_with(&want)), "{echoed:?}");
        let r5 = records::get_run(&w.ledger, &rid("r005")).unwrap();
        assert_eq!(r5.status, RunStatus::Failed);
        assert_eq!(
            job_rows(&w, "r005")
                .iter()
                .map(|(_, s)| *s)
                .collect::<Vec<_>>(),
            [JobState::Cancelled, JobState::Cancelled]
        );
        let ev: Event = db::one(
            w.ledger.conn(),
            "SELECT * FROM events WHERE kind = 'run_final' AND subject = 'r005'",
            [],
        )
        .unwrap()
        .unwrap();
        let payload: Value = serde_json::from_str(&ev.payload_json.unwrap()).unwrap();
        assert_eq!(
            (
                payload["pattern"].as_str(),
                payload["line"].as_str(),
                payload["by"].as_str()
            ),
            (Some(pattern), Some("step 10 loss nan"), Some("arb watch"))
        );

        // ---- NeedLogin pauses the machine: one notification, exit 1
        w.fake.set_need_login(true).unwrap();
        let mut echoed = Vec::new();
        let fake = w.fake.clone();
        let code = monitor::watch_with(
            &w.ctx,
            &mut w.ledger,
            monitor::WatchOptions {
                once: true,
                interval_s: 0,
                window_minutes: Some(30),
                max_iterations: None,
            },
            &mut |m: &str| echoed.push(m.to_string()),
            &mut |_, _| Ok(Box::new(fake.clone())),
        )
        .unwrap();
        assert_eq!(code, 1);
        assert_eq!(echoed.len(), 1, "{echoed:?}");
        assert!(echoed[0].contains("fake: 请先在终端运行 `ssh fake` 完成 Duo 认证"));
        assert!(echoed[0].ends_with("watch 已暂停对 fake 的远程调用，登录后重新运行 arb watch"));
        w.fake.set_need_login(false).unwrap();

        // ---- cancel r004
        let fake = w.fake.clone();
        let (run, cancelled) =
            monitor::cancel_with(&w.ctx, &mut w.ledger, &rid("r004"), Some(&fake)).unwrap();
        assert_eq!(run.status, RunStatus::Cancelled);
        assert_eq!(cancelled.len(), 2);
        assert_eq!(
            backups(&w)[0],
            "arboria backup: run_cancel r004; run_final r004"
        );
        assert!(
            monitor::cancel_with(&w.ctx, &mut w.ledger, &rid("r004"), Some(&fake))
                .unwrap_err()
                .to_string()
                .ends_with("nothing to cancel")
        );
        assert_eq!(
            monitor::cancel_with(&w.ctx, &mut w.ledger, &rid("r003"), Some(&fake))
                .unwrap_err()
                .to_string(),
            "run r003 was never approved; use `arb reject r003`"
        );
        let notes =
            std::fs::read_to_string(w.ctx.state_dir().join(monitor::NOTIFICATIONS)).unwrap();
        assert!(notes.contains("r004 已取消（arb cancel）"));

        // ---- a lost sbatch reply is adopted, not resubmitted
        submit_run(&mut w, "11h");
        w.fake
            .script(
                &jn("r006", 1),
                Scenario {
                    lose_reply: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(approve_run(&mut w, "r006")
            .unwrap_err()
            .to_string()
            .contains("connection lost"));
        assert_eq!(job_rows(&w, "r006"), [(None, JobState::Submitting)]);
        let n_submit = calls(&w, "submit").len();
        let res = approve_run(&mut w, "r006").unwrap();
        assert!(res.submitted[0].adopted);
        assert_eq!(calls(&w, "submit").len(), n_submit);

        // ---- the run lock refuses a second worker
        let _held = run_lock(&w.ctx, &rid("r006")).unwrap();
        assert_eq!(
            run_lock(&w.ctx, &rid("r006")).unwrap_err().to_string(),
            "another arb process is working on r006; try again shortly"
        );
    }
}
