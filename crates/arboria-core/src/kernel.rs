//! The writes of the notebook kernel (`arboria.lab` in the human's marimo kernel) on a
//! schema v8 ledger. Python opens v8 read-only (ADR 0002), so `lab` sends each write here
//! through the hidden `arb lab` command: one JSON request on stdin, one JSON reply on
//! stdout. Python reads the result back from the ledger itself.
//!
//! Invariants:
//! - The same checks as the Python functions they replace: what `lab` refuses inside
//!   Claude Code ([`LAB_HUMAN_ONLY`]) is refused here too, from the same environment
//!   variables, so calling `arb lab` directly gains nothing.
//! - Nothing here approves: `run` only creates a run that waits for the human's
//!   Decision ([`crate::submission::submit`]).
//! - Every write goes through the core functions the CLI uses (`Ledger::write`, the
//!   length checks, the backup events).

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::commands::{self, Call, Scope};
use crate::db::Ledger;
use crate::ideas::{self, CloseMode, NewIdea, SeedFields};
use crate::model::{
    Confidence, EventKind, Idea, IdeaName, IdeaStage, InspectView, RunId, TaskKind,
};
use crate::paths::{self, Home, RepoContext};
use crate::records::{self, py_repr, require_text, TextField};
use crate::submission::{self, SubmitArgs};
use crate::{bail, human, inspection, tasks, Error, Result};

/// `lab.HUMAN_ONLY`.
pub const LAB_HUMAN_ONLY: &str = "{what} 只由人在 notebook 里做；Claude 请用 `arb todo done tNNN --result \"...\"` 或 `arb run summary RUN \"...\"`";
/// `lab.SEED_NO_CONF`.
pub const SEED_NO_CONF: &str =
    "种子没有置信度：种下（arb idea plant）之后在 idea 的 notebook 里用 lab.conf";

fn human_only(what: &str) -> Result<()> {
    if human::in_claude_code().is_some() {
        bail!("{}", LAB_HUMAN_ONLY.replace("{what}", what));
    }
    Ok(())
}

/// Whose notebook the call came from.
enum Owner {
    Idea {
        ctx: RepoContext,
        ledger: Ledger,
        idea: Idea,
    },
    Seed {
        ledger: Ledger,
        seed: Idea,
    },
}

fn text<'a>(req: &'a Value, key: &str) -> Result<&'a str> {
    req[key]
        .as_str()
        .ok_or_else(|| Error::Arb(format!("arb lab: {key} must be a string")))
}

fn opt_text<'a>(req: &'a Value, key: &str) -> Option<&'a str> {
    req[key].as_str()
}

fn opt_u32(req: &Value, key: &str) -> Result<Option<u32>> {
    match &req[key] {
        Value::Null => Ok(None),
        v => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| Error::Arb(format!("arb lab: {key} must be a whole number"))),
    }
}

fn strings(req: &Value, key: &str) -> Result<Option<Vec<String>>> {
    match &req[key] {
        Value::Null => Ok(None),
        v => Ok(Some(serde_json::from_value(v.clone()).map_err(|_| {
            Error::Arb(format!("arb lab: {key} must be a list of strings"))
        })?)),
    }
}

fn pairs(req: &Value, key: &str) -> Result<Vec<(String, String)>> {
    match &req[key] {
        Value::Null => Ok(Vec::new()),
        v => serde_json::from_value(v.clone())
            .map_err(|_| Error::Arb(format!("arb lab: {key} must be a list of [key, value]"))),
    }
}

fn owner(req: &Value) -> Result<Owner> {
    if let Some(home) = opt_text(req, "home") {
        let home = Home {
            root: PathBuf::from(home),
        };
        let ledger = Ledger::open(&home.state_dir())?;
        let seed = ideas::get_seed(&ledger, &IdeaName::parse(text(req, "seed")?)?)?;
        return Ok(Owner::Seed { ledger, seed });
    }
    let ctx = paths::locate(Some(Path::new(text(req, "repo")?)))?.require_initialized()?;
    let ledger = Ledger::open(&ctx.state_dir())?;
    let idea = ideas::get_idea(&ledger, &IdeaName::parse(text(req, "idea")?)?)?;
    Ok(Owner::Idea { ctx, ledger, idea })
}

