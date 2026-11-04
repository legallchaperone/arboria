//! The approval card against Python: `cards.build_card` text, `as_dict` and hash for the
//! same plans (expected values printed by CPython 3.13 from `src/arboria/cards.py` with a
//! stub ledger; regenerate them by hand if the Python card changes).

mod common;

use arboria_core::cards;
use arboria_core::clusters::Profile;
use arboria_core::model::{Idea, ProjectId, Run};
use arboria_core::slurm::{self, Ask};
use rusqlite::Connection;
use serde_json::{json, Value};

const SIMPLE_TEXT: &str = "审核单 r001（idea dmd-distill）\n检查:\n  ✓ 快照 0123456789ab（不含被 .gitignore 忽略的文件）\n  ✓ 账户 bjki-dtai-gh\n  ✓ 计费卡数 = 申请卡数 = 1\n  ⚠ 未设项目预算（arboria.toml [budget] total_su = 0）\n资源:\n  机器 deltaai（aarch64），分区 ghx4，1 GPU\n  每节点 CPU 72，内存 110G\n  时长 1h\n  命令 python train.py\n  项目 wavefront-3f2a：作业名 arb-wavefront-3f2a-r001-s1，远程目录 /projects/bjki/$USER/arboria/runs/wavefront-3f2a/r001\n估计:\n  上限 1 卡 × 1.00 h × 1 = 1.00 SU（按申请的时长算满）\n预算:\n  项目已用 0.00 SU（arboria.toml 未设 [budget] total_su）\n  本 idea 已用 0.00 SU\n审核单哈希: 294c2218ec41979c";
const BUSY_TEXT: &str = "审核单 r012（idea dmd-distill）\n检查:\n  ✓ 快照 fedcba987654（不含被 .gitignore 忽略的文件）\n  ✓ 复用 r007 的快照，只改了命令参数和资源\n  ⚠ 重复运行（--repeat a1b2c3d4）：可能和已有的 run 完全相同，这是有意再跑一次\n  ⚠ 快照警告：大文件 data.bin（12 MB）\n  ✓ 账户 bjki-dtai-gh\n  ✓ 计费卡数 = 申请卡数 = 8\n  ⚠ 分区 ghx4-interactive 按 2 倍计费\n  ✓ 超过分区上限 2h，分 3 段（≤ max_segments 4），训练代码必须能续训\n  ⚠ 估计上限 64.00 SU 超过项目剩余 0.12 SU\n资源:\n  机器 deltaai（aarch64），分区 ghx4-interactive，2 节点 × 4 GPU\n  每节点 CPU 288，内存 300G\n  时长 4h1s，分段 02:00:00 + 02:00:00 + 00:00:01（--dependency=afterany 串联）\n  命令 python train.py lr=3e-4\n  项目 wavefront-3f2a：作业名 arb-wavefront-3f2a-r012-s1 …，远程目录 /projects/bjki/$USER/arboria/runs/wavefront-3f2a/r012\n估计:\n  上限 8 卡 × 4.00 h × 2 = 64.00 SU（按申请的时长算满）\n预算:\n  项目已用 99.88 / 100 SU，剩余 0.12\n  本 idea 已用 3.50 SU\n审核单哈希: 8a9dedc94dc1456f";
const SIMPLE_HASH: &str = "294c2218ec41979ca4f8b7c76d695543e002bc5b8403a9d6dea710a92507cc37";
const BUSY_HASH: &str = "8a9dedc94dc1456f27118862e3ba17d865934c98b27e7f148ca91e20e8b18113";

/// The DeltaAI profile of the `build_plan` vectors.
fn deltaai() -> Profile {
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(common::repo_root().join("tests/vectors/build_plan.json"))
            .unwrap(),
    )
    .unwrap();
    serde_json::from_value(file["cases"][0]["input"]["profile"].clone()).unwrap()
}

fn idea() -> Idea {
    serde_json::from_value(json!({
        "id": 1, "name": "dmd-distill", "stage": "open", "status": "open",
        "confidence": "guess", "created_at": "2026-11-04T00:00:00Z",
    }))
    .unwrap()
}

fn run(id: &str, commit: &str, parent: Option<&str>) -> Run {
    serde_json::from_value(json!({
        "id": id, "idea_id": 1, "snapshot_commit": commit, "tree_hash": "t",
        "change_reason": "x", "status": "draft", "created_at": "2026-11-04T00:00:00Z",
        "updated_at": "2026-11-04T00:00:00Z", "parent_run": parent,
    }))
    .unwrap()
}

/// `runs` / `jobs` with the columns `su_used` reads; `jobs` = (run, idea, billed_su).
fn ledger(jobs: &[(&str, i64, f64)]) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE runs (id TEXT PRIMARY KEY, idea_id INTEGER);
         CREATE TABLE jobs (id INTEGER PRIMARY KEY, run_id TEXT, billed_su REAL);",
    )
    .unwrap();
    for (run, idea, su) in jobs {
        conn.execute(
            "INSERT OR IGNORE INTO runs (id, idea_id) VALUES (?1, ?2)",
            rusqlite::params![run, idea],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (run_id, billed_su) VALUES (?1, ?2)",
            rusqlite::params![run, su],
        )
        .unwrap();
    }
    conn
}

fn ask(v: Value) -> Ask {
    serde_json::from_value(v).unwrap()
}

