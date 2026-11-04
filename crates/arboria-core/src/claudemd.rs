//! The arb-managed block in the main worktree's CLAUDE.md: stable rules + accepted
//! lessons. Ports `arboria.claudemd`.
//!
//! Invariants:
//! - Only the text between [`BEGIN`] and [`END`] is arb's; everything else in the file is
//!   the user's and is preserved byte for byte. Duplicate or unbalanced markers are an
//!   error, never "repaired".
//! - The block has at most [`MAX_BLOCK_LINES`] lines and [`MAX_ACCEPTED_LESSONS`]
//!   lessons. Current experiment state is never written here (the hook delivers it).
//! - The auto-commit touches only CLAUDE.md (`git commit --only`), and is skipped with a
//!   hint during a merge / rebase / cherry-pick, without a user identity, or when
//!   disabled; it never uses the user's index otherwise.
//! - The rules text ([`RULES`], [`HOME_RULES`]) is Python's verbatim until the app
//!   changes the agent workflow (ADR 0001).

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::Ledger;
use crate::model::Lesson;
use crate::paths::RepoContext;
use crate::Result;

/// The file.
pub const FILENAME: &str = "CLAUDE.md";
/// Block start marker.
pub const BEGIN: &str = "<!-- arboria:begin — 由 arb 管理，块内修改会被覆盖 -->";
/// Block end marker.
pub const END: &str = "<!-- arboria:end -->";
/// Lines in the block.
pub const MAX_BLOCK_LINES: usize = 50;
/// Accepted lessons in the block.
pub const MAX_ACCEPTED_LESSONS: usize = 20;
/// The auto-commit's message.
pub const COMMIT_MESSAGE: &str = "arboria: update CLAUDE.md";
/// Heading of the lessons section.
pub const LESSONS_HEADING: &str = "## 经验（已确认）";
/// The project rules (Python `claudemd.RULES`, verbatim).
pub const RULES: &str = r#"## 研究工作流（Arboria）
- 实验状态只通过 `arb` 读写，不要手改 `.arboria/`。
- 当前状态（idea、run、待办、待批准的 run、最新经验）由会话开始时的 hook 注入；本文件只放稳定规则和已确认的经验。
- 一个 idea 一个文件夹（worktree）：只在当前 idea 的文件夹里改代码。
- `notebook.py`（marimo，链接到 `.arboria/notebooks/<idea>.py`）是人的：读它了解人的想法和分析，绝不修改。
- 你的工作来自待办：`arb todo list` 查看，做完用 `arb todo done tNNN --result "..."`（≤600 字）交代结果。
- 命令待办（人在 notebook 里调用了 `lab.<名字>(...)`）：先运行 `arb commands guide` 看约定，只写 `.arboria/commands/<名字>.py`（或全局命令目录）里的那个文件，再 `arb todo done`。读代码待办（`lab.inspect(...)`）按 `arb inspect guide`，用 `arb inspect done` 交付。
- 同一类分析在 notebook 里反复出现，或待办要的是一次分析：写成命令（默认项目范围），在 `arb todo done` 里写明 `lab.<名字>(...)` 怎么调用。
- 看状态用 `arb status`。`arb note` 只由人写；你的结论写进 `arb todo done --result` 或 `arb run summary RUN "..."`。
- 上集群：`arb submit --reason "..." --gpus N --time 12h [-- CMD]`，或 `arb submit --from rNNN --set k=v --reason "..."`。它只在本地生成快照和审核单，不会提交。
- 批准只能由人在自己的终端里做：你不能运行 `arb approve` 和 `arb lesson accept`；绝不直接 `sbatch`/`srun`，也不要 ssh 上集群。
- 看集群上的日志和文件用 `arb logs RUN`、`arb remote ls RUN`，不用 ssh；跑完的输出在 `.arboria/runs/RUN/out/`。
- 发现可复用的教训用 `arb lesson propose "..."` 提出，由人确认；不要直接写进 CLAUDE.md。
- 训练代码约定：
  1. 启动时把完整的解析后配置导出为 `$ARBORIA_OUT/config.yaml`。
  2. `wandb.init(id=ARBORIA_WANDB_ID, group=ARBORIA_IDEA)`。
  3. 路径只从 `ARBORIA_*` 变量读取。
  4. 可续训：checkpoint 存到 `ARBORIA_CKPT_DIR`，重启后能接着跑（超过分区上限的 run 会分段）。"#;
/// The seed home's rules (Python `claudemd.HOME_RULES`, verbatim).
pub const HOME_RULES: &str = r#"## 种子园（Arboria）
- 这里是人的种子园：还没有仓库的研究想法。当前的种子和待办由会话开始时的 hook 注入。
- `seeds/<名字>.py`（marimo）是人的 notebook：读它了解人的想法，绝不修改；也不要改 `.arboria/`。
- 种子就是还没有仓库的 idea：在这里（仓库外）`arb idea` 管的是种子。看种子用 `arb idea list`、`arb idea show NAME`。种子只由人写：`arb idea new/set/drop/revive`（以及旧名 `arb seed …`）你都不能运行。
- 种下（在目标仓库里 `arb idea plant NAME`）由人在自己的终端里做：它会在那个仓库里建 idea 的分支和 worktree。
- 你的工作来自待办：`arb todo list` 查看，做完用 `arb todo done tNNN --result "..."`（≤600 字）交代结果。
- 命令待办（人在种子 notebook 里调用了 `lab.<名字>(...)`）：先运行 `arb commands guide`，只写全局命令目录里的那个文件；种子没有 run，命令里不用 `ctx.run`。
- 讨论想法时引用种子 notebook 里的内容；你的结论写进 `arb todo done --result`，不写进 notebook。"#;

/// What a sync did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncResult {
    pub path: PathBuf,
    /// `created` | `replaced` | `appended`; None = unchanged.
    pub action: Option<String>,
    pub committed: bool,
    pub hints: Vec<String>,
}

/// The block text for these lessons.
pub fn render_block(lessons: &[Lesson]) -> String {
    todo!("port of arboria.claudemd.render_block")
}

/// Byte range of the block in `text` (None when absent; error on bad markers).
pub fn find_block(text: &str, where_: &str) -> Result<Option<(usize, usize)>> {
    todo!("port of arboria.claudemd.find_block")
}

/// `text` with the block replaced or appended; returns `(new text, action)`.
pub fn apply_block(text: Option<&str>, block: &str, where_: &str) -> Result<(String, String)> {
    todo!("port of arboria.claudemd.apply_block")
}

/// Refuse lessons that would not fit the block.
pub fn check(ctx: &RepoContext, lessons: &[Lesson]) -> Result<()> {
    todo!("port of arboria.claudemd.check")
}

/// `arb claudemd sync [--no-commit]`.
pub fn sync(ctx: &RepoContext, ledger: &Ledger, commit: bool) -> Result<SyncResult> {
    todo!("port of arboria.claudemd.sync")
}

/// The seed home's CLAUDE.md (`arb install-claude --home`; never committed).
pub fn sync_home(root: &Path) -> Result<SyncResult> {
    todo!("port of arboria.claudemd.sync_home")
}