fn idea_owner(req: &Value) -> Result<(RepoContext, Ledger, Idea)> {
    match owner(req)? {
        Owner::Idea { ctx, ledger, idea } => Ok((ctx, ledger, idea)),
        Owner::Seed { seed, .. } => bail!(
            "这是一个种子（{}），还没种进仓库：先在目标仓库里运行 arb idea plant {}",
            seed.name,
            seed.name
        ),
    }
}

fn require_open(idea: &Idea) -> Result<()> {
    if idea.stage != IdeaStage::Open {
        bail!("idea {} is {}", py_repr(idea.name.as_str()), idea.stage);
    }
    Ok(())
}

/// One request (`{"op": …, "repo"/"idea" or "home"/"seed", …}`); the warnings it raised
/// go to `warnings`. The reply is what Python needs to read the result back.
pub fn handle(req: &Value, warnings: &mut Vec<String>) -> Result<Value> {
    let mut warn = |w: &str| warnings.push(w.to_string());
    match text(req, "op")? {
        "run" => {
            let (ctx, mut ledger, idea) = idea_owner(req)?;
            require_open(&idea)?;
            let from_run = opt_text(req, "from_run").map(RunId::parse).transpose()?;
            let command = strings(req, "command")?;
            let args = SubmitArgs {
                reason: text(req, "reason")?,
                command: command.as_deref(),
                gpus: opt_u32(req, "gpus")?,
                time: opt_text(req, "time"),
                nodes: opt_u32(req, "nodes")?,
                partition: opt_text(req, "partition"),
                machine: opt_text(req, "machine"),
                from_run: from_run.as_ref(),
                overrides: pairs(req, "overrides")?,
                repeat: req["repeat"].as_bool().unwrap_or(false),
                ..Default::default()
            };
            let s = submission::submit(&ctx, &mut ledger, &idea, args, &mut warn)?;
            Ok(json!({"id": s.run.id, "existed": s.existed}))
        }
        "todo" => {
            human_only("写待办")?;
            let (mut ledger, owner) = match owner(req)? {
                Owner::Idea { ledger, idea, .. } => (ledger, idea),
                Owner::Seed { ledger, seed } => (ledger, seed),
            };
            let todo = require_text(TextField::Todo, text(req, "text")?)?;
            if let Some(t) = tasks::open_plain_task(&ledger, owner.id, &todo)? {
                return Ok(json!({"id": t.task.id, "existed": true}));
            }
            let t = tasks::add(&mut ledger, &owner, &todo, TaskKind::Plain, None, None)?;
            Ok(json!({"id": t.task.id, "existed": false}))
        }
        "conf" => {
            human_only("改置信度")?;
            let (_, mut ledger, idea) = match owner(req)? {
                Owner::Seed { .. } => bail!("{SEED_NO_CONF}"),
                Owner::Idea { ctx, ledger, idea } => (ctx, ledger, idea),
            };
            let level: Confidence = text(req, "level")?.parse()?;
            records::set_confidence(&mut ledger, &idea, level)?;
            Ok(json!({}))
        }
        "metric" => {
            human_only("改指标")?;
            match owner(req)? {
                Owner::Idea {
                    mut ledger, idea, ..
                } => {
                    records::set_metric(&mut ledger, &idea, text(req, "text")?)?;
                }
                Owner::Seed { mut ledger, seed } => {
                    let fields = SeedFields {
                        metric: Some(text(req, "text")?),
                        ..Default::default()
                    };
                    ideas::set_seed_fields(&mut ledger, &seed.name, fields)?;
                }
            }
            Ok(json!({}))
        }
        "fork" => {
            let (ctx, mut ledger, idea) = idea_owner(req)?;
            let run = match opt_text(req, "from_run") {
                None => None,
                Some(r) => {
                    let run = records::get_run(&ledger, &RunId::parse(r)?)?;
                    if run.idea_id != idea.id {
                        bail!("run {} belongs to another idea", run.id);
                    }
                    Some(run)
                }
            };
            let name = IdeaName::parse(text(req, "name")?)?;
            let new = NewIdea {
                hypothesis: text(req, "hypothesis")?,
                metric: text(req, "metric")?,
                parent: Some(&idea.name),
                from_run: run.as_ref(),
                seed: None,
            };
            let made = ideas::new_idea(&ctx, &mut ledger, &name, new, &mut warn)?;
            Ok(json!({"name": made.name}))
        }
        "prune" => {
            human_only("关闭 idea")?;
            let (ctx, mut ledger, idea) = idea_owner(req)?;
            let reason = require_text(TextField::Note, text(req, "reason")?)?;
            let closed = ideas::close_idea(
                &ctx,
                &mut ledger,
                Some(&idea.name),
                CloseMode::Drop,
                None,
                false,
                &mut warn,
            )?;
            records::add_note(
                &mut ledger,
                &closed.idea,
                &format!("prune: {reason}"),
                None,
                None,
            )?;
            Ok(serde_json::to_value(&closed)?)
        }
        "command_request" => {
            human_only("写命令待办")?;
            let (mut ledger, owner) = match owner(req)? {
                Owner::Idea { ledger, idea, .. } => (ledger, idea),
                Owner::Seed { ledger, seed } => (ledger, seed),
            };
            let kind: TaskKind = text(req, "kind")?.parse()?;
            let args = strings(req, "args")?.unwrap_or_default();
            let kwargs = pairs(req, "kwargs")?;
            let (t, created) = commands::request(
                &mut ledger,
                &owner,
                kind,
                text(req, "name")?,
                Scope::parse(text(req, "scope")?)?,
                opt_text(req, "description"),
                Call {
                    args: &args,
                    kwargs: &kwargs,
                    path: opt_text(req, "path"),
                },
            )?;
            Ok(json!({"id": t.task.id, "created": created}))
        }
        "command_call" => {
            // a lightweight event (no backup commit), like Python `lab._log_call`
            let (mut ledger, key, owner) = match owner(req)? {
                Owner::Idea { ledger, idea, .. } => (ledger, "idea", idea),
                Owner::Seed { ledger, seed } => (ledger, "seed", seed),
            };
            let name = text(req, "name")?;
            commands::validate_name(name, false)?;
            let mut payload = Map::new();
            payload.insert(key.into(), owner.name.as_str().into());
            payload.insert(
                "scope".into(),
                Scope::parse(text(req, "scope")?)?.as_str().into(),
            );
            for k in ["version", "ok", "seconds"] {
                payload.insert(k.into(), req[k].clone());
            }
            ledger.write(|tx| tx.event(EventKind::CommandCall, name, &Value::Object(payload)))?;
            Ok(json!({}))
        }
        "inspect" => {
            if human::in_claude_code().is_some() {
                bail!(
                    "{}",
                    LAB_HUMAN_ONLY.replace("{what}", "请 agent 读代码（lab.inspect）")
                );
            }
            let (_, mut ledger, idea) = idea_owner(req)?;
            let view: InspectView = text(req, "view")?.parse()?;
            let run = opt_text(req, "run").map(RunId::parse).transpose()?;
            let st = inspection::ask(
                &mut ledger,
                &idea,
                text(req, "question")?,
                view,
                run.as_ref(),
                req["refresh"].as_bool().unwrap_or(false),
                true,
            )?;
            Ok(json!({"id": st.todo.map(|t| t.task.id), "created": st.created}))
        }
        op => bail!("arb lab: unknown request {}", py_repr(op)),
    }
}