#[test]
fn simple_card_is_pythons() {
    let profile = deltaai();
    let ask = ask(
        json!({"command": ["python", "train.py"], "gpus": 1, "nodes": 1,
        "walltime_s": 3600, "partition": "ghx4", "machine": "deltaai"}),
    );
    let plan =
        slurm::build_plan(&profile, &ask, &ProjectId::parse("wavefront-3f2a").unwrap()).unwrap();
    let run = run("r001", "0123456789abcdef0123", None);
    let card =
        cards::card_with_budget(&ledger(&[]), &run, &idea(), &profile, &plan, &[], 0.0).unwrap();
    assert_eq!(card.text(), SIMPLE_TEXT);
    assert_eq!(card.card_hash, SIMPLE_HASH);
    assert_eq!(serde_json::to_value(&card).unwrap(), SIMPLE_DICT());
}

#[test]
fn busy_card_is_pythons() {
    // two nodes, interactive partition (charge 2, 3 segments), --repeat, --from, a
    // snapshot warning, and a budget the estimate overruns
    let profile = deltaai();
    let ask = ask(
        json!({"command": ["python", "train.py", "lr=3e-4"], "gpus": 8, "nodes": 2,
        "walltime_s": 4 * 3600 + 1, "partition": "ghx4-interactive", "machine": "deltaai",
        "mem_gb": 300, "repeat": "a1b2c3d4"}),
    );
    let plan =
        slurm::build_plan(&profile, &ask, &ProjectId::parse("wavefront-3f2a").unwrap()).unwrap();
    let run = run("r012", "fedcba9876543210ffff", Some("r007"));
    // 99.875 in total (rounds half-even to 99.88), 3.5 of it in this idea
    let conn = ledger(&[("r001", 1, 3.5), ("r002", 2, 90.0), ("r003", 2, 6.375)]);
    let warnings = ["大文件 data.bin（12 MB）".to_string()];
    let card =
        cards::card_with_budget(&conn, &run, &idea(), &profile, &plan, &warnings, 100.0).unwrap();
    assert_eq!(card.text(), BUSY_TEXT);
    assert_eq!(card.card_hash, BUSY_HASH);
    assert_eq!(serde_json::to_value(&card).unwrap(), BUSY_DICT());
}

#[allow(non_snake_case)]
fn SIMPLE_DICT() -> Value {
    serde_json::from_str(r#"{"run": "r001", "idea": "dmd-distill", "checks": [{"ok": true, "text": "快照 0123456789ab（不含被 .gitignore 忽略的文件）"}, {"ok": true, "text": "账户 bjki-dtai-gh"}, {"ok": true, "text": "计费卡数 = 申请卡数 = 1"}, {"ok": false, "text": "未设项目预算（arboria.toml [budget] total_su = 0）"}], "resources": ["机器 deltaai（aarch64），分区 ghx4，1 GPU", "每节点 CPU 72，内存 110G", "时长 1h", "命令 python train.py", "项目 wavefront-3f2a：作业名 arb-wavefront-3f2a-r001-s1，远程目录 /projects/bjki/$USER/arboria/runs/wavefront-3f2a/r001"], "estimate": ["上限 1 卡 × 1.00 h × 1 = 1.00 SU（按申请的时长算满）"], "budget": ["项目已用 0.00 SU（arboria.toml 未设 [budget] total_su）", "本 idea 已用 0.00 SU"], "card_hash": "294c2218ec41979ca4f8b7c76d695543e002bc5b8403a9d6dea710a92507cc37", "est_su": 1.0}"#).unwrap()
}

#[allow(non_snake_case)]
fn BUSY_DICT() -> Value {
    serde_json::from_str(r#"{"run": "r012", "idea": "dmd-distill", "checks": [{"ok": true, "text": "快照 fedcba987654（不含被 .gitignore 忽略的文件）"}, {"ok": true, "text": "复用 r007 的快照，只改了命令参数和资源"}, {"ok": false, "text": "重复运行（--repeat a1b2c3d4）：可能和已有的 run 完全相同，这是有意再跑一次"}, {"ok": false, "text": "快照警告：大文件 data.bin（12 MB）"}, {"ok": true, "text": "账户 bjki-dtai-gh"}, {"ok": true, "text": "计费卡数 = 申请卡数 = 8"}, {"ok": false, "text": "分区 ghx4-interactive 按 2 倍计费"}, {"ok": true, "text": "超过分区上限 2h，分 3 段（≤ max_segments 4），训练代码必须能续训"}, {"ok": false, "text": "估计上限 64.00 SU 超过项目剩余 0.12 SU"}], "resources": ["机器 deltaai（aarch64），分区 ghx4-interactive，2 节点 × 4 GPU", "每节点 CPU 288，内存 300G", "时长 4h1s，分段 02:00:00 + 02:00:00 + 00:00:01（--dependency=afterany 串联）", "命令 python train.py lr=3e-4", "项目 wavefront-3f2a：作业名 arb-wavefront-3f2a-r012-s1 …，远程目录 /projects/bjki/$USER/arboria/runs/wavefront-3f2a/r012"], "estimate": ["上限 8 卡 × 4.00 h × 2 = 64.00 SU（按申请的时长算满）"], "budget": ["项目已用 99.88 / 100 SU，剩余 0.12", "本 idea 已用 3.50 SU"], "card_hash": "8a9dedc94dc1456f27118862e3ba17d865934c98b27e7f148ca91e20e8b18113", "est_su": 64.0}"#).unwrap()
}
