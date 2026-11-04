//! The approval card: what the human reads before approving a run, and the hash the
//! approval binds to. Ports `arboria.cards` and `slurm.card_payload` / `card_hash`.
//!
//! Invariants:
//! - Pure reads (ledger, `arboria.toml`, the profile); no writes, no backend. The
//!   read-only dashboard / app shows the same card text.
//! - [`card_hash`] = sha256 hex of [`crate::canonical::canonical_json`] of
//!   [`card_payload`]; byte-compatible with Python (test vector `card_hash`). `repeat` is
//!   present only when set, so hashes from before `--repeat` stay valid.
//! - A changed request or profile changes the hash; approval of a stale card is refused
//!   (`审核单已变化，请重新 arb submit`).

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::canonical::{canonical_json, python_format_g};
use crate::clusters::{self, MachineKind, Profile};
use crate::config;
use crate::model::{Idea, IdeaId, ProjectId, Run};
use crate::paths::RepoContext;
use crate::slurm::{self, Ask, Plan};
use crate::{Error, Result};

/// Shown when the recomputed hash differs from the stored one.
pub const CARD_CHANGED: &str = "审核单已变化，请重新 arb submit";
/// A run submitted before project ids existed.
pub const LEGACY_RUN: &str = "run {run} 是在加入项目 id 之前提交的（request 里没有 project），作业名和远程目录可能与其他仓库冲突；请重新 arb submit";

/// Everything an approval is bound to (Python `slurm.card_payload`).
pub fn card_payload(snapshot_commit: &str, plan: &Plan) -> Value {
    let mut payload = json!({
        "snapshot_commit": snapshot_commit,
        "project": plan.project,
        "command": plan.ask.command,
        "machine": plan.ask.machine,
        "partition": plan.partition,
        "resources": {
            "account": plan.account,
            "nodes": plan.ask.nodes,
            "gpus": plan.ask.gpus,
            "gpus_per_node": plan.gpus_per_node,
            "cpus_per_node": plan.cpus_per_node,
            "mem_gb_per_node": plan.mem_gb_per_node,
            "billed_gpus": plan.billed_gpus,
            "charge": plan.charge,
        },
        "segments": plan.segments,
        "env": plan.env,
    });
    // absent otherwise, so hashes from before --repeat stay valid
    if let Some(repeat) = &plan.ask.repeat {
        payload["repeat"] = json!(repeat);
    }
    payload
}

/// sha256 hex of the canonical payload (Python `slurm.card_hash`).
pub fn card_hash(snapshot_commit: &str, plan: &Plan) -> String {
    sha256_hex(canonical_json(&card_payload(snapshot_commit, plan)).as_bytes())
}

/// Lowercase sha256 hex of `bytes` (Python `hashlib.sha256(b).hexdigest()`).
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `runs.request_json` decoded (refuses runs not made by `arb submit`).
pub fn request_of(run: &Run) -> Result<Value> {
    match run.request_json.as_deref() {
        Some(text) if !text.is_empty() => Ok(serde_json::from_str(text)?),
        _ => Err(Error::Arb(format!(
            "run {} was not created by `arb submit`",
            run.id
        ))),
    }
}

/// The project id the run was submitted under (None before project ids).
pub fn run_project(run: &Run) -> Result<Option<ProjectId>> {
    match request_of(run)?.get("project") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(p)) => ProjectId::parse(p).map(Some),
        // Python check_project's message, with the value as JSON instead of repr()
        Some(other) => Err(Error::Arb(format!(
            "invalid project id {other}; must match ^[a-z0-9][a-z0-9-]{{0,30}}$"
        ))),
    }
}

/// Re-derive the plan from the stored request and the *current* profile.
pub fn replan(run: &Run) -> Result<(Profile, Plan)> {
    let Some(project) = run_project(run)? else {
        return Err(Error::Arb(LEGACY_RUN.replace("{run}", run.id.as_str())));
    };
    let ask: Ask = serde_json::from_value(request_of(run)?["ask"].clone())?;
    let profile = clusters::load_profile(&ask.machine, None)?;
    let plan = slurm::build_plan(&profile, &ask, &project)?;
    Ok((profile, plan))
}

/// SU billed so far (project, or one idea), rounded to 2 places.
pub fn su_used(conn: &rusqlite::Connection, idea_id: Option<IdeaId>) -> Result<f64> {
    let s: f64 = match idea_id {
        None => conn.query_row(
            "SELECT COALESCE(SUM(billed_su), 0) AS s FROM jobs",
            [],
            |r| r.get(0),
        )?,
        Some(id) => conn.query_row(
            "SELECT COALESCE(SUM(j.billed_su), 0) AS s FROM jobs j \
             JOIN runs r ON r.id = j.run_id WHERE r.idea_id = ?",
            [id.0],
            |r| r.get(0),
        )?,
    };
    Ok(slurm::py_round2(s))
}

/// One card line with its check mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub ok: bool,
    pub text: String,
}

impl Check {
    fn ok(text: impl Into<String>) -> Self {
        Check {
            ok: true,
            text: text.into(),
        }
    }

    fn warn(text: impl Into<String>) -> Self {
        Check {
            ok: false,
            text: text.into(),
        }
    }
}

/// The card (Python `cards.Card`; `--json` shape is `as_dict`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    #[serde(rename = "run")]
    pub run_id: String,
    pub idea: String,
    pub checks: Vec<Check>,
    pub resources: Vec<String>,
    pub estimate: Vec<String>,
    pub budget: Vec<String>,
    pub card_hash: String,
    pub est_su: f64,
}

impl Card {
    /// The text the human reads (`审核单 r001（idea …）` …).
    pub fn text(&self) -> String {
        let mut lines = vec![
            format!("审核单 {}（idea {}）", self.run_id, self.idea),
            "检查:".to_string(),
        ];
        lines.extend(
            self.checks
                .iter()
                .map(|c| format!("  {} {}", if c.ok { '✓' } else { '⚠' }, c.text)),
        );
        for (title, items) in [
            ("资源:", &self.resources),
            ("估计:", &self.estimate),
            ("预算:", &self.budget),
        ] {
            lines.push(title.to_string());
            lines.extend(items.iter().map(|t| format!("  {t}")));
        }
        let short: String = self.card_hash.chars().take(16).collect();
        lines.push(format!("审核单哈希: {short}"));
        lines.join("\n")
    }
}

/// Build the card for a run (`warnings` = snapshot warnings at submit time).
pub fn build_card(
    ctx: &RepoContext,
    conn: &rusqlite::Connection,
    run: &Run,
    idea: &Idea,
    profile: &Profile,
    plan: &Plan,
    warnings: &[String],
) -> Result<Card> {
    let total = config::project_config(ctx)?.total_su;
    card_with_budget(conn, run, idea, profile, plan, warnings, total)
}

/// [`build_card`] with the project budget (`arboria.toml [budget] total_su`) given.
pub fn card_with_budget(
    conn: &rusqlite::Connection,
    run: &Run,
    idea: &Idea,
    profile: &Profile,
    plan: &Plan,
    warnings: &[String],
    total_su: f64,
) -> Result<Card> {
    let a = &plan.ask;
    let est_su = plan.est_su();
    let mut checks = Vec::new();
    let commit: String = run.snapshot_commit.chars().take(12).collect();
    checks.push(Check::ok(format!(
        "快照 {commit}（不含被 .gitignore 忽略的文件）"
    )));
    if let Some(parent) = &run.parent_run {
        checks.push(Check::ok(format!(
            "复用 {parent} 的快照，只改了命令参数和资源"
        )));
    }
    if let Some(repeat) = a.repeat.as_deref().filter(|r| !r.is_empty()) {
        checks.push(Check::warn(format!(
            "重复运行（--repeat {repeat}）：可能和已有的 run 完全相同，这是有意再跑一次"
        )));
    }
    checks.extend(
        warnings
            .iter()
            .map(|w| Check::warn(format!("快照警告：{w}"))),
    );
    checks.push(Check::ok(format!("账户 {}", plan.account)));
    if plan.over_billed() {
        checks.push(Check::warn(format!(
            "计费卡数 {billed} > 申请卡数 {gpus}：内存或 CPU 超过 {gpus} 卡的份额，按 {billed} 卡计费",
            billed = plan.billed_gpus,
            gpus = a.gpus
        )));
    } else {
        checks.push(Check::ok(format!(
            "计费卡数 = 申请卡数 = {}",
            plan.billed_gpus
        )));
    }
    if plan.charge != 1.0 {
        checks.push(Check::warn(format!(
            "分区 {} 按 {} 倍计费",
            plan.partition,
            python_format_g(plan.charge)
        )));
    }
    if plan.segments.len() > 1 {
        checks.push(Check::ok(format!(
            "超过分区上限 {}，分 {} 段（≤ max_segments {}），训练代码必须能续训",
            slurm::human_duration(plan.max_walltime_s),
            plan.segments.len(),
            profile.max_segments
        )));
    }
    if profile.kind == MachineKind::Fake {
        checks.push(Check::warn(format!(
            "机器 {} 是 FakeSlurm（本地模拟，不是真集群）",
            profile.name
        )));
    }

    let per = if a.nodes > 1 {
        format!("{} 节点 × {} GPU", a.nodes, plan.gpus_per_node)
    } else {
        format!("{} GPU", plan.gpus_per_node)
    };
    let arch = if profile.arch.is_empty() {
        "?"
    } else {
        &profile.arch
    };
    let mut duration = format!("时长 {}", slurm::human_duration(a.walltime_s));
    if plan.segments.len() > 1 {
        let parts: Vec<String> = plan
            .segments
            .iter()
            .map(|s| slurm::slurm_time(*s))
            .collect();
        duration.push_str(&format!(
            "，分段 {}（--dependency=afterany 串联）",
            parts.join(" + ")
        ));
    }
    let resources = vec![
        format!(
            "机器 {}（{arch}），分区 {}，{per}",
            profile.name, plan.partition
        ),
        format!(
            "每节点 CPU {}，内存 {}G",
            plan.cpus_per_node, plan.mem_gb_per_node
        ),
        duration,
        format!("命令 {}", a.command.join(" ")),
        format!(
            "项目 {}：作业名 {}{}，远程目录 {}",
            plan.project,
            slurm::job_name(&plan.project, &run.id, 1)?,
            if plan.segments.len() == 1 { "" } else { " …" },
            slurm::run_dir(&plan.env.paths, &plan.project, &run.id)
        ),
    ];
    let estimate = vec![format!(
        "上限 {} 卡 × {:.2} h × {} = {est_su:.2} SU（按申请的时长算满）",
        plan.billed_gpus,
        a.walltime_s as f64 / 3600.0,
        python_format_g(plan.charge)
    )];

    let used = su_used(conn, None)?;
    let idea_used = su_used(conn, Some(idea.id))?;
    let mut budget = Vec::new();
    if total_su > 0.0 {
        let left = total_su - used;
        budget.push(format!(
            "项目已用 {used:.2} / {} SU，剩余 {left:.2}",
            python_format_g(total_su)
        ));
        if est_su > left {
            checks.push(Check::warn(format!(
                "估计上限 {est_su:.2} SU 超过项目剩余 {left:.2} SU"
            )));
        }
    } else {
        budget.push(format!(
            "项目已用 {used:.2} SU（arboria.toml 未设 [budget] total_su）"
        ));
        checks.push(Check::warn(
            "未设项目预算（arboria.toml [budget] total_su = 0）",
        ));
    }
    budget.push(format!("本 idea 已用 {idea_used:.2} SU"));
    Ok(Card {
        run_id: run.id.to_string(),
        idea: idea.name.to_string(),
        checks,
        resources,
        estimate,
        budget,
        card_hash: card_hash(&run.snapshot_commit, plan),
        est_su,
    })
}
